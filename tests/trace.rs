//! `--trace`: the log is rebuilt into a graph (question → the request that
//! carried it → the record that waited on it) and checked against what the
//! program must do; then the views of it, and logs cut short.
//! `tests/examples.rs` states whole logs for small runs.

mod common;

use common::*;
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

struct Log {
    events: Vec<Json>,
}

impl Log {
    fn read(path: &std::path::Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap();
        let mut lines = text.lines();
        let head: Json = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!((head["deem_trace"].as_u64(), head["clock"].as_str()), (Some(1), Some("real")));
        Self { events: lines.map(|l| serde_json::from_str(l).unwrap()).collect() }
    }

    fn run(args: &[&str], input: &str) -> (Run, Self) {
        let s = fake_api();
        let path = temp(&args.join(" "));
        let mut all = vec!["run", "--trace", path.to_str().unwrap()];
        all.extend(args);
        let r = bin(&all, input, &[("JEV_API_KEY", "sk"), ("JEV_BASE_URL", &s.url)], ".");
        let log = Self::read(&path);
        std::fs::remove_file(path).unwrap();
        (r, log)
    }

    fn all(&self, ev: &str) -> Vec<&Json> {
        self.events.iter().filter(|e| e["ev"] == ev).collect()
    }

    /// The first event of a kind that matches.
    fn find(&self, ev: &str, f: impl Fn(&Json) -> bool) -> &Json {
        self.all(ev).into_iter().find(|e| f(e)).unwrap_or_else(|| panic!("no {ev} event"))
    }

    /// Question → (the asking record, its text, the answers it had waited for before).
    fn asked(&self) -> BTreeMap<u64, (u64, String, Vec<u64>)> {
        let ask = |e: &&Json| {
            let after = e["after"].as_array().unwrap().iter().map(|a| a.as_u64().unwrap()).collect();
            (u(e, "question"), (u(e, "record"), e["q"].as_str().unwrap().to_owned(), after))
        };
        self.all("ask").iter().map(ask).collect()
    }

    /// Question → the request that carried it.
    fn carried_by(&self) -> BTreeMap<u64, u64> {
        let mut m = BTreeMap::new();
        for r in self.all("request") {
            for q in r["questions"].as_array().unwrap() {
                assert!(m.insert(q.as_u64().unwrap(), u(r, "request")).is_none(), "a question in two requests");
            }
        }
        m
    }

    /// When a request's answer came back.
    fn answered(&self, request: u64) -> u64 {
        u(self.find("answered", |e| u(e, "request") == request), "t")
    }
}

fn u(e: &Json, k: &str) -> u64 {
    e[k].as_u64().unwrap_or_else(|| panic!("{k} in {e}"))
}

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("deem-trace-{}-{}.jsonl", std::process::id(), fnv(name)))
}

/// The graph holds together: every question is carried by one request,
/// every request is answered and in one wave, and every wait is on a
/// question, inside its record's run, and resumes after its answer came.
fn consistent(t: &Log) {
    let asked = t.asked();
    let carried = t.carried_by();
    assert_eq!(asked.keys().collect::<BTreeSet<_>>(), carried.keys().collect(), "each question asked is sent once");
    // records send their rounds at once, so the events come in any order
    let mut in_waves: Vec<u64> = t
        .all("wave")
        .iter()
        .flat_map(|w| w["requests"].as_array().unwrap().iter().map(|r| r.as_u64().unwrap()))
        .collect();
    let mut requests: Vec<u64> = t.all("request").iter().map(|r| u(r, "request")).collect();
    in_waves.sort();
    requests.sort();
    assert_eq!(in_waves, requests, "each request in one wave");
    for w in t.all("wait") {
        let (q, rec) = (u(w, "question"), u(w, "record"));
        let resumed = t.find("resume", |e| u(e, "question") == q && u(e, "record") == rec);
        let (start, end) = (t.find("record", |e| u(e, "record") == rec), t.find("end", |e| u(e, "record") == rec));
        assert!(u(start, "t") <= u(w, "t") && u(resumed, "t") <= u(end, "t"), "waiting inside record {rec}");
        assert!(t.answered(carried[&q]) <= u(resumed, "t"), "a wait ends after its answer arrives");
    }
}

