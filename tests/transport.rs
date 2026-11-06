//! TRANSPORT.md's rules, against a local stub of the API. Each test names
//! the rule it checks; the rules' other tests are in tests/http.rs and the
//! unit tests in src/transport/ and src/trace/.

mod common;

use common::*;
use deem::RunOptions as Opts;
use deem::api::{Http, Judge};
use serde_json::{Value as Json, json};
use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

fn http(url: &str, timeout: Duration) -> Arc<dyn Judge> {
    Arc::new(Http::new(url, "sk-test", timeout))
}

/// A reply answering every question of `body` with `score`, as the API
/// words it, reporting `tokens` input tokens.
fn answer(body: &str, score: f64, tokens: u64) -> String {
    let v: Json = serde_json::from_str(body).unwrap();
    let answers: serde_json::Map<String, Json> = (v["questions"].as_object().unwrap().keys())
        .map(|k| (k.clone(), json!({"type": "noul", "noul": score})))
        .collect();
    json!({"model": "jev-1.13.0", "answers": answers, "usage": {"input_tokens": tokens, "output_tokens": 20}})
        .to_string()
}

/// The record a request asks about: its state.
fn record_asked(body: &str) -> String {
    serde_json::from_str::<Json>(body).unwrap()["state"].to_string()
}

/// The test config at `rps` requests a second, as `JEV_REQUESTS_PER_SECOND`
/// sets it: with the senders and window that follow from it.
fn rate(rps: u64) -> deem::Config {
    let set = deem::Config::from_env(|k| (k == "JEV_REQUESTS_PER_SECOND").then(|| rps.to_string())).unwrap();
    deem::Config { requests_per_second: rps, workers: set.workers, window: set.window, ..config() }
}

fn numbered(n: usize, prefix: &str) -> String {
    lines((0..n).map(|i| format!("{prefix} {i}")))
}

/// Run on another thread; `None` if it hasn't ended within `limit`.
fn within(limit: Duration, f: impl FnOnce() -> Run + Send + 'static) -> Option<Run> {
    let (tx, rx) = channel();
    std::thread::spawn(move || drop(tx.send(f())));
    rx.recv_timeout(limit).ok()
}

/// Response headers a stub sends.
type Headers = &'static [(&'static str, &'static str)];

