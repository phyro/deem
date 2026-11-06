//! Batching: every test asserts wave and request counts from the fake's
//! log, not just outputs.

mod common;

use common::*;
use deem::RunOptions as Opts;
use deem::api::{Answer, ApiError, Judge, Request, Response};
use serde_json::{Value as Json, json};
use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

fn json_lines(n: usize, f: impl Fn(usize) -> Json) -> String {
    lines((0..n).map(|i| f(i).to_string()))
}

// Independent questions about one record travel together, in the record's
// own request.
#[test]
fn two_fields_one_wave() {
    let recs = records48();
    let program = r#"{a: ask("q1"; .), b: ask("q2"; .)}"#;
    let r = deem(program, &lines(&recs));
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    assert_eq!(r.waves(), 1);
    assert_eq!(r.log.len(), 48, "a request per record");
    for req in &r.log {
        assert_eq!(req.questions.len(), 2);
    }
    let one = deem(program, &lines(&recs));
    assert_eq!((one.code, one.stderr.as_str(), one.waves(), one.log.len()), (0, "", 1, 48), "a request per record");
    assert!(one.log.iter().all(|req| common::records(req).len() == 1 && req.questions.len() == 2));
    assert_eq!(one.lines(), r.lines());
    for (rec, line) in recs.iter().zip(r.lines()) {
        let want = json!({"a": noul("q1", &text(rec)), "b": noul("q2", &text(rec))});
        assert_eq!(line, want.to_string());
    }
}