#[test]
fn cascade() {
    let input = lines((0..30).map(|i| format!("line {i}")));
    let (r, t) = Log::run(&[r#"select(ask("q1"; .) > 0.5) | {a: ask("q2"; .), b: ask("q3"; .)}"#], &input);
    assert_eq!(r.code, 0);
    consistent(&t);
    let carried = t.carried_by();
    // questions per wave: a wave has an event for each record's round in it
    let mut per_wave: BTreeMap<u64, usize> = BTreeMap::new();
    for w in t.all("wave") {
        let reqs: Vec<u64> = w["requests"].as_array().unwrap().iter().map(|r| r.as_u64().unwrap()).collect();
        *per_wave.entry(u(w, "wave")).or_default() += carried.values().filter(|r| reqs.contains(r)).count();
    }
    // every q1 in wave 1, then the passed records' q2 and q3 in wave 2,
    // however long any answer took
    let passed = r.stdout.lines().count();
    assert_eq!(per_wave, BTreeMap::from([(1, 30), (2, 2 * passed)]));
    let asked = t.asked();
    // q2 and q3 are asked only by records that passed, after their q1 answer
    for e in t.all("ask").iter().filter(|e| e["q"] != "q1") {
        let rec = u(e, "record");
        let q1 = asked.iter().find(|(_, (r, q, _))| *r == rec && q == "q1").unwrap().0;
        let resumed = t.find("resume", |w| u(w, "question") == *q1);
        assert!(u(e, "t") >= u(resumed, "t"), "record {rec} asked before it had its q1 answer");
    }
    assert_eq!(asked.values().filter(|(_, q, _)| q == "q2").count(), passed);
}

#[test]
fn dependent_question_waits_independent_does_not() {
    let (r, t) = Log::run(&[r#"{a: ask("q1"; .), b: ask("q2 given \(ask("q0"; .))"; .)}"#], "x\n");
    assert_eq!(r.code, 0);
    consistent(&t);
    let (asked, carried) = (t.asked(), t.carried_by());
    let id = |text: &str| *asked.iter().find(|(_, (_, q, _))| q.starts_with(text)).unwrap().0;
    let (q1, q0, q2) = (id("q1"), id("q0"), id("q2 given"));
    assert_eq!(carried[&q1], carried[&q0], "q1 and q0 travel together");
    let sent = u(t.find("request", |e| u(e, "request") == carried[&q2]), "t");
    assert!(t.answered(carried[&q0]) <= sent, "q2 is sent after q0's answer");
    assert_eq!(u(t.all("wait")[0], "question"), q0, "the record waits for q0 to build q2's text");
    assert_eq!(asked[&q2].2, [q0], "q2 was asked after the answer to q0");
}

#[test]
fn the_question_limit_is_marked() {
    let (r, t) = Log::run(&["--max-questions", "3", r#"range(1e9) as $i | ask("q \($i)"; .)"#], "x\n");
    assert_eq!(r.code, 2);
    let stopped = t.all("stop");
    assert_eq!(stopped.len(), 1);
    assert!(stopped[0]["why"].as_str().unwrap().contains("question limit (3)"));
    assert_eq!(t.find("end", |e| u(e, "record") == 1)["outcome"], "stopped");
}

#[test]
fn a_failed_record_says_why() {
    let (r, t) = Log::run(&[r#"select(ask("FAIL"; .) > 0.5)"#], "fine\nboom\n");
    assert_eq!(r.code, 2);
    consistent(&t);
    assert_eq!(t.find("end", |e| u(e, "record") == 1)["outcome"], "done");
    let failed = t.find("end", |e| u(e, "record") == 2)["outcome"].as_str().unwrap().to_owned();
    assert!(failed.starts_with("the answer to \"FAIL\" failed"), "{failed}");
}

/// `deem trace FILE`: the dependent-question run, as text.
#[test]
fn the_report() {
    let s = fake_api();
    let path = temp("report");
    let path = path.to_str().unwrap();
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", s.url.as_str())];
    bin(&["run", "--trace", path, r#"{a: ask("q1"; .), b: ask("q2 given \(ask("q0"; .))"; .)}"#], "x\n", &env, ".");
    let r = bin(&["trace", path], "", &[], ".");
    std::fs::remove_file(path).unwrap();
    assert_eq!(r.code, 0, "{}", r.stderr);
    let lines: Vec<&str> = r.stdout.lines().collect();
    assert!(lines[0].starts_with("1 record · 3 questions · 2 requests · 2 waves · "), "{}", r.stdout);
    let q0 = noul("q0", &text("x"));
    let body: Vec<String> = lines
        .iter()
        .map(|l| l.split(" at ").next().unwrap().split(" · ").next().unwrap().trim_end().to_owned())
        .collect();
    let want = [
        "wave 1",
        "  request 1",
        r#"    record 1     #1 "q1", #2 "q0""#,
        "wave 2",
        "  request 2",
        &format!(r#"    record 1     #3 "q2 given {q0}"   after #2"#),
    ];
    let got: Vec<&String> = body
        .iter()
        .filter(|l| l.starts_with("wave") || l.starts_with("  request") || l.starts_with("    record"))
        .collect();
    assert_eq!(got, want);
    let r = bin(&["trace", "missing.jsonl"], "", &[], ".");
    assert_eq!((r.code, r.stderr.as_str()), (2, "deem: missing.jsonl: No such file or directory (os error 2)\n"));
}

/// `deem trace --visual FILE`: a row per request and per record, an arrow
/// into each request from the ones whose answers its record waited for
/// before asking it, and the questions grouped by text with what each group
/// was asked after.
#[test]
fn the_timeline() {
    let s = fake_api();
    let path = temp("visual");
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", s.url.as_str())];
    // a few that fail the first question, and one urgent line for each service
    let input = lines([0, 1, 3, 4, 5, 7, 9, 15].map(|i| format!("line {i} ERROR upstream timeout after {i}s")));
    let program = r#"select(ask("is this an error?"; .) > 0.3)
      | choose("which service?"; ["api", "db", "worker"]; .) as $svc
      | {svc: $svc,
         severe: ask("a user would notice it with p=\(ask("would a user notice?"; .)); is it severe?"; .),
         page: (if ask("is it urgent?"; .) > 0.5 then ask("should we page the \($svc) on-call?"; .) else null end)}"#;
    let p = path.to_str().unwrap();
    bin(&["run", "--trace", p, program], &input, &env, ".");
    let t = Log::read(&path);
    let r = bin(&["trace", "--visual", p], "", &[], ".");
    let text = bin(&["trace", p], "", &[], ".");
    std::fs::remove_file(&path).unwrap();
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.stdout.lines().next(), text.stdout.lines().next(), "the same headline as the text report");

    // what each request needed, from the log itself
    let (asked, carried) = (t.asked(), t.carried_by());
    let mut needed: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
    for (q, (_, _, after)) in &asked {
        let n = carried[q];
        let entry = needed.entry(n).or_default();
        for d in after.iter().map(|a| carried[a]).filter(|&d| d != n) {
            entry.insert(d);
        }
    }
    // a record waits for one answer at a time, so every request after the
    // first wave was held back by exactly one
    assert!(needed.values().all(|d| d.len() <= 1) && needed.values().any(|d| d.len() == 1), "{needed:?}");
    let rows: Vec<&str> = r.stdout.lines().filter(|l| l.starts_with("request ")).collect();
    assert!(needed.len() <= 40, "every request fits the view's 40 rows: {}", needed.len());
    assert_eq!(rows.len(), needed.len(), "{}", r.stdout);
    for (row, (n, deps)) in rows.iter().zip(&needed) {
        assert!(row.starts_with(&format!("request {n} ")), "{row}");
        let deps: Vec<String> = deps.iter().map(u64::to_string).collect();
        let want = match deps.len() {
            0 => None,
            1 => Some(format!("after request {}", deps[0])),
            _ => Some(format!("after requests {}", deps.join(", "))),
        };
        assert_eq!(row.find("] after ").map(|i| &row[i + 2..]), want.as_deref(), "{row}");
        // an arrow in (`├` where a line to a later request crosses it) exactly when it needed one
        assert_eq!(row.contains(['└', '├']), !deps.is_empty(), "{row}");
    }
    assert_eq!(r.stdout.lines().filter(|l| l.starts_with("record ")).count(), 8);

    // the questions, by group: letter → (text, what it was asked after)
    let groups: BTreeMap<String, (String, String)> = r
        .stdout
        .split("\nquestions")
        .nth(1)
        .unwrap()
        .lines()
        .skip(1)
        .map(|l| {
            let (letter, rest) = l.trim().split_once(' ').unwrap();
            let text = rest.trim().split(" · ").next().unwrap().to_owned();
            let after = rest.split(" · after ").nth(1).unwrap_or("").to_owned();
            (letter.to_owned(), (text, after))
        })
        .collect();
    let find =
        |text: &str| groups.iter().find(|g| g.1.0 == text).map(|g| g.0.clone()).unwrap_or_else(|| panic!("{text}"));
    let (error, notice, urgent) =
        (find(r#""is this an error?""#), find(r#""would a user notice?""#), find(r#""is it urgent?""#));
    let after = |text: &str| groups[&find(text)].1.clone();
    assert_eq!(after(r#""which service?""#), error);
    assert_eq!(after(r#""a user would notice it with p=…; is it severe?""#), notice, "numbers are masked");
    assert_eq!(after(r#""would a user notice?""#), error);
    assert_eq!(after(r#""is it urgent?""#), notice, "asked once the record had waited for `notice`");
    for svc in ["api", "db", "worker"] {
        // asked in the `then` branch of `ask("is it urgent?")`; the
        // service's answer in its text was known by then
        assert_eq!(after(&format!(r#""should we page the {svc} on-call?""#)), urgent, "{svc}");
    }

    let r = bin(&["trace", "--visual"], "", &[], ".");
    assert_eq!(
        (r.code, r.stderr.as_str()),
        (2, "deem: usage: deem trace [--visual] FILE, or deem trace compare --with NAME FILE...\n")
    );
}

// A request's event says how large its body was. Three questions about a
// long line send it once.
#[test]
fn what_a_request_carries() {
    let input = format!("{}\nshort\n", "x".repeat(2000));
    let program = r#"[ask("q1"; .), ask("q2"; .), ask("q3"; .)]"#;
    let (r, log) = Log::run(&[program], &input);
    let mut bytes: Vec<u64> = log.all("request").iter().map(|e| u(e, "bytes")).collect();
    bytes.sort();
    assert_eq!(bytes.len(), 2, "a request per record");
    assert!(2000 < bytes[1] && bytes[1] < 2 * 2000, "the long line once: {bytes:?}");
    assert_eq!(r.stderr, "");
}

// A request's event has its body as sent: the value as its state, and its
// questions named q0, q1, … in the order of `questions`. Its answered event
// has the reply as received, which answers each by that name.
#[test]
fn requests_and_responses() {
    let program = r#"{a: ask("is it an error?"; .), b: choose("which?"; ["api", "db"]; .)}"#;
    let (r, log) = Log::run(&[program], "disk full\nall good\n");
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    let asked = log.asked();
    let out: Vec<Json> = r.lines().iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(log.all("request").len(), 2);
    for req in log.all("request") {
        let body = &req["body"];
        assert_eq!(body["model"], "jev-latest");
        let record = body["state"].as_str().unwrap();
        let ids: Vec<u64> = req["questions"].as_array().unwrap().iter().map(|q| q.as_u64().unwrap()).collect();
        let answered = log.find("answered", |e| e["request"] == req["request"]);
        for (i, id) in ids.iter().enumerate() {
            let q = &body["questions"][format!("q{i}")];
            assert_eq!(q["instructions"].as_str().unwrap(), asked[id].1, "q{i} is question #{id}");
            let a = &answered["response"]["answers"][format!("q{i}")];
            let line = ["disk full", "all good"].iter().position(|l| *l == record).unwrap();
            let field = if asked[id].1 == "is it an error?" { "a" } else { "b" };
            let kind = a["type"].as_str().unwrap();
            assert_eq!(a[kind], out[line][field], "the answer the program got");
            if field == "b" {
                assert_eq!(q["criteria"], serde_json::json!({"api": null, "db": null}));
            }
        }
        assert_eq!(body["questions"].as_object().unwrap().len(), ids.len());
        assert_eq!(answered["response"]["answers"].as_object().unwrap().len(), ids.len());
        assert!(req.get("pictures").is_none(), "no pictures, no map");
    }
}

// A reply that leaves a question unanswered fails its request as broken,
// and the trace keeps it as received: it is the one worth reading.
#[test]
fn a_broken_reply_is_kept() {
    let reply = r#"{"answers": {"q0": {"type": "noul", "noul": 0.9}}}"#;
    let s = stub(move |_, _, _| (200, reply.to_owned()));
    let path = temp("a broken reply");
    let program = r#"[ask("q1"; .), ask("q2"; .)]"#;
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", s.url.as_str())];
    let r = bin(&["run", "--trace", path.to_str().unwrap(), program], "line\n", &env, ".");
    assert_eq!(r.code, 2, "{}", r.stderr);
    let log = Log::read(&path);
    std::fs::remove_file(path).unwrap();
    let answered = log.all("answered")[0];
    assert!(answered["result"].as_str().unwrap().contains("no answer for q1"), "{answered}");
    assert_eq!(answered["response"], serde_json::from_str::<Json>(reply).unwrap());
}

/// The dependent-question run of `tests/examples.rs`, as its log.
const DEPENDENT: &str = r#"{"deem_trace":1,"clock":"virtual"}
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
"#;

/// A kill can cut the last line short; it's skipped. A broken line
/// anywhere else, or a file that isn't a trace, is an error.
#[test]
fn a_torn_log() {
    let torn = format!("{DEPENDENT}{{\"t\":250,\"ev\":\"ans");
    assert_eq!(deem::trace::report(&torn), deem::trace::report(DEPENDENT));
    let broken = DEPENDENT.replacen("{\"t\":0,\"ev\":\"wave\"", "{\"t\":0,\"ev\":\"wav", 1);
    assert_eq!(deem::trace::report(&broken).unwrap_err(), "not a deem trace: line 5 is not an event");
    let old = "[\n{\"ph\":\"M\"}\n]\n";
    assert!(deem::trace::report(old).unwrap_err().starts_with("not a deem trace: the first line"));
}

/// A run killed while a request is out: the log has everything up to the
/// kill, and the views show what was still open.
#[test]
fn a_killed_run() {
    use std::io::Write;
    let s = stub(|_, _, _| {
        std::thread::sleep(std::time::Duration::from_secs(60));
        (500, String::new())
    });
    let path = temp("killed");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_deem"))
        .args(["--trace", path.to_str().unwrap(), r#"ask("q"; .)"#])
        .env_clear()
        .envs(OFFLINE)
        .env("JEV_API_KEY", "sk")
        .env("JEV_BASE_URL", &s.url)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"x\n").unwrap();
    let sent = (0..500).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::read_to_string(&path).is_ok_and(|t| t.contains(r#""ev":"wait""#) && t.contains(r#""ev":"request""#))
    });
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(sent, "the request went out and the record waited");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    // wrapped lines, rejoined
    let report = deem::trace::report(&text).unwrap().replace("\n      ", " ");
    assert!(report.contains("· 1 record, 1 question · still out when the trace ends"), "{report}");
    assert!(report.contains("still running when the trace ends · waited on #1 (still waiting)"), "{report}");
    assert!(deem::trace::visual(&text).unwrap().contains("still out when the trace ends"));
}

/// The fake, its yes-or-no answers turned around: 1 − the fake's.
struct Mirror(Fake);

impl deem::api::Judge for Mirror {
    fn judge(&self, req: &deem::api::Request) -> Result<deem::api::Response, deem::api::ApiError> {
        let mut r = self.0.judge(req)?;
        for (k, a) in r.answers.iter_mut() {
            if let (Some(x), "noul") = (a.value.as_f64(), question_sent(req, k)["type"].as_str().unwrap()) {
                a.value = serde_json::json!(((1.0 - x) * 100.0).round() / 100.0);
            }
        }
        Ok(r)
    }
}

/// The table in a report that starts with `head`: its lines, to a blank one.
fn table<'a>(report: &'a str, head: &str) -> Vec<&'a str> {
    let lines = report.lines().skip_while(|l| !l.trim_start().starts_with(head));
    lines.take_while(|l| !l.is_empty()).collect()
}

fn w(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

// `deem trace compare`: the same program run against two APIs, a trace
// each, and one more trace of the reference alone. Answers are matched by
// what was asked (question and value), whichever trace has them; a value
// only one API was asked about is counted, not compared.
#[test]
fn traces_are_compared() {
    let h = scratch("trace-compare");
    let dir = h.to_str().unwrap();
    let deem = |args: &[&str]| bin(args, "", &[("HOME", dir)], dir);
    let (a, m) = (fake_api(), api(Mirror(Fake::default())));
    for (name, s) in [("a", &a), ("m", &m)] {
        assert_eq!(deem(&["api", "add", name, &s.url]).code, 0);
    }
    let lines: Vec<String> = (1..=40).map(|i| format!("ticket {i}")).collect();
    std::fs::write(h.join("t.txt"), lines.join("\n") + "\n").unwrap();
    std::fs::write(h.join("u.txt"), "ticket 1\nsomething else\n").unwrap();
    let program = r#"[ask("is it urgent?"; .), choose("topic?"; ["x", "y", "z"]; .)]"#;
    for name in ["a", "m"] {
        let r = deem(&["--api", name, "--trace", &format!("{name}-1.jsonl"), "run", program, "t.txt"]);
        assert_eq!(r.code, 0, "{}", r.stderr);
    }
    // only a: one value m was asked about too, one it wasn't
    let r = deem(&["--api", "a", "--trace", "a-2.jsonl", "run", program, "u.txt"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    // and a question of 2 values, first in the files
    for name in ["a", "m"] {
        let r = deem(&["--api", name, "--trace", &format!("{name}-0.jsonl"), "-p", "is it small?", "u.txt"]);
        assert_eq!(r.code, 0, "{}", r.stderr);
    }
    let r = deem(&["trace", "compare", "--with", "a", "a-0.jsonl", "a-1.jsonl", "m-0.jsonl", "m-1.jsonl", "a-2.jsonl"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let out = &r.stdout;
    // the APIs, where each is, then a table of their answers
    let uw = a.url.len().max(m.url.len());
    assert!(out.starts_with(&format!("a  {:<uw$}  jev-latest\nm  {:<uw$}  jev-latest\n", a.url, m.url)), "{out}");
    let words = |l: &str| l.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    let answers = table(out, "answers");
    // a has 40 + 2 values, the first of u.txt's again, and 2 more; m has 42
    assert_eq!(
        (words(answers[1])[..3].to_vec(), words(answers[2])[..3].to_vec()),
        (w(&["a", "84", "0"]), w(&["m", "82", "0"]))
    );
    assert!(out.contains("\nanswered again, the first answer kept: a 2\n"), "{out}");
    // each question, the one with the most answers first, though asked
    // last, under a row for all; one of fewer than 20 after a line that
    // says it is a smoke test. Only the values both answered count: a's
    // 41st isn't one
    let ask = table(out, "ask");
    assert_eq!(ask[0], "ask: how often each API answers as a does", "{out}");
    assert_eq!((words(ask[1]), words(ask[2])), (w(&["answers", "m"]), w(&["compared", "agrees"])), "{out}");
    assert!(ask[3].starts_with("all 2 questions  ") && ask[4].starts_with("is it urgent?  "), "{out}");
    assert!(ask[5].starts_with("── fewer than 20 answers") && ask[6].starts_with("is it small?  "), "{out}");
    // the mirror answers every yes no and every no yes
    let same = lines.iter().filter(|l| noul("is it urgent?", &format!("{l:?}")) == 0.5).count();
    let pct = format!("({:.0}%)", same as f64 * 100.0 / 40.0);
    assert_eq!(words(ask[4])[3..].to_vec(), w(&["40", &same.to_string(), &pct]), "{out}");
    assert!(words(ask[6]).ends_with(&w(&["2", "0/2"])), "{out}");
    // and leaves every option as it was
    let choose = table(out, "choose");
    assert!(choose[4].starts_with("topic?  ") && choose[4].ends_with(" 40  40 (100%)"), "{out}");
    assert!(out.contains("\nm against a\n"), "{out}");
    // the mirror orders every yes below every no, and differs either way;
    // each column's head names both APIs
    let detail = out.split("\nm against a\n").nth(1).unwrap();
    let head = table(detail, "ask");
    assert!(head[0].ends_with("  m yes,  m no,") && head[1].ends_with("  a no  a yes"), "{out}");
    let urgent = words(table(detail, "ask")[2]);
    assert_eq!(urgent[3], "0.00", "{out}");
    let (yes, no): (usize, usize) = (urgent[5].parse().unwrap(), urgent[6].parse().unwrap());
    assert!(yes > 0 && no > 0 && yes + no == 40 - same, "{out}");
    // the same options, so no table of choices
    assert!(!out.contains("choices that differ"), "{out}");
    // what wasn't compared, and why
    let apart = table(out, "not compared");
    assert!(apart.iter().any(|l| l.starts_with("is it urgent?  ") && l.ends_with("  1 only in a")), "{out}");
    // every largest difference the other way, so each marked
    let largest = table(out, "largest differences");
    assert_eq!(largest[1], "     m     a      Δ", "{out}");
    assert!(largest[2..].iter().step_by(2).all(|l| l.starts_with("≠ ")), "{out}");
    // pictures pair by their content, and are shown by their files
    for (name, size) in [("p1.png", (800, 600)), ("p2.png", (640, 480))] {
        std::fs::write(h.join(name), png(size.0, size.1)).unwrap();
    }
    for name in ["a", "m"] {
        assert_eq!(deem(&["api", "edit", name, "--pictures"]).code, 0);
        let r = deem(&["--api", name, "--trace", &format!("{name}-p.jsonl"), "-p", "is it red?", "p1.png", "p2.png"]);
        assert_eq!(r.code, 0, "{}", r.stderr);
    }
    let r = deem(&["trace", "compare", "--with", "a", "a-p.jsonl", "m-p.jsonl"]);
    let ask = table(&r.stdout, "ask");
    assert!(ask[5].starts_with("is it red?  ") && ask[5].contains("/2"), "{}", r.stdout);
    assert!(r.stdout.contains("  is it red?\n                     {\"image\":\"p1.png\"}\n"), "{}", r.stdout);
    // a third API without pictures: the side-by-side tables keep what every
    // API answered, a table of its own says what they left out and how each
    // did, and each pair's section still compares it
    let n = fake_api();
    assert_eq!(deem(&["api", "add", "n", &n.url]).code, 0);
    let r = deem(&["--api", "n", "--trace", "n-1.jsonl", "run", program, "t.txt"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let files = ["a-p.jsonl", "m-p.jsonl", "a-1.jsonl", "m-1.jsonl", "n-1.jsonl"];
    let r = deem(&[&["trace", "compare", "--with", "a"][..], &files].concat());
    assert_eq!(r.code, 0, "{}", r.stderr);
    let out = &r.stdout;
    assert!(!table(out, "ask").iter().any(|l| l.starts_with("is it red?")), "{out}");
    let left = table(out, "not every API answered these");
    assert_eq!(words(left[1]), w(&["m", "n"]), "{out}");
    assert!(words(left[2]).ends_with(&w(&["0/2", "-"])) && left[2].starts_with("is it red?  "), "{out}");
    let section = out.split("\nm against a\n").nth(1).unwrap();
    assert!(table(section, "ask").iter().any(|l| l.starts_with("is it red?  ")), "{out}");
    // each part under a title between two rules
    let rule = "═".repeat(80);
    for title in ["every API against a", "m against a", "n against a"] {
        assert!(out.contains(&format!("\n\n{rule}\n{title}\n{rule}\n")), "{title}\n{out}");
    }
    // a trace of the environment's API, without a config file, is named by its URL
    let bare = scratch("trace-compare-bare");
    let env = [("HOME", bare.to_str().unwrap()), ("JEV_BASE_URL", a.url.as_str())];
    let r = bin(&["--trace", h.join("e.jsonl").to_str().unwrap(), "run", program, "t.txt"], "", &env, dir);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let r = deem(&["trace", "compare", "--with", "a", "a-1.jsonl", "e.jsonl"]);
    let detail = r.stdout.split(&format!("\n{} against a\n", a.url)).nth(1).expect(&r.stdout);
    assert_eq!(words(table(detail, "ask")[2])[3..].to_vec(), w(&["1.00", "+0.00", "0", "0"]), "{}", r.stdout);
    let _ = std::fs::remove_dir_all(bare);
    // what can't be compared says why
    let err = |args: &[&str]| deem(args).stderr;
    assert_eq!(
        err(&["trace", "compare", "--with", "z", "a-1.jsonl", "m-1.jsonl"]),
        "deem: --with z: no trace of it here; these are traces of a, m\n"
    );
    assert_eq!(
        err(&["trace", "compare", "--with", "a", "a-1.jsonl", "a-2.jsonl"]),
        "deem: only traces of a: give traces of another API to compare with it\n"
    );
    assert!(
        err(&["trace", "compare", "--with", "a", "a-1.jsonl", "t.txt"]).starts_with("deem: t.txt: not a deem trace")
    );
    let _ = std::fs::remove_dir_all(h);
}
