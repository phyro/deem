//! Shared by the integration tests: the fake model, and helpers that run
//! the whole pipeline in-process.
#![allow(dead_code)]

use deem::Config;
use deem::RunOptions as Opts;
use deem::api::{Answer, ApiError, Judge, Request, Response};
use deem::trace::Counts;
use serde_json::value::RawValue;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A fake model: deterministic answers and a log of every request.
///
/// A noul answer is a hash of (question, record) mapped to 0.01..=0.99; a
/// choice picks an option by the same hash. A question containing "FAIL"
/// gets an invalid answer for records containing "boom". `delay` lets a
/// test slow down requests whose questions contain "SLOW".
#[derive(Default)]
pub struct Fake {
    pub log: Mutex<Vec<Request>>,
    pub slow: Option<Duration>,
}

/// FNV-1a 64.
pub fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The fake's answer to `question` about `record` (the record as compact
/// JSON, as the request carries it).
pub fn noul(question: &str, record: &str) -> f64 {
    (fnv(&format!("{question}\u{0}{record}")) % 99 + 1) as f64 / 100.0
}

/// The values a request judges, as sent, in order of first appearance.
pub fn records(req: &Request) -> Vec<String> {
    let mut seen = Vec::new();
    for (r, _) in asked_raw(req) {
        if !seen.contains(&r) {
            seen.push(r);
        }
    }
    seen
}

/// A question as sent.
pub fn question_sent(req: &Request, name: &str) -> Json {
    serde_json::from_str(req.questions[name].get()).unwrap()
}

/// Every (record, question) a request asks, in question order.
pub fn asked(req: &Request) -> Vec<(Json, String)> {
    asked_raw(req).into_iter().map(|(r, q)| (serde_json::from_str(&r).unwrap(), q)).collect()
}

/// Every (record, question) a request asks, with the record as sent: in
/// the state, the questions as written.
pub fn asked_raw(req: &Request) -> Vec<(String, String)> {
    #[derive(serde::Deserialize)]
    struct Q {
        instructions: String,
    }
    let mut qs: Vec<_> = req.questions.iter().collect();
    qs.sort_by_key(|(k, _)| k[1..].parse::<usize>().unwrap());
    qs.into_iter()
        .map(|(_, q)| {
            let instr = serde_json::from_str::<Q>(q.get()).unwrap().instructions;
            (req.state.get().to_owned(), instr.lines().next().unwrap().to_owned())
        })
        .collect()
}

impl Judge for Fake {
    fn judge(&self, req: &Request) -> Result<Response, ApiError> {
        self.log.lock().unwrap().push(req.clone());
        refuse_bad_images(req)?;
        let mut answers = std::collections::BTreeMap::new();
        for (i, (record, question)) in asked(req).into_iter().enumerate() {
            if let Some(d) = self.slow.filter(|_| question.contains("SLOW")) {
                std::thread::sleep(d);
            }
            let record = record.to_string();
            let q = question_sent(req, &format!("q{i}"));
            let h = fnv(&format!("{question}\u{0}{record}"));
            let value = if question.contains("FAIL") && record.contains("boom") {
                Json::Null
            } else if let Some(opts) = q.get("criteria").filter(|_| q["type"] == "choice") {
                let opts: Vec<&String> = opts.as_object().unwrap().keys().collect();
                json!(opts[(h % opts.len() as u64) as usize])
            } else if q["type"] == "box" {
                // a box somewhere in the picture
                let (t, l) = ((h % 50) as f64 / 100.0, (h / 50 % 50) as f64 / 100.0);
                json!({"top": t, "bottom": t + 0.3, "left": l, "right": l + 0.4})
            } else if let Some(levels) = q.get("criteria").filter(|_| q["type"] == "score") {
                // a point on the scale, in tenths
                let tenths = (levels.as_array().unwrap().len() as u64 - 1) * 10 + 1;
                json!((h % tenths) as f64 / 10.0)
            } else {
                json!(noul(&question, &record))
            };
            answers.insert(format!("q{i}"), Answer { value, confidence: None, probabilities: None });
        }
        Ok(Response { answers, input_tokens: None, raw: None })
    }
}

