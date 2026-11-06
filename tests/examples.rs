//! Worked examples: a program, its input, the model's answers, and what the
//! run must print and the `--trace` log it must write, event by event.
//!
//! The trace runs on virtual time: only the API takes time, `LATENCY` per
//! attempt (and a retry its backoff), so timestamps are the same every run.
//! What the threads still decide is renumbered before comparing: questions
//! by record, then in the order the record asked them; requests by start,
//! then first question; and each request goes on the lowest-numbered sender
//! free when it starts. Events are compared as a set, sorted by time.

mod common;

use common::*;
use deem::RunOptions as Opts;
use deem::api::{Answer, ApiError, Judge, Request, Response};
use serde_json::{Value as Json, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Microseconds the API takes per attempt, and the tokens it reports for one.
const LATENCY: u64 = 100;
const TOKENS: u64 = 10;

/// The model, scripted: an answer per (record, question), and the first
/// `fail` requests answered with a 500.
struct Script {
    answer: fn(&str, &str) -> Json,
    fail: usize,
    calls: AtomicUsize,
}

impl Judge for Script {
    fn judge(&self, req: &Request) -> Result<Response, ApiError> {
        deem::testing::elapse(LATENCY);
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.fail {
            return Err(ApiError::Status(500, "overloaded".into()));
        }
        let answers = asked(req)
            .into_iter()
            .enumerate()
            .map(|(i, (rec, q))| {
                let rec = rec.as_str().map_or_else(|| rec.to_string(), str::to_owned);
                (format!("q{i}"), Answer { value: (self.answer)(&rec, &q), confidence: None, probabilities: None })
            })
            .collect();
        Ok(Response { answers, input_tokens: Some(TOKENS), raw: None })
    }
}

struct Example {
    args: &'static [&'static str],
    program: &'static str,
    input: &'static str,
    answer: fn(&str, &str) -> Json,
    fail: usize,
}

impl Example {
    fn new(program: &'static str, input: &'static str, answer: fn(&str, &str) -> Json) -> Self {
        Self { args: &[], program, input, answer, fail: 0 }
    }

    /// Run it, and check the exit code, the output, and the trace: its
    /// header, then `events`, one per line.
    fn check(self, code: i32, stdout: &str, events: &str) {
        static RUN: AtomicUsize = AtomicUsize::new(0);
        let n = RUN.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("deem-example-{}-{n}.jsonl", std::process::id()));
        let mut opts = Opts { trace: Some(path.to_str().unwrap().to_owned()), ..Opts::default() };
        for a in self.args {
            opts.max_questions = Some(a.strip_prefix("--max-questions=").unwrap().parse().unwrap());
        }
        let cfg = deem::Config { virtual_time: true, ..config() };
        let script = Script { answer: self.answer, fail: self.fail, calls: AtomicUsize::new(0) };
        let r = run_with(self.program, self.input, &opts, &cfg, Arc::new(script));
        let trace = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((r.code, r.stdout.as_str()), (code, stdout), "{}", r.stderr);

        let mut lines = trace.lines();
        // the format, then what the run sent to: the environment's settings, so no `api`
        let head = r#"{"clock":"virtual","deem_trace":1,"model":"jev-latest","url":"http://127.0.0.1:9"}"#;
        assert_eq!(lines.next(), Some(head));
        let got: Vec<Json> = lines.map(|l| serde_json::from_str(l).unwrap()).collect();
        let want: Vec<Json> =
            events.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(sorted(canonical(got)).join("\n"), sorted(want).join("\n"), "\nthe trace:\n{trace}");
    }
}

