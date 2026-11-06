//! The HTTP client and the sender pool against a local stub: retries,
//! 4xx/5xx, a rejected key, malformed replies, timeouts.

mod common;

use common::*;
use deem::RunOptions as Opts;
use deem::api::{ApiError, Http, Judge};
use serde_json::{Value as Json, json};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const OK: &str = r#"{"answers": {"q0": {"type": "noul", "noul": 0.25, "confidence": 0.9, "probabilities": {"true": 0.25, "false": 0.75}}}, "usage": {"input_tokens": 42}}"#;

fn http(s: &Stub) -> Arc<dyn Judge> {
    Arc::new(Http::new(&s.url, "sk-test", Duration::from_millis(500)))
}

fn run_against(s: &Stub, program: &str, input: &str) -> Run {
    run_with(program, input, &Opts::default(), &config(), http(s))
}

#[test]
fn request_and_answer() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let s = stub(move |_, line, body| {
        log.lock().unwrap().push((line.to_owned(), body.to_owned()));
        (200, OK.into())
    });
    let r = run_against(&s, r#"judge({q: "is it?"}; .)"#, "a line\n");
    assert_eq!(r.stderr, "");
    assert_eq!(r.stdout, "{\"value\":0.25,\"confidence\":0.9,\"probabilities\":{\"false\":0.75,\"true\":0.25}}\n");
    assert_eq!(r.stats.tokens, 42, "usage.input_tokens when the API reports it");
    let (line, body) = seen.lock().unwrap()[0].clone();
    assert_eq!(line, "POST /v1/systemone HTTP/1.1");
    let body: Json = serde_json::from_str(&body).unwrap();
    assert_eq!(
        body,
        json!({"model": "jev-latest", "state": "a line",
               "questions": {"q0": {"type": "noul", "instructions": "is it?"}}})
    );
}