/// The API's checks on pictures: every asset is shown, and nothing the
/// model would read is a reference deem forgot to replace.
pub fn refuse_bad_images(req: &Request) -> Result<(), ApiError> {
    let text: String = std::iter::once(req.state.get()).chain(req.questions.values().map(|q| q.get())).collect();
    let unshown = req.assets.keys().any(|id| !text.contains(id.as_str()));
    if unshown || text.contains("<image:") && !req.assets.is_empty() {
        return Err(ApiError::Status(400, r#"{"message": "Invalid image request."}"#.into()));
    }
    Ok(())
}

pub fn config() -> Config {
    let mut c = Config::from_env(|_| None).unwrap();
    c.key = "test".into();
    // not the hosted API's: a client made from this config reaches nothing
    c.base_url = NOWHERE.into();
    c.backoff = Duration::from_millis(1);
    c
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub stats: Counts,
    pub log: Vec<Request>,
}

impl Run {
    pub fn lines(&self) -> Vec<&str> {
        self.stdout.lines().collect()
    }

    pub fn waves(&self) -> usize {
        self.stats.waves as usize
    }

    /// Every (record, question) asked, request by request.
    pub fn asked(&self) -> Vec<Vec<(Json, String)>> {
        self.log.iter().map(asked).collect()
    }
}

pub fn run_with(program: &str, input: &str, opts: &Opts, cfg: &Config, judge: Arc<dyn Judge>) -> Run {
    run_args(program, &[], input, opts, cfg, judge)
}

pub fn run_args(program: &str, args: &[String], input: &str, opts: &Opts, cfg: &Config, judge: Arc<dyn Judge>) -> Run {
    let src: Box<dyn std::io::BufRead + Send> = Box::new(std::io::Cursor::new(input.as_bytes().to_vec()));
    run_input(program, args, src.into(), opts, cfg, judge)
}

/// Run `program` on any input: text, picture files, or pictures' paths.
pub fn run_input(
    program: &str,
    args: &[String],
    input: deem::Input,
    opts: &Opts,
    cfg: &Config,
    judge: Arc<dyn Judge>,
) -> Run {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let r = deem::run(("the program", program, args), opts, cfg, judge, input, &mut out, &mut err);
    Run {
        code: r.code,
        stdout: String::from_utf8(out).unwrap(),
        stderr: String::from_utf8(err).unwrap(),
        stats: r.stats,
        log: Vec::new(),
    }
}

/// Run `program` over `input` against a fresh fake, with default flags.
pub fn deem(program: &str, input: &str) -> Run {
    deem_opts(program, input, &Opts::default())
}

/// Run `program` over JSON records (`-J`) against a fresh fake.
pub fn deem_json(program: &str, input: &str) -> Run {
    deem_opts(program, input, &json())
}

/// The default flags with `-J`.
pub fn json() -> Opts {
    Opts { json_in: true, ..Opts::default() }
}

pub fn deem_opts(program: &str, input: &str, opts: &Opts) -> Run {
    deem_args(program, &[], input, opts)
}

/// Run a program with arguments against a fresh fake.
pub fn deem_args(program: &str, args: &[&str], input: &str, opts: &Opts) -> Run {
    let fake = Arc::new(Fake::default());
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let mut r = run_args(program, &args, input, opts, &config(), fake.clone());
    r.log = fake.log.lock().unwrap().clone();
    r
}

/// The first bytes of a PNG this size: enough for deem, which reads only
/// the header (the fake never decodes a picture).
pub fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    b.extend(w.to_be_bytes());
    b.extend(h.to_be_bytes());
    b.extend([8, 6, 0, 0, 0]);
    b
}

/// The first bytes of a baseline JPEG this size.
pub fn jpeg(w: u16, h: u16) -> Vec<u8> {
    let mut b = vec![0xff, 0xd8, 0xff, 0xc0, 0, 11, 8];
    b.extend(h.to_be_bytes());
    b.extend(w.to_be_bytes());
    b.extend([1, 1, 0x11, 0]);
    b
}

/// A fresh directory for a test's files.
pub fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("deem-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Output as the docs show it: a line that starts with exactly two spaces
/// continues the line before it, so a long line can be wrapped for the page.
/// Applied to both sides of a comparison, it changes nothing else: carets
/// (four spaces) stay, and deem's own two-space lines join on both sides.
pub fn unwrap_lines(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for l in s.lines() {
        let cont = l.starts_with("  ") && !l[2..].starts_with(' ') && l.len() > 2;
        match out.last_mut() {
            Some(last) if cont => *last = format!("{} {}", last.trim_end(), l.trim_start()),
            _ => out.push(l.to_owned()),
        }
    }
    out.join("\n")
}

/// Output as the docs show it, from Linux, where they were run: a refused
/// connection is `os error 111` there, and on macOS `os error 61`; and
/// macOS's `wc -l` pads its count with spaces (`       2`).
pub fn as_on_linux(s: &str) -> String {
    let refused = if cfg!(target_os = "macos") { 61 } else { 111 };
    let s = s.replace(&std::io::Error::from_raw_os_error(refused).to_string(), "Connection refused (os error 111)");
    if !cfg!(target_os = "macos") {
        return s;
    }
    let count = |l: &str| !l.trim().is_empty() && l.trim().chars().all(|c| c.is_ascii_digit());
    s.split_inclusive('\n').map(|l| if count(l) { l.trim_start() } else { l }).collect()
}

/// Lines of text, one per record.
pub fn lines<S: AsRef<str>>(xs: impl IntoIterator<Item = S>) -> String {
    xs.into_iter().map(|x| format!("{}\n", x.as_ref())).collect()
}

/// The 48 text records the prototype used.
pub fn records48() -> Vec<String> {
    std::fs::read_to_string("tests/golden/records48.txt").unwrap().lines().map(String::from).collect()
}

/// A string as the fake sees it in a request.
pub fn text(s: &str) -> String {
    Json::String(s.to_owned()).to_string()
}

/// A tiny HTTP server on localhost. `respond` gets the request number
/// (from 0), the request line and the body, and returns a status and body.
pub struct Stub {
    pub url: String,
    pub hits: Arc<std::sync::atomic::AtomicUsize>,
    /// each request's `authorization` header, in the order they came
    pub auth: Arc<Mutex<Vec<Option<String>>>>,
}

pub fn stub(respond: impl Fn(usize, &str, &str) -> (u16, String) + Send + Sync + 'static) -> Stub {
    stub_headers(move |n, line, body| {
        let (status, out) = respond(n, line, body);
        (status, Vec::new(), out)
    })
}

/// A stub whose responses carry headers too (`Location`, `Retry-After`).
#[allow(clippy::type_complexity)]
pub fn stub_headers(
    respond: impl Fn(usize, &str, &str) -> (u16, Vec<(&'static str, String)>, String) + Send + Sync + 'static,
) -> Stub {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let auth = Arc::new(Mutex::new(Vec::new()));
    let (respond, count, seen) = (Arc::new(respond), hits.clone(), auth.clone());
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { return };
            let (respond, count, seen) = (respond.clone(), count.clone(), seen.clone());
            std::thread::spawn(move || {
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut w = conn;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let (mut len, mut chunked, mut key) = (0, false, None);
                    loop {
                        let mut h = String::new();
                        r.read_line(&mut h).unwrap();
                        if h.trim().is_empty() {
                            break;
                        }
                        if h.to_ascii_lowercase().starts_with("authorization:") {
                            key = Some(h["authorization:".len()..].trim().to_owned());
                        }
                        let h = h.to_ascii_lowercase();
                        if let Some(v) = h.strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap();
                        }
                        chunked |= h.starts_with("transfer-encoding:") && h.contains("chunked");
                    }
                    let mut body = vec![0; len];
                    r.read_exact(&mut body).unwrap();
                    // a streamed body (a picture's file part) comes in chunks
                    while chunked {
                        let mut size = String::new();
                        r.read_line(&mut size).unwrap();
                        let n = usize::from_str_radix(size.trim(), 16).unwrap();
                        let mut chunk = vec![0; n + 2];
                        r.read_exact(&mut chunk).unwrap();
                        body.extend(&chunk[..n]);
                        chunked = n > 0;
                    }
                    seen.lock().unwrap().push(key);
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    let (status, headers, out) = respond(n, line.trim(), &String::from_utf8_lossy(&body));
                    let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nContent-Type: application/json\r\n{extra}\r\n",
                        out.len()
                    );
                    if w.write_all(head.as_bytes()).and_then(|_| w.write_all(out.as_bytes())).is_err() {
                        return;
                    }
                }
            });
        }
    });
    Stub { url, hits, auth }
}