/// The log with what the threads decided renumbered (see the top).
fn canonical(mut events: Vec<Json>) -> Vec<Json> {
    let u = |e: &Json, k: &str| e[k].as_u64().unwrap();
    let ids =
        |e: &Json, k: &str| -> Vec<u64> { e[k].as_array().unwrap().iter().map(|a| a.as_u64().unwrap()).collect() };
    let of = |kind: &str| -> Vec<Json> { events.iter().filter(|e| e["ev"] == kind).cloned().collect() };

    let mut asks: Vec<(u64, u64)> = of("ask").iter().map(|e| (u(e, "record"), u(e, "question"))).collect();
    asks.sort();
    let q: HashMap<u64, u64> = asks.iter().zip(1..).map(|(&(_, old), new)| (old, new)).collect();

    let ends: HashMap<u64, u64> = of("answered").iter().map(|e| (u(e, "request"), u(e, "t"))).collect();
    let mut order: Vec<(u64, u64, u64)> = of("request")
        .iter()
        .map(|e| (u(e, "t"), ids(e, "questions").iter().map(|x| q[x]).min().unwrap(), u(e, "request")))
        .collect();
    order.sort();
    let (mut r, mut sender, mut free) = (HashMap::new(), HashMap::new(), Vec::<u64>::new());
    for (new, &(t, _, old)) in (1u64..).zip(&order) {
        let lane = free.iter().position(|&f| f <= t).unwrap_or_else(|| {
            free.push(0);
            free.len() - 1
        });
        free[lane] = ends.get(&old).copied().unwrap_or(u64::MAX);
        r.insert(old, new);
        sender.insert(old, lane as u64);
    }

    let map = |xs: Vec<u64>, m: &HashMap<u64, u64>| -> Json {
        let mut xs: Vec<u64> = xs.iter().map(|x| m[x]).collect();
        xs.sort();
        xs.into()
    };
    for e in &mut events {
        if e.get("question").is_some() {
            e["question"] = q[&u(e, "question")].into();
        }
        match e["ev"].as_str().unwrap() {
            // `after` keeps its order: the order the record waited
            "ask" => e["after"] = ids(e, "after").iter().map(|x| q[x]).collect(),
            "wave" => e["requests"] = map(ids(e, "requests"), &r),
            "request" => {
                // what a request carries is checked in tests/trace.rs; the
                // examples show how requests are made
                for k in ["bytes", "body", "pictures"] {
                    e.as_object_mut().unwrap().remove(k);
                }
                let old = u(e, "request");
                (e["request"], e["sender"]) = (r[&old].into(), sender[&old].into());
                e["questions"] = map(ids(e, "questions"), &q);
            }
            "answered" => {
                e["request"] = r[&u(e, "request")].into();
                e.as_object_mut().unwrap().remove("response");
                // a request's answers come in no fixed order
                let mut scores: Vec<f64> =
                    e["scores"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
                scores.sort_by(f64::total_cmp);
                e["scores"] = scores.into();
            }
            _ => {}
        }
    }
    events
}

/// Events as compact JSON, by time.
fn sorted(events: Vec<Json>) -> Vec<String> {
    let mut v: Vec<_> = events.into_iter().map(|e| (e["t"].as_u64().unwrap(), e.to_string())).collect();
    v.sort();
    v.into_iter().map(|(_, s)| s).collect()
}

fn missing(rec: &str, q: &str) -> Json {
    panic!("no answer scripted for {q:?} about {rec:?}")
}

/// Both records go in one wave, a request each.
#[test]
fn a_filter() {
    let answer = |rec: &str, q: &str| match rec {
        "disk full" => json!(0.9),
        "all good" => json!(0.1),
        _ => missing(rec, q),
    };
    let program = r#"select(ask("is it an error?"; .) > 0.5)"#;
    Example::new(program, "disk full\nall good\n", answer).check(
        0,
        "disk full\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"record","record":2}
{"t":0,"ev":"ask","record":1,"question":1,"q":"is it an error?","after":[]}
{"t":0,"ev":"ask","record":2,"question":2,"q":"is it an error?","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[1]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[2]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1]}
{"t":0,"ev":"request","request":2,"sender":1,"questions":[2]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":0,"ev":"wait","record":2,"question":2}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[0.9]}
{"t":100,"ev":"answered","request":2,"result":"ok","attempts":1,"tokens":10,"scores":[0.1]}
{"t":100,"ev":"resume","record":1,"question":1}
{"t":100,"ev":"resume","record":2,"question":2}
{"t":100,"ev":"end","record":1,"outcome":"done"}
{"t":100,"ev":"end","record":2,"outcome":"done"}
"#,
    );
}

/// Two questions that don't depend on each other go out together, and the
/// record waits once: the second answer came with the first.
#[test]
fn independent_questions() {
    Example::new(r#"{a: ask("q1"; .), b: ask("q2"; .)}"#, "x\n", |rec, q| match q {
        "q1" => json!(0.2),
        "q2" => json!(0.7),
        _ => missing(rec, q),
    })
    .check(
        0,
        "{\"a\":0.2,\"b\":0.7}\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"ask","record":1,"question":1,"q":"q1","after":[]}
{"t":0,"ev":"ask","record":1,"question":2,"q":"q2","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":2,"requests":[1]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1,2]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[0.2,0.7]}
{"t":100,"ev":"resume","record":1,"question":1}
{"t":100,"ev":"end","record":1,"outcome":"done"}
"#,
    );
}

/// A question whose text holds an answer waits for it; the one beside it
/// doesn't, and travels with the answer it needs.
#[test]
fn a_dependent_question() {
    Example::new(r#"{a: ask("q1"; .), b: ask("q2 given \(ask("q0"; .))"; .)}"#, "x\n", |rec, q| match q {
        "q0" => json!(0.25),
        "q1" => json!(0.5),
        "q2 given 0.25" => json!(0.75),
        _ => missing(rec, q),
    })
    .check(
        0,
        "{\"a\":0.5,\"b\":0.75}\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"ask","record":1,"question":1,"q":"q1","after":[]}
{"t":0,"ev":"ask","record":1,"question":2,"q":"q0","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":2,"requests":[1]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1,2]}
{"t":0,"ev":"wait","record":1,"question":2}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[0.25,0.5]}
{"t":100,"ev":"resume","record":1,"question":2}
{"t":100,"ev":"ask","record":1,"question":3,"q":"q2 given 0.25","after":[2]}
{"t":100,"ev":"wave","wave":2,"questions":1,"requests":[2]}
{"t":100,"ev":"request","request":2,"sender":0,"questions":[3]}
{"t":100,"ev":"wait","record":1,"question":3}
{"t":200,"ev":"answered","request":2,"result":"ok","attempts":1,"tokens":10,"scores":[0.75]}
{"t":200,"ev":"resume","record":1,"question":3}
{"t":200,"ev":"end","record":1,"outcome":"done"}
"#,
    );
}