// Rule 6: what fails every request the same way aborts on the first reply,
// with one message naming the setting to check. 30 records are 2 requests,
// sent together: neither is retried or resent one record per request.
#[test]
fn setup_errors_abort_at_once() {
    let cases: [(u16, Headers, &str, &str); 6] = [
        (301, &[("Location", "https://api.example/v1/systemone")], "", "JEV_BASE_URL"),
        (404, &[], r#"{"detail":"Not Found"}"#, "JEV_BASE_URL"),
        (405, &[], r#"{"detail":"Method Not Allowed"}"#, "JEV_BASE_URL"),
        (400, &[], r#"{"detail":{"error_type":"api_usage_error","message":"Unknown model: nope"}}"#, "JEV_MODEL"),
        (
            401,
            &[],
            r#"{"detail":{"error_type":"authentication_error","message":"Cannot authenticate"}}"#,
            "JEV_API_KEY",
        ),
        (
            403,
            &[],
            r#"{"detail":{"error_type":"authentication_error","message":"Must supply an API key!"}}"#,
            "JEV_API_KEY",
        ),
    ];
    for (status, headers, body, setting) in cases {
        let headers: Vec<(&'static str, String)> = headers.iter().map(|(k, v)| (*k, v.to_string())).collect();
        let s = stub_headers(move |_, _, _| (status, headers.clone(), body.into()));
        let r = run_with(
            r#"ask("q"; .)"#,
            &numbered(300, "line"),
            &Opts::default(),
            &config(),
            http(&s.url, Duration::from_secs(5)),
        );
        let hits = s.hits.load(Ordering::SeqCst);
        assert_eq!((r.code, r.stdout.as_str()), (2, ""), "{status}");
        let senders = config().workers;
        assert!(hits <= senders, "{status}: {hits} requests; at most those the {senders} senders had sent");
        assert_eq!(r.stderr.lines().count(), 1, "{status}: one message\n{}", r.stderr);
        assert!(r.stderr.contains(setting), "{status}: names {setting}: {}", r.stderr);
        if status == 301 {
            assert!(r.stderr.contains("https://api.example/v1"), "says where it redirects: {}", r.stderr);
        }
    }
    // a URL deem can't use at all: no attempt is retried
    let t = Instant::now();
    let r = run_with(
        r#"ask("q"; .)"#,
        "x\n",
        &Opts::default(),
        &config(),
        http("ftp://127.0.0.1:9/v1", Duration::from_secs(5)),
    );
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("JEV_BASE_URL"), "{}", r.stderr);
    assert!(
        r.stats.requests <= 1 && t.elapsed() < Duration::from_secs(2),
        "{:?}, {} attempts",
        t.elapsed(),
        r.stats.requests
    );
}

/// The API at most `rps` requests a second (429 above that, as the real
/// one does), each taking `latency`.
fn limited(rps: f64, latency: Duration) -> Stub {
    let bucket = Mutex::new((rps, Instant::now()));
    stub(move |_, _, body| {
        {
            let mut b = bucket.lock().unwrap();
            let now = Instant::now();
            b.0 = (b.0 + now.duration_since(b.1).as_secs_f64() * rps).min(rps);
            b.1 = now;
            if b.0 < 1.0 {
                return (429, r#"{"detail":"Too Many Requests"}"#.into());
            }
            b.0 -= 1.0;
        }
        std::thread::sleep(latency);
        (200, answer(body, 0.5, 300))
    })
}

// Rule 8: two invocations on one key share its rate. Each slows down on the
// 429s instead of failing: both finish, with every record. The limit is
// half the rate each would take alone (20, set as `JEV_REQUESTS_PER_SECOND`
// would), so they must share it. 120 records each are 240 requests at 10 a
// second: long enough to compete throughout.
#[test]
fn two_invocations_share_the_rate() {
    let s = limited(10.0, Duration::from_millis(150));
    let runs: Vec<_> = (0..2)
        .map(|k| {
            let url = s.url.clone();
            std::thread::spawn(move || {
                let cfg = deem::Config { backoff: Duration::from_millis(250), ..rate(20) };
                run_with(
                    r#"ask("q"; .)"#,
                    &numbered(120, &format!("run {k} line")),
                    &Opts::default(),
                    &cfg,
                    http(&url, Duration::from_secs(5)),
                )
            })
        })
        .collect();
    let mut throttled = 0;
    for r in runs {
        let r = r.join().unwrap();
        assert_eq!((r.code, r.lines().len(), r.stderr.lines().next()), (0, 120, None), "{}", r.stderr);
        throttled += r.stats.throttles;
    }
    assert!(throttled > 0, "the two did compete for the rate");
}

// Rule 8: the rate is a setting, 80 requests and 100,000 tokens a second
// unless set, and what follows from it is bounded: senders enough for 1.2 s
// a request, at most 256, and two requests' worth of records for each.
#[test]
fn the_rate_is_a_setting() {
    let cfg = |rps: Option<&str>, tps: Option<&str>, parallel: Option<&str>| {
        deem::Config::from_env(|k| match k {
            "JEV_REQUESTS_PER_SECOND" => rps.map(String::from),
            "JEV_TOKENS_PER_SECOND" => tps.map(String::from),
            "JEV_PARALLEL" => parallel.map(String::from),
            _ => None,
        })
    };
    let got = |c: deem::Config| (c.requests_per_second, c.tokens_per_second, c.workers, c.window);
    assert_eq!(got(cfg(None, None, None).unwrap()), (80, 100_000, 96, 192));
    assert_eq!(got(cfg(Some("10"), Some("5000"), None).unwrap()), (10, 5_000, 12, 24));
    assert_eq!(got(cfg(Some("1000"), None, None).unwrap()), (1_000, 100_000, 256, 512));
    // --parallel caps the senders, and never adds to them
    assert_eq!(got(cfg(None, None, Some("4")).unwrap()), (80, 100_000, 4, 8));
    assert_eq!(got(cfg(Some("10"), None, Some("100")).unwrap()), (10, 100_000, 12, 24));
    for (rps, tps, parallel, e) in [
        (Some("0"), None, None, "JEV_REQUESTS_PER_SECOND: 0 is not from 1 to 1000"),
        (Some("1001"), None, None, "JEV_REQUESTS_PER_SECOND: 1001 is not from 1 to 1000"),
        (Some("fast"), None, None, "JEV_REQUESTS_PER_SECOND: \"fast\" is not a whole number"),
        (None, Some("0"), None, "JEV_TOKENS_PER_SECOND: 0 would send nothing; give at least 1"),
        (None, None, Some("0"), "JEV_PARALLEL: 0 is not from 1 to 256"),
        (None, None, Some("257"), "JEV_PARALLEL: 257 is not from 1 to 256"),
    ] {
        assert_eq!(cfg(rps, tps, parallel).err().as_deref(), Some(e));
    }
}

// Rule 8: with --parallel, deem has at most that many requests at the API
// at once, and holds the rest itself: none waits in the server's queue, so
// none times out there and is sent again. 40 records against a server that
// takes 100 ms a request and counts how many it holds at once: never more
// than 3, and every request sent once.
#[test]
fn parallel_caps_requests_at_once() {
    use std::sync::atomic::AtomicUsize;
    let (held, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (h, m) = (held.clone(), most.clone());
    let s = stub(move |_, _, body| {
        m.fetch_max(h.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        h.fetch_sub(1, Ordering::SeqCst);
        (200, answer(body, 0.5, 300))
    });
    let set = deem::Config::from_env(|k| (k == "JEV_PARALLEL").then(|| "3".into())).unwrap();
    let cfg = deem::Config { workers: set.workers, window: set.window, ..config() };
    let r =
        run_with(r#"ask("q"; .)"#, &numbered(40, "line"), &Opts::default(), &cfg, http(&s.url, Duration::from_secs(5)));
    assert_eq!((r.code, r.stats.requests, r.stats.throttles), (0, 40, 0), "{}", r.stderr);
    assert_eq!((most.load(Ordering::SeqCst), s.hits.load(Ordering::SeqCst)), (3, 40));
}

// Rule 8: deem keeps to the rate it is given: 40 requests at 10 a second
// are a second's burst of 10, then 30 more at 10 a second.
#[test]
fn keeps_to_the_rate() {
    let s = limited(1000.0, Duration::ZERO);
    let cfg = deem::Config { requests_per_second: 10, ..config() };
    let t = Instant::now();
    let r =
        run_with(r#"ask("q"; .)"#, &numbered(40, "line"), &Opts::default(), &cfg, http(&s.url, Duration::from_secs(5)));
    let took = t.elapsed();
    assert_eq!((r.code, r.stats.requests, r.stats.throttles), (0, 40, 0), "{}", r.stderr);
    assert!(took >= Duration::from_millis(2_800) && took < Duration::from_secs(6), "{took:?}");
}

// Rule 9: a request's tokens are estimated from the bytes per token measured
// on earlier answers. Here the API reports 4 bytes a token on the first
// request, and nothing on the second, which is then counted at the measured
// 4, not the starting 2.
#[test]
fn tokens_are_estimated_as_measured() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = bodies.clone();
    let s = stub(move |n, _, body| {
        seen.lock().unwrap().push(body.len() as u64);
        match n {
            0 => (200, answer(body, 0.5, body.len() as u64 / 4)),
            _ => (200, json!({"answers": {"q0": {"type": "noul", "noul": 0.5}}}).to_string()),
        }
    });
    let line = "sshd[2211]: Failed password for root from 10.0.0.7 port 22 ssh2 ".repeat(300);
    let program = r#"ask("q1"; .) as $a | ask("q2 \($a)"; .)"#;
    let r = run_with(program, &format!("{line}\n"), &Opts::default(), &config(), http(&s.url, Duration::from_secs(5)));
    assert_eq!((r.code, r.stats.requests), (0, 2), "{}", r.stderr);
    let b = bodies.lock().unwrap().clone();
    assert!(b[0] >= 16 << 10, "large enough to measure: {b:?}");
    let measured = b[0] / 4 + b[1] / 4;
    assert!(r.stats.tokens.abs_diff(measured) <= 2, "{} tokens, not {measured}", r.stats.tokens);
}

// Rule 4: every kind of failure ends, with every question answered or
// failed, well within its limits (here shortened: 1 ms backoff, 500 ms
// timeout, 1 s of "slow down" without a success).
#[test]
fn every_failure_ends() {
    type Respond = Box<dyn Fn(usize, &str, &str) -> (u16, String) + Send + Sync>;
    let kinds: Vec<(&str, Option<Respond>)> = vec![
        ("slow down", Some(Box::new(|_, _, _| (429, "{}".into())))),
        ("overloaded", Some(Box::new(|_, _, _| (529, "{}".into())))),
        ("failing", Some(Box::new(|_, _, _| (503, "{}".into())))),
        (
            "no reply",
            Some(Box::new(|_, _, _| {
                std::thread::sleep(Duration::from_secs(2));
                (200, "{}".into())
            })),
        ),
        ("broken reply", Some(Box::new(|_, _, _| (200, "not json".into())))),
        ("missing answer", Some(Box::new(|_, _, _| (200, r#"{"answers": {}}"#.into())))),
        ("invalid answer", Some(Box::new(|_, _, b| (200, answer(b, 7.0, 300))))),
        (
            "refused",
            Some(Box::new(|_, _, _| {
                (400, r#"{"detail":{"error_type":"api_usage_error","message":"Invalid request."}}"#.into())
            })),
        ),
        ("unreachable", None),
    ];
    let runs: Vec<_> = kinds
        .into_iter()
        .map(|(kind, respond)| {
            let url = match respond {
                Some(f) => stub(f).url,
                None => {
                    format!("http://{}/v1", std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap())
                }
            };
            let run = within(Duration::from_secs(60), move || {
                let cfg = deem::Config { busy_for: Duration::from_secs(1), ..config() };
                run_with(
                    r#"ask("q"; .)"#,
                    &numbered(50, "line"),
                    &Opts::default(),
                    &cfg,
                    http(&url, Duration::from_millis(500)),
                )
            });
            (kind, run)
        })
        .collect();
    for (kind, r) in runs {
        let r = r.unwrap_or_else(|| panic!("{kind}: still running after 60 s"));
        assert_eq!(r.code, 2, "{kind}: {}", r.stderr);
    }
}

// Rule 1: attempts ≤ 20 × questions, whatever the API does. The stub
// answers each attempt with one of every kind of reply, in a fixed
// pseudo-random order.
#[test]
fn attempts_are_bounded() {
    let s = stub(|n, _, body| match fnv(&n.to_string()) % 9 {
        0 => (429, "{}".into()),
        1 => (529, "{}".into()),
        2 => (503, "{}".into()),
        3 => (500, "{}".into()),
        4 => (400, r#"{"detail":{"error_type":"api_usage_error","message":"Invalid request."}}"#.into()),
        5 => (400, r#"{"detail":{"error_type":"max_tokens_exceeded"}}"#.into()),
        6 => (200, "not json".into()),
        7 => (200, r#"{"answers": {}}"#.into()),
        _ => (200, answer(body, 0.5, 300)),
    });
    let cfg = deem::Config { busy_for: Duration::from_secs(1), ..config() };
    let questions = 2 * 48;
    let r = run_with(
        r#"[ask("a"; .), ask("b"; .)]"#,
        &numbered(48, "line"),
        &Opts::default(),
        &cfg,
        // long enough that no attempt times out: one that did could reach the
        // stub after deem gave up on it, or be given up before it arrived
        http(&s.url, Duration::from_secs(5)),
    );
    let attempts = s.hits.load(Ordering::SeqCst);
    assert!(attempts <= 20 * questions, "{attempts} attempts for {questions} questions");
    assert_eq!(attempts as u64, r.stats.requests, "--stats counts every attempt\n{}", r.stderr);
}

// Rule 5: an abort ends a sender's wait for `Retry-After` too. "wait" is
// told to come back in 30 s; "bad" is a rejected key, which aborts.
#[test]
fn an_abort_ends_a_retry_after_wait() {
    let s = stub_headers(|_, _, body| {
        if body.contains("wait") {
            (429, vec![("Retry-After", "30".into())], "{}".into())
        } else {
            std::thread::sleep(Duration::from_millis(300));
            (401, Vec::new(), r#"{"detail":{"error_type":"authentication_error"}}"#.into())
        }
    });
    let t = Instant::now();
    let r =
        run_with(r#"ask("q"; .)"#, "wait\nbad\n", &Opts::default(), &config(), http(&s.url, Duration::from_secs(5)));
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("JEV_API_KEY"), "{}", r.stderr);
    assert!(t.elapsed() < Duration::from_secs(5), "waited {:?} for a Retry-After after the abort", t.elapsed());
}

// Rule 2: while every request hangs, deem reads a window of records and no
// more.
#[test]
fn the_reader_stops_at_the_window() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let seen = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let (g, sn) = (gate.clone(), seen.clone());
    let s = stub(move |_, _, body| {
        sn.lock().unwrap().insert(record_asked(body));
        let (open, cv) = &*g;
        let _unused = cv.wait_while(open.lock().unwrap(), |open| !*open).unwrap();
        (200, answer(body, 0.5, 300))
    });
    let url = s.url.clone();
    let run = std::thread::spawn(move || {
        // A window of 20 records, fewer than the senders, so each reaches
        // the stub.
        let cfg = deem::Config { window: 20, ..config() };
        run_with(r#"ask("q"; .)"#, &numbered(1000, "line"), &Opts::default(), &cfg, http(&url, Duration::from_secs(30)))
    });
    // however slow the machine: the window fills, and then nothing more comes
    let start = Instant::now();
    while seen.lock().unwrap().len() < 20 && start.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(seen.lock().unwrap().len(), 20, "a window of records, and no more");
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let r = run.join().unwrap();
    assert_eq!((r.code, r.lines().len()), (0, 1000));
}

/// Answers 0.5, except that a question about a record containing "boom"
/// gets an answer that doesn't fit (7).
fn boom_fails() -> Stub {
    stub(|_, _, body| {
        let noul = if record_asked(body).contains("boom") { 7.0 } else { 0.5 };
        let v: Json = serde_json::from_str(body).unwrap();
        let answers: serde_json::Map<String, Json> = (v["questions"].as_object().unwrap().keys())
            .map(|q| (q.clone(), json!({"type": "noul", "noul": noul})))
            .collect();
        (200, json!({"answers": answers, "usage": {"input_tokens": 300}}).to_string())
    })
}

// Rule 11: records missing from the output are counted at exit. The first
// five failures are shown, then how many more.
#[test]
fn missing_records_are_counted() {
    let s = boom_fails();
    let input = lines((0..100).map(|i| if i % 14 == 3 { format!("boom {i}") } else { format!("line {i}") }));
    let r = run_with(r#"ask("q"; .)"#, &input, &Opts::default(), &config(), http(&s.url, Duration::from_secs(5)));
    assert_eq!((r.code, r.lines().len()), (2, 93));
    let shown = r.stderr.lines().filter(|l| l.starts_with("deem: record ")).count();
    assert_eq!(shown, 5, "{}", r.stderr);
    assert!(r.stderr.contains("deem: … and 2 more\n"), "{}", r.stderr);
    assert!(
        r.stderr.ends_with("deem: 7 of 100 records failed; --failed FILE keeps them to run again\n"),
        "{}",
        r.stderr
    );

    let failed = std::env::temp_dir().join(format!("deem-missing-{}.txt", std::process::id()));
    let opts = Opts { failed: Some(failed.to_str().unwrap().into()), ..Opts::default() };
    let r = run_with(r#"ask("q"; .)"#, &input, &opts, &config(), http(&s.url, Duration::from_secs(5)));
    let kept = std::fs::read_to_string(&failed).unwrap();
    std::fs::remove_file(&failed).unwrap();
    assert_eq!(kept.lines().count(), 7);
    assert!(
        r.stderr.ends_with(&format!("deem: 7 of 100 records failed; they are in {}\n", failed.display())),
        "{}",
        r.stderr
    );
}

// Rule 11: when retries hold the invocation up (here past a shortened
// 300 ms), deem says why, once, and then carries on.
#[test]
fn long_delays_are_announced() {
    for (status, says) in [(503, "api 503"), (429, "429")] {
        // busy for 1.5 s from the first request, however late that comes
        let first = std::sync::OnceLock::new();
        let s = stub(move |_, _, body| {
            if first.get_or_init(Instant::now).elapsed() < Duration::from_millis(1500) {
                return (status, r#"{"detail":"busy"}"#.into());
            }
            (200, answer(body, 0.5, 300))
        });
        let cfg =
            deem::Config { backoff: Duration::from_millis(100), notice_after: Duration::from_millis(300), ..config() };
        let r = run_with(
            r#"ask("q"; .)"#,
            &numbered(10, "line"),
            &Opts::default(),
            &cfg,
            http(&s.url, Duration::from_secs(5)),
        );
        assert_eq!((r.code, r.lines().len()), (0, 10), "{}", r.stderr);
        let notes: Vec<&str> = r.stderr.lines().collect();
        assert_eq!(notes.len(), 1, "{status}: said once\n{}", r.stderr);
        assert!(notes[0].contains(says) && notes[0].contains("retrying"), "{status}: {}", notes[0]);
    }
}

/// Lines arriving one at a time, `every` apart, like `tail -f`.
struct Trickle {
    lines: std::collections::VecDeque<String>,
    every: Duration,
    line: Vec<u8>,
    at: usize,
    /// when the last line was read
    ended: Arc<Mutex<Option<Instant>>>,
}

impl Trickle {
    /// The input, noting in `ended` when it ends.
    fn timed(n: usize, every: Duration, ended: Arc<Mutex<Option<Instant>>>) -> Box<dyn std::io::BufRead + Send> {
        let lines = (0..n).map(|i| format!("line {i}\n")).collect();
        Box::new(std::io::BufReader::new(Trickle { lines, every, line: Vec::new(), at: 0, ended }))
    }
}

impl std::io::Read for Trickle {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at == self.line.len() {
            let Some(next) = self.lines.pop_front() else {
                self.ended.lock().unwrap().get_or_insert_with(Instant::now);
                return Ok(0);
            };
            std::thread::sleep(self.every);
            (self.line, self.at) = (next.into_bytes(), 0);
        }
        let n = buf.len().min(self.line.len() - self.at);
        buf[..n].copy_from_slice(&self.line[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Output that notes when its first byte arrives.
struct FirstWrite(Arc<Mutex<Option<Instant>>>);

impl std::io::Write for FirstWrite {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().get_or_insert_with(Instant::now);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Rule 4: a steady stream doesn't hold its questions back. At 30 lines a
// second the input is never quiet; a record's question goes when the record
// waits for it, however much more input is coming.
#[test]
fn a_stream_is_not_held_back() {
    let s = stub(|_, _, body| (200, answer(body, 0.5, 300)));
    let first = Arc::new(Mutex::new(None));
    let ended = Arc::new(Mutex::new(None));
    let input = Trickle::timed(90, Duration::from_millis(33), ended.clone());
    let program = ("the program", r#"ask("q"; .)"#, &[][..]);
    let r = deem::run(
        program,
        &Opts::default(),
        &config(),
        http(&s.url, Duration::from_secs(5)),
        input,
        FirstWrite(first.clone()),
        &mut Vec::new(),
    );
    assert_eq!(r.code, 0);
    // held back, it would come when the input ends; the input slows with a
    // busy machine as much as deem does, so this holds on any
    let (first, ended) = (first.lock().unwrap().expect("some output"), ended.lock().unwrap().expect("the end"));
    assert!(first < ended, "the first output came {:?} after the input ended", first - ended);
}