/// The fake model behind HTTP, speaking the API's wire format.
pub fn fake_api() -> Stub {
    api(Fake::default())
}

/// A model behind HTTP, speaking the API's wire format.
pub fn api(fake: impl Judge + 'static) -> Stub {
    stub(move |_, _, body| {
        #[derive(serde::Deserialize)]
        struct Wire {
            model: String,
            state: Box<RawValue>,
            questions: BTreeMap<String, Box<RawValue>>,
            #[serde(default)]
            assets: BTreeMap<String, Json>,
        }
        let v: Wire = serde_json::from_str(request_part(body)).unwrap();
        let asset = |id: &String| deem::api::Asset { kind: "image", part: id.clone(), path: String::new(), mime: "" };
        let req = Request {
            model: v.model,
            state: v.state,
            questions: v.questions,
            assets: v.assets.keys().map(|id| (id.clone(), asset(id))).collect(),
            pictures: Default::default(),
        };
        let resp = fake.judge(&req).unwrap();
        let answers: serde_json::Map<String, Json> = resp
            .answers
            .iter()
            .map(|(k, a)| {
                let kind = question_sent(&req, k)["type"].as_str().unwrap().to_owned();
                let (confidence, probabilities) = (a.confidence, &a.probabilities);
                (
                    k.clone(),
                    json!({"type": kind, &kind: a.value, "confidence": confidence, "probabilities": probabilities}),
                )
            })
            .collect();
        (200, json!({"model": req.model, "answers": answers, "usage": {"input_tokens": 10}}).to_string())
    })
}