#[test]
fn usage_under_either_name() {
    use deem::api::Response;
    let tokens =
        |usage: &str| Response::decode(&format!(r#"{{"answers": {{}}, "usage": {usage}}}"#)).unwrap().input_tokens;
    assert_eq!(tokens(r#"{"input_tokens": 7}"#), Some(7));
    assert_eq!(tokens(r#"{"prompt_tokens": 7}"#), Some(7), "as OpenAI-style servers name it");
    assert_eq!(tokens("{}"), None);
}

/// The request line and headers `Http` sends to `base` with `key`, seen
/// through a raw socket (the stub only sees the body).
fn head(base: &str, key: &str) -> Vec<String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}{base}", listener.local_addr().unwrap());
    let t = std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        let (conn, _) = listener.accept().unwrap();
        let mut r = BufReader::new(conn.try_clone().unwrap());
        let mut head = Vec::new();
        loop {
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            if l.trim().is_empty() {
                break;
            }
            head.push(l.trim().to_ascii_lowercase());
        }
        let mut w = conn;
        w.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{OK}", OK.len()).as_bytes()).unwrap();
        head
    });
    let _ = Http::new(&url, key, Duration::from_secs(5)).judge(&deem::api::Request {
        model: "m".into(),
        state: serde_json::value::RawValue::from_string("{}".into()).unwrap(),
        questions: Default::default(),
        assets: Default::default(),
        pictures: Default::default(),
    });
    t.join().unwrap()
}

#[test]
fn headers() {
    let h = head("/v1", "sk-test");
    assert_eq!(h[0], "post /v1/systemone http/1.1");
    assert!(h.contains(&"authorization: bearer sk-test".to_owned()), "{h:?}");
    assert!(h.contains(&"content-type: application/json".to_owned()), "{h:?}");
    let h = head("/jev/", "");
    assert_eq!(h[0], "post /jev/systemone http/1.1", "any base URL, trailing slash or not");
    assert!(!h.iter().any(|l| l.starts_with("authorization:")), "no key, no header: {h:?}");
}

#[test]
fn retries_what_backing_off_can_fix() {
    let s = stub(|n, _, _| if n == 0 { (429, "slow down".into()) } else { (200, OK.into()) });
    let r = run_against(&s, r#"ask("q"; .)"#, "x\n");
    assert_eq!((r.code, r.stdout.as_str()), (0, "0.25\n"));
    assert_eq!((s.hits.load(Ordering::SeqCst), r.stats.throttles), (2, 1));

    let s = stub(|_, _, _| (500, "down".into()));
    let r = run_against(&s, r#"ask("q"; .)"#, "x\n");
    assert_eq!(s.hits.load(Ordering::SeqCst), 6, "the first try and 5 retries");
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("the answer to \"q\" failed: api 500: down"), "{}", r.stderr);
}

/// The API refuses a request as a whole, and a request carries one record:
/// each record's own question decides whether it fails.
#[test]
fn client_errors_are_isolated_to_their_record() {
    let run = |s: &Stub, input: &str| run_with(r#"ask("q"; .)"#, input, &Opts::default(), &config(), http(s));
    let s = stub(|_, _, _| (422, r#"{"error": "bad question"}"#.into()));
    let r = run(&s, "x\ny\n");
    assert_eq!(s.hits.load(Ordering::SeqCst), 2, "a request for each record; none retried");
    assert_eq!(r.stats.questions, 2);
    assert!(r.stderr.starts_with("deem: record 1: the answer to \"q\" failed: api 422: {\"error\": \"bad question\"}"));
    assert!(
        r.stderr.ends_with("\ndeem: 2 of 2 records failed; --failed FILE keeps them to run again\n"),
        "{}",
        r.stderr
    );

    // one bad record among good ones: only it fails
    let s = stub(|_, _, body| {
        if body.contains("boom") {
            return (422, "bad question".into());
        }
        let v: Json = serde_json::from_str(body).unwrap();
        let answers: serde_json::Map<String, Json> = (v["questions"].as_object().unwrap().keys())
            .map(|k| (k.clone(), json!({"type": "noul", "noul": 0.5})))
            .collect();
        (200, json!({"answers": answers}).to_string())
    });
    let r = run(&s, "fine\nboom\nalso fine\n");
    assert_eq!((r.code, r.stdout.as_str()), (2, "0.5\n0.5\n"));
    assert_eq!(
        r.stderr,
        "deem: record 2: the answer to \"q\" failed: api 422: bad question\n\
         deem: 1 of 3 records failed; --failed FILE keeps them to run again\n"
    );
    assert_eq!(s.hits.load(Ordering::SeqCst), 3, "a request for each record");
}

/// A refused connection is retried like a 5xx.
#[test]
fn network_errors_are_retried() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let judge: Arc<dyn Judge> =
        Arc::new(Http::new(&format!("http://{closed}/v1"), "sk-test", Duration::from_millis(500)));
    let r = run_with(r#"ask("q"; .)"#, "x\n", &Opts::default(), &config(), judge);
    assert_eq!((r.code, r.stats.requests), (2, 6), "the first try and 5 retries");
    assert!(r.stderr.contains("request failed"), "{}", r.stderr);
}

/// Requests that keep failing after their retries mean the API is down: ten
/// in a row abort the run, once, and every record that didn't finish goes
/// to `--failed` (TRANSPORT.md, rule 6).
#[test]
fn an_outage_stops_the_run_once() {
    let s = stub(|_, _, _| (503, "unavailable".into()));
    let failed = std::env::temp_dir().join(format!("deem-outage-{}.txt", std::process::id()));
    let opts = Opts { failed: Some(failed.to_str().unwrap().into()), ..Opts::default() };
    let input = lines((0..30).map(|i| format!("line {i}")));
    let r = run_with(r#"ask("q"; .)"#, &input, &opts, &config(), http(&s));
    let rerun = std::fs::read_to_string(&failed).unwrap();
    std::fs::remove_file(&failed).unwrap();
    assert_eq!((r.code, r.stdout.as_str()), (2, ""));
    let stops: Vec<&str> = r.stderr.lines().filter(|l| l.contains("the API is unavailable")).collect();
    assert_eq!(
        stops,
        [
            "deem: the API is unavailable: 10 requests in a row failed after their retries (the last: api 503: unavailable)"
        ]
    );
    assert!(r.stderr.lines().filter(|l| l.starts_with("deem: record")).count() <= 5, "{}", r.stderr);
    assert!(r.stderr.contains("deem: stopped after reading 30 records; any after them weren't read\n"), "{}", r.stderr);
    // the summary counts every record in the file: those that failed, and
    // those the stop left unfinished
    let summary = r.stderr.lines().last().unwrap();
    let counts = summary
        .strip_prefix("deem: ")
        .and_then(|s| s.strip_suffix(&format!("; they are in {}", failed.display())))
        .and_then(|s| s.split_once(" of 30 records failed and "))
        .and_then(|(f, s)| Some((f.parse::<usize>().ok()?, s.strip_suffix(" didn't finish")?.parse::<usize>().ok()?)));
    assert!(counts.is_some_and(|(f, s)| f > 0 && f + s == 30), "{}", r.stderr);
    // the failures not listed are counted before the stop is told
    if let (Some(more), Some(stop)) = (r.stderr.find("deem: … and "), r.stderr.find("deem: the API is unavailable")) {
        assert!(more < stop, "{}", r.stderr);
    }
    let mut rerun: Vec<&str> = rerun.lines().collect();
    rerun.sort_by_key(|l| l[5..].parse::<u32>().unwrap());
    assert_eq!(rerun, input.lines().collect::<Vec<_>>(), "every record, as read, to run again");
}

/// What fails every request the same way but isn't recognised as a setup
/// error (here a refusal, or answers in another format) stops the run after
/// ten requests with nothing usable, instead of sending the whole input,
/// every record failing. Once it stops, the requests still queued are
/// dropped without waiting for a turn under the rate limit. At 20 requests
/// a second, so 24 senders and a window of 48: what is sent past the tenth
/// is at most the senders' worth, 34 in all (20 were sent, 2026-10-19).
/// (A wrong URL, key or model aborts at once: tests/transport.rs,
/// `setup_errors_abort_at_once`.)
#[test]
fn nothing_usable_stops_the_run() {
    let most = 10 + 24;
    for (status, body) in [
        (400, r#"{"detail":{"error_type":"api_usage_error","message":"Invalid request."}}"#),
        (422, "not like that"),
        (200, r#"{"answers": {}}"#),
        (200, r#"{"answers": {"q0": {"type": "noul", "noul": 7}}}"#),
    ] {
        let s = stub(move |_, _, _| (status, body.into()));
        let input = lines((0..2000).map(|i| format!("line {i}")));
        let t = std::time::Instant::now();
        let cfg = deem::Config { requests_per_second: 20, workers: 24, window: 48, ..config() };
        let r = run_with(r#"ask("q"; .)"#, &input, &Opts::default(), &cfg, http(&s));
        let hits = s.hits.load(Ordering::SeqCst);
        assert!(hits <= most, "{status} {body}: {hits} requests, not 2000 records' worth");
        assert_eq!((r.code, r.stdout.as_str()), (2, ""));
        let stops: Vec<&str> = r.stderr.lines().filter(|l| l.contains("no usable answer to 10 requests")).collect();
        assert_eq!(stops.len(), 1, "{}", r.stderr);
        if status == 422 {
            assert!(stops[0].ends_with("(the last: api 422: not like that)"), "{}", stops[0]);
        }
        assert!(r.stats.records < 2000, "no more records are read");
        // the queued ~450 records alone, at 20 a second, would take 20 s
        assert!(t.elapsed() < Duration::from_secs(10), "{status}: {:?}", t.elapsed());
    }
}

#[test]
fn a_rejected_key_stops_the_run() {
    let s = stub(|_, _, _| (401, "invalid key".into()));
    // more records than the window, so some are still unread when it stops
    let input = lines((0..1000).map(|i| format!("line {i}")));
    let r = run_against(&s, r#"ask("q"; .)"#, &input);
    assert_eq!(r.code, 2);
    assert_eq!(r.stdout, "");
    assert_eq!(r.stderr, "deem: the API refused the request; check JEV_API_KEY (invalid key)\n", "one message");
    assert!(r.stats.records < 1000, "no more records are read");
}

#[test]
fn malformed_replies() {
    for (body, want) in [
        ("not json", "decoding the API's response"),
        (r#"{"model": "m"}"#, "no `answers`"),
        (r#"{"answers": {}}"#, "the response had no answer for q0"),
        (r#"{"answers": {"q0": {"type": "noul", "noul": 7}}}"#, "the model answered 7, not a number in [0,1]"),
    ] {
        let s = stub(move |_, _, _| (200, body.into()));
        let r = run_against(&s, r#"ask("q"; .)"#, "x\n");
        assert_eq!(r.code, 2, "{body}");
        assert!(r.stderr.contains(want), "{body}: {}", r.stderr);
    }
    let s = stub(|_, _, _| (200, r#"{"answers": {"q0": {"type": "choice", "choice": "c"}}}"#.into()));
    let r = run_against(&s, r#"choose("q"; ["a", "b"]; .)"#, "x\n");
    assert!(r.stderr.contains(r#"the model answered "c", not one of ["a","b"]"#), "{}", r.stderr);
}

// A Score answer, in the API's shape: the value is `score`, `legend` (an echo
// of the levels) is left out, and a score off the scale fails.
#[test]
fn score_replies() {
    let reply = r#"{"answers": {"q0": {"type": "score", "score": 1.4, "legend": {"0": "calm", "1": "busy", "2": "on fire"},
                    "probabilities": {"0": 0.1, "1": 0.4, "2": 0.5}, "confidence": 0.6}}}"#;
    let s = stub(move |_, _, _| (200, reply.into()));
    let r = run_against(&s, r#"judge({q: "how busy?", levels: ["calm", "busy", "on fire"]}; .)"#, "x\n");
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    let want = r#"{"value":1.4,"confidence":0.6,"probabilities":{"0":0.1,"1":0.4,"2":0.5}}"#;
    assert_eq!(r.stdout, format!("{want}\n"));
    let s = stub(|_, _, _| (200, r#"{"answers": {"q0": {"type": "score", "score": 2.5}}}"#.into()));
    let r = run_against(&s, r#"rate("how busy?"; ["calm", "busy", "on fire"]; .)"#, "x\n");
    assert!(r.stderr.contains("the model answered 2.5, not a number in [0,2]"), "{}", r.stderr);
}

#[test]
fn timeouts() {
    let s = stub(|_, _, _| {
        std::thread::sleep(Duration::from_secs(2));
        (200, OK.into())
    });
    let r = run_against(&s, r#"ask("q"; .)"#, "x\n");
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("request failed:") && r.stderr.contains("timeout"), "{}", r.stderr);
    let e = Http::new(&s.url, "k", Duration::from_millis(100)).models().unwrap_err();
    assert!(matches!(e, ApiError::Transport(_)), "{e:?}");
}