/// Only the record that passed the first question asks the second, once
/// the other record has finished.
#[test]
fn a_cascade() {
    let answer = |rec: &str, q: &str| match (rec, q) {
        ("disk full", "error?") => json!(0.9),
        ("all good", "error?") => json!(0.1),
        ("disk full", "severe?") => json!(0.6),
        _ => missing(rec, q),
    };
    let program = r#"select(ask("error?"; .) > 0.5) | ask("severe?"; .)"#;
    Example::new(program, "disk full\nall good\n", answer).check(
        0,
        "0.6\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"record","record":2}
{"t":0,"ev":"ask","record":1,"question":1,"q":"error?","after":[]}
{"t":0,"ev":"ask","record":2,"question":3,"q":"error?","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[1]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[2]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1]}
{"t":0,"ev":"request","request":2,"sender":1,"questions":[3]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":0,"ev":"wait","record":2,"question":3}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[0.9]}
{"t":100,"ev":"answered","request":2,"result":"ok","attempts":1,"tokens":10,"scores":[0.1]}
{"t":100,"ev":"resume","record":1,"question":1}
{"t":100,"ev":"resume","record":2,"question":3}
{"t":100,"ev":"end","record":2,"outcome":"done"}
{"t":100,"ev":"ask","record":1,"question":2,"q":"severe?","after":[1]}
{"t":100,"ev":"wave","wave":2,"questions":1,"requests":[3]}
{"t":100,"ev":"request","request":3,"sender":0,"questions":[2]}
{"t":100,"ev":"wait","record":1,"question":2}
{"t":200,"ev":"answered","request":3,"result":"ok","attempts":1,"tokens":10,"scores":[0.6]}
{"t":200,"ev":"resume","record":1,"question":2}
{"t":200,"ev":"end","record":1,"outcome":"done"}
"#,
    );
}

/// Records' second questions share a wave too.
#[test]
fn two_records_two_waves() {
    let answer = |rec: &str, q: &str| match (rec, q) {
        ("a", "first") => json!(0.1),
        ("b", "first") => json!(0.2),
        ("a", "then 0.1") => json!(0.3),
        ("b", "then 0.2") => json!(0.4),
        _ => missing(rec, q),
    };
    let program = r#"ask("then \(ask("first"; .))"; .)"#;
    Example::new(program, "a\nb\n", answer).check(
        0,
        "0.3\n0.4\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"record","record":2}
{"t":0,"ev":"ask","record":1,"question":1,"q":"first","after":[]}
{"t":0,"ev":"ask","record":2,"question":3,"q":"first","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[1]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[2]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1]}
{"t":0,"ev":"request","request":2,"sender":1,"questions":[3]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":0,"ev":"wait","record":2,"question":3}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[0.1]}
{"t":100,"ev":"answered","request":2,"result":"ok","attempts":1,"tokens":10,"scores":[0.2]}
{"t":100,"ev":"resume","record":1,"question":1}
{"t":100,"ev":"resume","record":2,"question":3}
{"t":100,"ev":"ask","record":1,"question":2,"q":"then 0.1","after":[1]}
{"t":100,"ev":"ask","record":2,"question":4,"q":"then 0.2","after":[3]}
{"t":100,"ev":"wave","wave":2,"questions":1,"requests":[3]}
{"t":100,"ev":"wave","wave":2,"questions":1,"requests":[4]}
{"t":100,"ev":"request","request":3,"sender":0,"questions":[2]}
{"t":100,"ev":"request","request":4,"sender":1,"questions":[4]}
{"t":100,"ev":"wait","record":1,"question":2}
{"t":100,"ev":"wait","record":2,"question":4}
{"t":200,"ev":"answered","request":3,"result":"ok","attempts":1,"tokens":10,"scores":[0.3]}
{"t":200,"ev":"answered","request":4,"result":"ok","attempts":1,"tokens":10,"scores":[0.4]}
{"t":200,"ev":"resume","record":1,"question":2}
{"t":200,"ev":"resume","record":2,"question":4}
{"t":200,"ev":"end","record":1,"outcome":"done"}
{"t":200,"ev":"end","record":2,"outcome":"done"}
"#,
    );
}

/// Asking the same thing about the same record twice sends it once.
#[test]
fn a_repeated_question() {
    Example::new(r#"ask("q"; .) + ask("q"; .)"#, "x\n", |_, _| json!(0.25)).check(
        0,
        "0.5\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"ask","record":1,"question":1,"q":"q","after":[]}
{"t":0,"ev":"reuse","record":1,"question":1}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[1]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[0.25]}
{"t":100,"ev":"resume","record":1,"question":1}
{"t":100,"ev":"end","record":1,"outcome":"done"}
"#,
    );
}

#[test]
fn a_choice() {
    Example::new(r#"choose("which team?"; ["api", "db"]; .)"#, "slow query\n", |_, _| json!("db")).check(
        0,
        "db\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"ask","record":1,"question":1,"q":"which team?","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[1]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[]}
{"t":100,"ev":"resume","record":1,"question":1}
{"t":100,"ev":"end","record":1,"outcome":"done"}
"#,
    );
}

/// A request per record, in one wave, side by side.
#[test]
fn one_record_per_request() {
    Example::new(r#"ask("q"; .)"#, "a\nb\n", |_, _| json!(0.5)).check(
        0,
        "0.5\n0.5\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"record","record":2}
{"t":0,"ev":"ask","record":1,"question":1,"q":"q","after":[]}
{"t":0,"ev":"ask","record":2,"question":2,"q":"q","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[1]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[2]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1]}
{"t":0,"ev":"request","request":2,"sender":1,"questions":[2]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":0,"ev":"wait","record":2,"question":2}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[0.5]}
{"t":100,"ev":"answered","request":2,"result":"ok","attempts":1,"tokens":10,"scores":[0.5]}
{"t":100,"ev":"resume","record":1,"question":1}
{"t":100,"ev":"resume","record":2,"question":2}
{"t":100,"ev":"end","record":1,"outcome":"done"}
{"t":100,"ev":"end","record":2,"outcome":"done"}
"#,
    );
}

/// A 500 is retried after the backoff (1 ms in tests): 100 µs, 1000 µs,
/// 100 µs.
#[test]
fn a_retry() {
    // the failed attempt reports no tokens, so it counts deem's estimate
    let state = serde_json::value::RawValue::from_string(r#""x""#.into()).unwrap();
    let (state, spec) = (&*state, deem::api::Spec { q: "q".into(), ..Default::default() });
    let req = &deem::testing::requests(&config().model, &[(state, &spec)])[0].req;
    let estimate = deem::testing::tokens(req, deem::testing::BYTES_PER_TOKEN);
    Example { fail: 1, ..Example::new(r#"ask("q"; .)"#, "x\n", |_, _| json!(0.5)) }.check(
        0,
        "0.5\n",
        &r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"ask","record":1,"question":1,"q":"q","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[1]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":1200,"ev":"answered","request":1,"result":"ok","attempts":2,"tokens":RETRY,"scores":[0.5]}
{"t":1200,"ev":"resume","record":1,"question":1}
{"t":1200,"ev":"end","record":1,"outcome":"done"}
"#
        .replace("RETRY", &(TOKENS + estimate).to_string()),
    );
}

/// A bad answer fails its record; the other record is fine.
#[test]
fn a_bad_answer() {
    let answer = |rec: &str, _: &str| match rec {
        "boom" => Json::Null,
        _ => json!(0.9),
    };
    Example::new(r#"select(ask("q"; .) > 0.5)"#, "fine\nboom\n", answer).check(
        2,
        "fine\n",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"record","record":2}
{"t":0,"ev":"ask","record":1,"question":1,"q":"q","after":[]}
{"t":0,"ev":"ask","record":2,"question":2,"q":"q","after":[]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[1]}
{"t":0,"ev":"wave","wave":1,"questions":1,"requests":[2]}
{"t":0,"ev":"request","request":1,"sender":0,"questions":[1]}
{"t":0,"ev":"request","request":2,"sender":1,"questions":[2]}
{"t":0,"ev":"wait","record":1,"question":1}
{"t":0,"ev":"wait","record":2,"question":2}
{"t":100,"ev":"answered","request":1,"result":"ok","attempts":1,"tokens":10,"scores":[0.9]}
{"t":100,"ev":"answered","request":2,"result":"ok","attempts":1,"tokens":10,"scores":[]}
{"t":100,"ev":"resume","record":1,"question":1}
{"t":100,"ev":"resume","record":2,"question":2}
{"t":100,"ev":"end","record":1,"outcome":"done"}
{"t":100,"ev":"end","record":2,"outcome":"the answer to \"q\" failed: the model answered null, not a number in [0,1]"}
"#,
    );
}

/// The third question passes the limit, so the run stops and the record
/// fails. The two questions within the limit are dropped, not sent: no
/// record is left to read their answers.
#[test]
fn the_question_limit() {
    Example {
        args: &["--max-questions=2"],
        ..Example::new(r#"ask("a"; .), ask("b"; .), ask("c"; .)"#, "x\n", missing)
    }
    .check(
        2,
        "",
        r#"
{"t":0,"ev":"record","record":1}
{"t":0,"ev":"ask","record":1,"question":1,"q":"a","after":[]}
{"t":0,"ev":"ask","record":1,"question":2,"q":"b","after":[]}
{"t":0,"ev":"stop","why":"stopped at the question limit (2); raise it with --max-questions N, 0 for no limit"}
{"t":0,"ev":"end","record":1,"outcome":"stopped"}
"#,
    );
}