/// The request in a body: the body, or in a multipart one, its part
/// `request`.
fn request_part(body: &str) -> &str {
    let Some(boundary) = body.strip_prefix("--").and_then(|b| b.lines().next()) else { return body };
    let start = body.find("name=\"request\"").expect("a multipart request has its part `request`");
    let part = &body[start..];
    let json = &part[part.find("\r\n\r\n").unwrap() + 4..];
    &json[..json.find(&format!("\r\n--{boundary}")).unwrap()]
}

/// Where nothing listens: the discard port, on this machine.
pub const NOWHERE: &str = "http://127.0.0.1:9";

/// What every deem a test starts has in its environment, so that it can
/// reach this machine and nothing else: a request to any other host, the
/// hosted API's above all, goes to a proxy at `NOWHERE` and is refused
/// there. A test that forgot its stub's URL then fails, where it would have
/// asked the real API. Only tests/live start deem without it
/// (`no_test_reaches_another_machine` checks that it holds).
pub const OFFLINE: [(&str, &str); 4] =
    [("ALL_PROXY", NOWHERE), ("HTTPS_PROXY", NOWHERE), ("HTTP_PROXY", NOWHERE), ("NO_PROXY", "127.0.0.1,localhost")];

/// Run the deem binary in `dir`, with `env` on top of a clean environment
/// that can't reach another machine (`OFFLINE`).
pub fn bin(args: &[&str], stdin: &str, env: &[(&str, &str)], dir: &str) -> Run {
    let env: Vec<(&str, &str)> = OFFLINE.iter().chain(env).copied().collect();
    bin_online(args, stdin, &env, dir)
}

/// `bin`, with the network as it is: for tests/live.
pub fn bin_online(args: &[&str], stdin: &str, env: &[(&str, &str)], dir: &str) -> Run {
    use std::io::Write;
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_deem"));
    cmd.args(args).current_dir(dir).env_clear().env("PATH", std::env::var("PATH").unwrap());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    let o = child.wait_with_output().unwrap();
    Run {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        stats: Counts::default(),
        log: Vec::new(),
    }
}