// A question behind `select` waits for the answer it depends on.
#[test]
fn cascade_two_waves() {
    let recs = records48();
    let r = deem(r#"select(ask("q1"; .) > 0.5) | ask("q2"; .)"#, &lines(&recs));
    assert_eq!(r.waves(), 2);
    let passed: BTreeSet<&String> = recs.iter().filter(|s| noul("q1", &text(s)) > 0.5).collect();
    assert!(!passed.is_empty() && passed.len() < recs.len());
    let asked = r.asked().concat();
    let q2: BTreeSet<&str> = asked.iter().filter(|(_, q)| q == "q2").map(|(s, _)| s.as_str().unwrap()).collect();
    assert_eq!(q2, passed.iter().map(|s| s.as_str()).collect(), "q2 is asked only for records that passed");
    assert_eq!(r.lines().len(), passed.len());
}

// A question computed from an answer is sent with that answer in it.
#[test]
fn computed_question_two_waves() {
    let recs = records48();
    let r = deem(r#"ask("Assuming " + (ask("a"; .) | tostring) + "?"; .)"#, &lines(&recs));
    assert_eq!(r.waves(), 2);
    let asked = r.asked().concat();
    for rec in &recs {
        let want = format!("Assuming {}?", noul("a", &text(rec)));
        assert!(asked.contains(&(Json::String(rec.clone()), want.clone())), "{want} for {rec}");
    }
}

// Binding an answer to a variable does not wait for it.
#[test]
fn binding_does_not_wait() {
    let recs = records48();
    let r = deem(r#"ask("q1"; .) as $x | ask("q2"; .) | [$x, .]"#, &lines(&recs));
    assert_eq!(r.waves(), 1);
    assert_eq!(r.asked().concat().len(), 2 * recs.len());
    let want = format!("[{},{}]", noul("q1", &text(&recs[0])), noul("q2", &text(&recs[0])));
    assert_eq!(r.lines()[0], want);
}

// Questions whose text, or whose judged value, differs per record.
#[test]
fn values_per_record_one_wave() {
    let recs = records48();
    let r = deem(r#"ask("about " + . + "?"; .)"#, &lines(&recs));
    assert_eq!(r.waves(), 1);
    let qs: HashSet<String> = r.asked().concat().into_iter().map(|(_, q)| q).collect();
    assert_eq!(qs.len(), recs.len());

    let input = json_lines(48, |i| json!({"id": i, "xs": [format!("x{i}"), format!("y{i}"), format!("z{i}")]}));
    let r = deem_json(r#".xs[] | ask("q"; .)"#, &input);
    assert_eq!(r.waves(), 1, "every element of every record in one wave");
    assert_eq!(r.asked().concat().len(), 3 * 48);
    assert_eq!(r.lines()[15..18], ["x5", "y5", "z5"].map(|s| noul("q", &text(s)).to_string()));

    let input = json_lines(48, |i| json!({"a": format!("a{i}"), "b": format!("b{i}")}));
    let r = deem_json(r#".[] | ask("q"; .)"#, &input);
    assert_eq!((r.waves(), r.asked().concat().len()), (1, 96));
}

// Arithmetic on a deferred value is deferred, so these stay at one wave
// (jaq builds `{k: a, v: b}` with `+`).
#[test]
fn objects_in_arrays_one_wave() {
    let input = json_lines(48, |i| json!({"xs": [format!("x{i}"), format!("y{i}")]}));
    for program in [
        r#"[.xs[] | {k: ., v: ask("q"; .)}] | max_by(.v) | .k"#,
        r#"reduce .xs[] as $x (0; . + ($x | ask("q"; .)))"#,
        r#".xs | map(ask("q"; .)) | add / length"#,
        r#".xs[] |= ask("q"; .)"#,
        r#"{a: .xs[0], b: .xs[1]} | map_values(ask("q"; .))"#,
        r#".xs[0] |= ask("q"; .)"#,
        r#". * {meta: {a: (.xs[0] | ask("q"; .)), b: (.xs[1] | ask("q"; .))}}"#,
        r#".xs | map(ask("q"; .)) | .[0] * .[1]"#,
    ] {
        let r = deem_json(program, &input);
        assert_eq!((r.stderr.as_str(), r.waves()), ("", 1), "{program}");
    }
}

// What costs a wave per question: a comparison or condition looks at its
// answer before jaq evaluates the next part of the program.
#[test]
fn forced_before_next_question() {
    let input = json_lines(48, |i| json!({"xs": [format!("x{i}"), format!("y{i}"), format!("z{i}")]}));
    for (program, waves) in [
        (r#"{x: (ask("q1"; .) > 0.5), y: (ask("q2"; .) > 0.5)}"#, 2),
        (r#".xs | map(select(ask("q"; .) > 0.5))"#, 3),
        (r#".xs | map(ask("q"; .)) | map(select(. > 0.5))"#, 1), // the rewrite
    ] {
        assert_eq!(deem_json(program, &input).waves(), waves, "{program}");
    }
}

// The README's chain of conditions: `and` is a cascade, a wave per question
// and later questions only for the records that passed; asked into an array
// first, it is one wave and every record is asked everything.
#[test]
fn and_chain_vs_array() {
    let recs = records48();
    let chain = r#"select(ask("q1"; .) > 0.5 and ask("q2"; .) > 0.5 and ask("q3"; .) > 0.5)"#;
    let array =
        r#"[ask("q1"; .), ask("q2"; .), ask("q3"; .)] as [$a, $b, $c] | select($a > 0.5 and $b > 0.5 and $c > 0.5)"#;
    let (chain, array) = (deem(chain, &lines(&recs)), deem(array, &lines(&recs)));
    let yes = |q: &str, s: &String| noul(q, &text(s)) > 0.5;
    let pass1 = recs.iter().filter(|s| yes("q1", s)).count();
    let pass2 = recs.iter().filter(|s| yes("q1", s) && yes("q2", s)).count();
    assert!(0 < pass2 && pass2 < pass1 && pass1 < recs.len());
    assert_eq!((chain.waves(), chain.asked().concat().len()), (3, recs.len() + pass1 + pass2));
    assert_eq!((array.waves(), array.asked().concat().len()), (1, 3 * recs.len()));
    assert_eq!(chain.lines(), array.lines());
}

// The README's two-question label is one wave.
#[test]
fn triage_one_wave() {
    let program = std::fs::read_to_string("examples/.deem/triage.deem").unwrap();
    let input = std::fs::read_to_string("examples/app.log").unwrap();
    let r = deem(&program, &input);
    assert_eq!((r.waves(), r.log.len()), (1, 2), "2 ERROR lines, a request each");
    assert!(r.asked().iter().all(|req| req.len() == 2), "a line's 2 questions in its request");
}

// The manual's 500-record example: 500 requests, each record's text once
// with both questions, 192 records at a time; each record asks once, so the
// run is one wave however the window refills.
#[test]
fn five_hundred_records() {
    let input = lines((0..500).map(|i| format!("line {i}")));
    let r = deem(r#"{text: ., notice: ask("would a user notice?"; .), severe: ask("is this severe?"; .)}"#, &input);
    assert_eq!(r.lines().len(), 500);
    assert_eq!(r.log.len(), 500, "a request per record");
    assert!(r.log.iter().all(|q| q.questions.len() == 2), "both questions in a record's request");
    assert_eq!(r.stats.wave_sizes, [1000], "one wave");
}

// More records than the window (192 at 80 requests a second, one to a
// request): no deadlock, and each record's one round is wave 1.
#[test]
fn past_the_window() {
    let input = lines((0..600).map(|i| format!("line {i}")));
    let r = deem(r#"{a: ask("q1"; .), b: ask("q2"; .)}"#, &input);
    assert_eq!((r.code, r.lines().len()), (0, 600));
    assert_eq!(r.asked().concat().len(), 1200);
    assert_eq!(r.stats.wave_sizes, [1200], "one wave, a window at a time");
    for (i, l) in r.lines().iter().enumerate() {
        assert_eq!(
            *l,
            json!({"a": noul("q1", &text(&format!("line {i}"))), "b": noul("q2", &text(&format!("line {i}")))})
                .to_string()
        );
    }
}

// Identical questions are asked once per run, within a wave and across.
#[test]
fn memo() {
    let r = deem(r#"ask("q"; .)"#, &lines(vec!["same line"; 500]));
    assert_eq!((r.lines().len(), r.stats.questions, r.log.len()), (500, 1, 1));
    let r = deem(r#"select(ask("q"; .) >= 0) | select(ask("q"; .) >= 0) | ask("q2"; .)"#, &lines(["a", "b"]));
    assert_eq!((r.waves(), r.stats.questions), (2, 4), "the repeated question is answered from the memo");
}

// One answer per question per run, even when the model would answer
// differently each time it's asked and many records ask at once: the memo
// is checked and filled under the batcher's lock, so a question is only
// ever sent once, and every asker shares its answer.
#[test]
fn one_answer_per_question_per_run() {
    /// A model that never answers the same way twice.
    struct Fickle(std::sync::atomic::AtomicU64);
    impl deem::api::Judge for Fickle {
        fn judge(&self, req: &deem::api::Request) -> Result<deem::api::Response, deem::api::ApiError> {
            let answers = (0..req.questions.len())
                .map(|i| {
                    let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let value = json!((n % 99 + 1) as f64 / 100.0);
                    (format!("q{i}"), deem::api::Answer { value, confidence: None, probabilities: None })
                })
                .collect();
            Ok(deem::api::Response { answers, input_tokens: None, raw: None })
        }
    }
    let fickle = || Arc::new(Fickle(std::sync::atomic::AtomicU64::new(0)));
    // 300 records at once, all the same line: one question, one answer
    let r = run_with(r#"ask("q"; .)"#, &lines(vec!["same line"; 300]), &Opts::default(), &config(), fickle());
    let answers: std::collections::BTreeSet<&str> = r.stdout.lines().collect();
    assert_eq!((r.stdout.lines().count(), answers.len(), r.stats.questions), (300, 1, 1));
    // one record asking the same thing three ways, before and after waiting
    let program = r#"[ask("q"; .), ask("q"; .), (ask("q"; .) as $a | ask("other \($a)"; .) as $b | ask("q"; .))] | unique | length"#;
    let r = run_with(program, "a line\n", &Opts::default(), &config(), fickle());
    assert_eq!(r.stdout, "1\n", "every ask(\"q\"; .) saw the same answer");
}

// While the first record's request is slow, the records behind it finish
// and keep their output until it prints. At most four windows of records
// may be started and not printed, so the reader waits instead of holding
// the whole input's output: here, with a window of 4, at most 16 questions
// are asked before the slow answer returns, not 60.
#[test]
fn a_slow_record_holds_back_at_most_four_windows() {
    /// The fake, but a request with a "SLOW" question notes how many
    /// questions had been asked when it returns.
    struct HeldHead {
        fake: Fake,
        seen: std::sync::atomic::AtomicUsize,
    }
    impl deem::api::Judge for HeldHead {
        fn judge(&self, req: &deem::api::Request) -> Result<deem::api::Response, deem::api::ApiError> {
            let slow = asked(req).iter().any(|(_, q)| q == "SLOW");
            if slow {
                std::thread::sleep(Duration::from_millis(500));
            }
            let r = self.fake.judge(req);
            if slow {
                let asked: usize = self.fake.log.lock().unwrap().iter().map(|r| r.questions.len()).sum();
                self.seen.store(asked, std::sync::atomic::Ordering::SeqCst);
            }
            r
        }
    }
    let judge = Arc::new(HeldHead { fake: Fake::default(), seen: 0.into() });
    let mut cfg = config();
    cfg.window = 4;
    let input = lines((0..60).map(|i| format!("line {i}")));
    let program = r#"if . == "line 0" then ask("SLOW"; .) else ask("q"; .) end"#;
    let r = run_with(program, &input, &Opts::default(), &cfg, judge.clone());
    assert_eq!((r.code, r.lines().len()), (0, 60), "{}", r.stderr);
    let seen = judge.seen.load(std::sync::atomic::Ordering::SeqCst);
    assert!((1..=16).contains(&seen), "{seen} questions asked while the first record waited");
}

// A record that queues questions without looking at them parks at 64, so
// waves and memory stay bounded, and the answers are still right.
#[test]
fn unanswered_questions_are_bounded() {
    let r = deem(r#"[range(200) as $i | ask("q \($i)"; .)] | length"#, "one record\n");
    assert_eq!((r.code, r.stdout.as_str()), (0, "200\n"));
    assert_eq!(r.stats.wave_sizes, [64, 64, 64, 8]);

    // endless output: a chunk at a time
    let r = deem(r#"limit(1000; repeat(ask("q"; .)))"#, "x\n");
    assert_eq!((r.lines().len(), r.stats.questions), (1000, 1));
}

// A runaway program stops at the question limit: questions already sent
// are answered, records that finished are written in order, and the run
// exits 2 with one message. Which of the others finish depends on whether
// they asked before the runaway used the limit up (DESIGN.md, "the cap").
#[test]
fn runaway_stops_at_the_limit() {
    let input = lines(["small", "runaway", "after"]);
    let opts = Opts { max_questions: Some(50), ..Opts::default() };
    let program = r#"ask("q"; .) as $a | if . == "runaway" and $a > 0 then range(1e9) as $i | ask("is \($i) special?"; .) else $a end"#;
    let r = deem_opts(program, &input, &opts);
    assert_eq!(r.code, 2);
    let finished = [noul("q", &text("small")), noul("q", &text("after"))].map(|x| x.to_string());
    let printed = r.lines();
    let want: Vec<&str> = finished.iter().map(String::as_str).filter(|l| printed.contains(l)).collect();
    assert_eq!(printed, want, "only finished records, in order");
    assert!(r.stats.questions <= 50, "{}", r.stats.questions);
    assert_eq!(r.stderr, "deem: stopped at the question limit (50); raise it with --max-questions N, 0 for no limit\n");
}

// Answers resolve per request: a slow request doesn't hold back records
// answered by a fast one, even across waves.
#[test]
fn slow_request_does_not_block() {
    // `a`'s request doesn't answer until `b`'s follow-up has been sent: if
    // `b` waited for `a`, the gate times out and says so. `b`'s second round
    // goes when `b` waits, whatever else is out.
    /// open, its signal, the requests, and whether the slow one timed out
    struct Gate(Mutex<bool>, Condvar, Mutex<Vec<Request>>, Mutex<bool>);
    impl Judge for Gate {
        fn judge(&self, req: &Request) -> Result<Response, ApiError> {
            self.2.lock().unwrap().push(req.clone());
            let (rec, q) = asked(req).remove(0);
            if q == "SLOW" {
                let open = self.0.lock().unwrap();
                let (open, t) = self.1.wait_timeout_while(open, Duration::from_secs(10), |open| !*open).unwrap();
                drop(open);
                *self.3.lock().unwrap() = t.timed_out();
            } else if q.starts_with("then ") {
                *self.0.lock().unwrap() = true;
                self.1.notify_all();
            }
            let answer = Answer { value: json!(noul(&q, &rec.to_string())), confidence: None, probabilities: None };
            Ok(Response { answers: [("q0".to_owned(), answer)].into(), input_tokens: None, raw: None })
        }
    }
    let gate = Arc::new(Gate(Mutex::new(false), Condvar::new(), Mutex::default(), Mutex::new(false)));
    let program = r#"if . == "a" then ask("SLOW"; .) else ask("then \(ask("first"; .))"; .) end"#;
    let r = run_with(program, &lines(["a", "b"]), &Opts::default(), &config(), gate.clone());
    assert_eq!((r.code, r.stats.waves, gate.2.lock().unwrap().len()), (0, 2, 3), "{}", r.stderr);
    assert!(!*gate.3.lock().unwrap(), "b's follow-up waited for a's slow request");
}

// A record's rounds follow from its answers, not from when they arrive. It
// asks `a` and a slow `b`, looks at `a`, asks `c`, looks at `b`, asks `d`
// and looks at `c`: round 1 is a and b, round 2 is c and d, however late
// `b`'s answer comes.
#[test]
fn rounds_do_not_depend_on_timing() {
    let program = r#"ask("a"; .) as $a | ask("SLOW"; "other") as $b | ($a > 0) as $_
        | ask("c"; .) as $c | ($b > 0) as $_ | ask("d"; .) as $d | ($c > 0) as $_ | [$a, $b, $c, $d] | length"#;
    for slow in [None, Some(Duration::from_millis(300))] {
        let fake = Arc::new(Fake { slow, ..Fake::default() });
        let r = run_with(program, "x\n", &Opts::default(), &config(), fake);
        assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "4\n", ""));
        assert_eq!(r.stats.wave_sizes, [2, 2], "slow: {slow:?}");
    }
}

// `input: "all"`: one record, `.` the array of every record. Questions
// built for every element before any is compared go out as one wave, a
// request for each element.
#[test]
fn all_one_wave() {
    let recs: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
    let p = r#"module {input: "all"}; [.[] | ask("q"; .)] | map(select(. > 0.5)) | length"#;
    let r = deem(p, &lines(&recs));
    let want = recs.iter().filter(|x| noul("q", &text(x)) > 0.5).count();
    assert_eq!((r.code, r.stdout.clone(), r.stderr.as_str()), (0, format!("{want}\n"), ""));
    assert_eq!((r.waves(), r.log.len(), r.stats.questions), (1, 200, 200), "a request per line");
    assert_eq!(r.stats.records, 1);
}

// Dependent rounds take a wave each, however many records there are.
#[test]
fn all_three_rounds() {
    let recs: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
    let p = r#"module {input: "all"};
        reduce range(3) as $r (.; [.[] | {l: ., s: ask("q\($r)"; .)}] | [.[] | select(.s > 0.3) | .l]) | length"#;
    let r = deem(p, &lines(&recs));
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    assert_eq!(r.waves(), 3);
}

// The gotcha, pinned: comparing inside `.[] |` waits for each answer
// before asking the next, a wave per record.
#[test]
fn all_compare_in_loop_is_a_wave_per_record() {
    let recs: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
    let r = deem(r#"module {input: "all"}; [.[] | select(ask("q"; .) > 0.5)] | length"#, &lines(&recs));
    assert_eq!((r.code, r.waves(), r.log.len()), (0, 10, 10));
}

// The whole input is one record: a failed answer fails it, exit 2, and
// --failed gets the whole input as read.
#[test]
fn all_fails_as_a_whole() {
    let dir = std::env::temp_dir().join(format!("deem-all-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let failed = dir.join("failed.txt");
    let opts = Opts { failed: Some(failed.to_str().unwrap().into()), ..Opts::default() };
    let p = r#"module {input: "all"}; [.[] | ask("FAIL?"; .)] | length"#;
    let r = deem_opts(p, "ok\nboom\n\nfine\n", &opts);
    assert_eq!((r.code, r.stdout.as_str(), r.waves()), (2, "", 1));
    assert!(r.stderr.starts_with("deem: record 1: "), "{}", r.stderr);
    assert_eq!(std::fs::read_to_string(&failed).unwrap(), "ok\nboom\nfine\n");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn all_input_errors() {
    let p = r#"module {input: "all"}; length"#;
    let r = deem_opts(p, "a\n", &Opts { slurp: true, ..Opts::default() });
    assert_eq!(
        (r.code, r.stderr.as_str()),
        (2, "deem: -s can't be used with an input \"all\" program: it already reads every record\n")
    );
    assert_eq!(deem(p, "").stdout, "0\n", "no records: an empty array");
    assert_eq!(deem_json(p, "{\"a\":1}\n{\"a\":2}\n").stdout, "2\n");
    let big = "x".repeat(1 << 20) + "\n";
    let r = deem(p, &big.repeat(65));
    assert_eq!((r.code, r.stdout.as_str()), (2, ""));
    assert_eq!(r.stderr, "deem: reading the input: the input is longer than 64 MiB, the most input \"all\" reads\n");
}
