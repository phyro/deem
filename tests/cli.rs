//! The binary: flags, shapes, exit codes, the byte limit, `-e`.

mod common;

use common::*;
use std::sync::atomic::Ordering;

fn env(s: &Stub) -> Vec<(&'static str, String)> {
    vec![("JEV_API_KEY", "sk".into()), ("JEV_BASE_URL", s.url.clone())]
}

/// `deem run ARGS…` against the stub.
fn deem_bin(s: &Stub, args: &[&str], stdin: &str) -> Run {
    let args: Vec<&str> = ["run"].iter().chain(args).copied().collect();
    deem_raw(s, &args, stdin)
}

/// `deem ARGS…` against the stub.
fn deem_raw(s: &Stub, args: &[&str], stdin: &str) -> Run {
    let env = env(s);
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    bin(args, stdin, &env, ".")
}

#[test]
fn text_in_raw_lines_out() {
    let s = fake_api();
    let r = deem_raw(&s, &["is it?", "-t", "0"], "one\ntwo\n");
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "one\ntwo\n", ""));
    let r = deem_raw(&s, &["is it?", "-p"], "{\"a\":1}\n");
    assert!(r.stdout.ends_with("\t{\"a\":1}\n"), "the record as read: {}", r.stdout);
    let r = deem_raw(&s, &["which?", "-o", "x,y"], "one\n");
    assert!(["x\tone\n", "y\tone\n"].contains(&r.stdout.as_str()), "{}", r.stdout);
    let r = deem_raw(&s, &["how urgent?", "--levels", "low, mid ,high", "-J"], "{\"a\":1}\n");
    let (level, record) = r.stdout.trim_end().split_once('\t').unwrap();
    assert!((0.0..=2.0).contains(&level.parse::<f64>().unwrap()), "{}", r.stdout);
    assert_eq!((r.code, record), (0, "{\"a\":1}"), "a level, then the record as read");
    let r = deem_raw(&s, &["-t", "0", "--", "-is it?"], "one\n");
    assert_eq!(r.stdout, "one\n", "a question after --");
    let r = deem_bin(&s, &[r#"{text: ., a: ask("q"; .)}"#], "plain line\n");
    assert!(r.stdout.starts_with(r#"{"text":"plain line","a":"#), "{}", r.stdout);
}

// A file that is a pipe (`deem q <(…)`, a FIFO) is read once, whole: deem
// looks at its first bytes to tell a picture from text, and keeps them.
// Two files, the pipe second, so the bytes kept go in their place; the
// pipe starts with a byte-order mark, dropped as a file's is.
#[cfg(unix)]
#[test]
fn a_pipe_is_read_whole() {
    let s = fake_api();
    let dir = scratch("pipe");
    let (file, pipe) = (dir.join("file.txt"), dir.join("pipe"));
    std::fs::write(&file, "zero\n").unwrap();
    assert!(std::process::Command::new("mkfifo").arg(&pipe).status().unwrap().success());
    let writer = {
        let pipe = pipe.clone();
        std::thread::spawn(move || std::fs::write(pipe, "\u{feff}one\ntwo\n").unwrap())
    };
    let r = deem_raw(&s, &["is it?", "-t", "0", file.to_str().unwrap(), pipe.to_str().unwrap()], "");
    writer.join().unwrap();
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "zero\none\ntwo\n", ""));
    assert_eq!(s.hits.load(Ordering::SeqCst), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn json_in_json_out_and_composes() {
    let s = fake_api();
    let first = deem_bin(&s, &[r#"{text: ., a: ask("q"; .)}"#], "plain line\n");
    let second = deem_bin(&s, &["-J", r#".b = ask("q2"; .)"#], &first.stdout);
    assert!(second.stdout.contains(r#""a":"#) && second.stdout.contains(r#""b":"#), "{}", second.stdout);
    let r = deem_bin(&s, &["-J", ".text"], &first.stdout);
    assert_eq!(r.stdout, "\"plain line\"\n", "JSON input prints strings as JSON");
    let r = deem_bin(&s, &["-J", "-r", ".text"], &first.stdout);
    assert_eq!(r.stdout, "plain line\n");
    let r = deem_bin(&s, &["length"], "{\"a\":1}\n");
    assert_eq!(r.stdout, "7\n", "without -J, JSON is text");
    let r = deem_bin(&s, &["-J", "."], "[1, 2]\n{\n  \"a\": 1\n}\n");
    assert_eq!(r.stdout, "[1,2]\n{\"a\":1}\n", "-J: each value is a record, however it is laid out");
}

#[test]
fn a_forgotten_json_flag_is_named() {
    let s = fake_api();
    let r = deem_bin(&s, &[r#"select(ask("q"; .body) > 0.5)"#], "{\"body\":\"a\"}\n{\"body\":\"b\"}\n");
    assert_eq!((r.code, r.stdout.as_str()), (2, ""));
    assert!(r.stderr.ends_with("deem: the input was read as text; pass -J to read JSON records\n"), "{}", r.stderr);
    assert_eq!(s.hits.load(Ordering::SeqCst), 0, "nothing was asked");
    let r = deem_bin(&s, &["error(\"no\")"], "{\"body\":\"a\"}\n");
    assert!(!r.stderr.contains("-J"), "only when a field was read: {}", r.stderr);
}

#[test]
fn slurp_and_split() {
    let s = fake_api();
    assert_eq!(deem_bin(&s, &["-s", "length"], "a\nb\nc\n").stdout, "5\n");
    assert_eq!(deem_bin(&s, &["--split", r"\n\n+", "."], "one\ntwo\n\nthree\n").stdout, "one\ntwo\nthree\n");
    assert_eq!(deem_bin(&s, &["-J", "-s", "map(.a) | add"], "{\"a\":1}\n{\n\"a\":2}\n").stdout, "3\n");
    let r = deem_bin(&s, &["--split", "(", "."], "x\n");
    assert_eq!(r.code, 2);
    assert!(r.stderr.starts_with("deem: --split:"), "{}", r.stderr);
    let r = deem_bin(&s, &["-J", "--split", r"\0", "."], "1\n");
    assert_eq!(r.code, 2);
    assert_eq!(r.stderr, "deem: --split cuts text into records; with -J each JSON value is a record\n");
}

#[test]
fn exit_codes() {
    let s = fake_api();
    assert_eq!(deem_bin(&s, &["select(false)"], "x\n").code, 1, "nothing selected");
    assert_eq!(deem_bin(&s, &["-e", "false"], "x\n").code, 1);
    assert_eq!(deem_bin(&s, &["-e", "null"], "x\n").code, 1);
    assert_eq!(deem_bin(&s, &["-e", "1"], "x\n").code, 0);
    assert_eq!(deem_bin(&s, &["-e", ". == \"y\""], "x\ny\n").code, 0, "the last output decides");
    let r = deem_bin(&s, &["error(\"no\")"], "x\n");
    assert_eq!((r.code, r.stderr.as_str()), (2, "deem: record 1: no\n"));
    let r = deem_bin(&s, &["if . == \"b\" then error({a: 1}) else . end"], "a\nb\nc\n");
    assert_eq!((r.code, r.stdout.as_str()), (2, "a\nc\n"), "the other records run");
    assert_eq!(
        r.stderr,
        "deem: record 2: {\"a\":1} (not a string)\ndeem: 1 of 3 records failed; --failed FILE keeps them to run again\n"
    );
    let r = deem_bin(&s, &["{"], "x\n");
    assert_eq!(r.code, 2);
    assert!(r.stderr.starts_with("deem: 1:2: expected"), "{}", r.stderr);
    let r = deem_bin(&s, &["-J", ".a"], "{\"a\":1}\nnot json\n{\"a\":3}\n");
    assert_eq!((r.code, r.stdout.as_str()), (2, "1\n"), "the input stops where the JSON breaks");
    assert!(r.stderr.starts_with("deem: reading the input: record 2 (line 2) is not JSON: "), "{}", r.stderr);
    assert!(r.stderr.contains("; the input stops here"), "{}", r.stderr);
}

#[test]
fn usage_errors() {
    let s = fake_api();
    for (args, want) in [
        (vec![], "deem: no question. Try `deem \"is this an error?\" app.log` or `deem --help`\n"),
        (vec!["run", "-t", "0.5", "."], "deem: the program has no value \"threshold\" (it has none)\n"),
        (vec!["-t", "2", "q"], "deem: -t: 2 is not between 0 and 1\n"),
        (vec!["run", "--arg", "t", "1", "."], "deem: the program has no value \"t\" (it has none)\n"),
        (vec!["q", "--arg", "t", "1"], "deem: the program has no value \"t\" (it has: threshold)\n"),
        (vec!["-t", "x", "q"], "deem: -t: \"x\" is not a number\n"),
        (vec!["--nope", "q"], "deem: invalid option '--nope'. Try `deem --help`\n"),
        (vec!["-a", "q"], "deem: invalid option '-a'. Try `deem --help`\n"),
        (vec!["-f", "p.deem", "q"], "deem: invalid option '-f'. Try `deem --help`\n"),
        (vec!["--prelude"], "deem: invalid option '--prelude'. Try `deem --help`\n"),
        (vec!["q", "missing.log"], "deem: missing.log: No such file or directory (os error 2)\n"),
        (
            vec!["q", "-p", "-t", "0.8"],
            "deem: -t doesn't go with -p, -o or -l: they print every line. To keep some, filter the scores: deem \"…\" -p FILE | awk '$1 > 0.8'\n",
        ),
    ] {
        let r = deem_raw(&s, &args, "");
        assert_eq!((r.code, r.stderr.as_str()), (2, want), "{args:?}");
    }
    for env in [vec![], vec![("JEV_BASE_URL", "https://api.typesafe.ai/v1/")]] {
        let r = bin(&["run", "."], "x\n", &env, ".");
        assert_eq!(
            (r.code, r.stderr.as_str()),
            (
                2,
                "deem: no API is added and JEV_API_KEY is not set; to add the hosted one: deem api add jev https://api.typesafe.ai/v1 --pictures --key\n"
            ),
            "{env:?}"
        );
    }
    let r = bin(&["models"], "", &[], ".");
    assert_eq!(
        r.stderr,
        "deem: no API is added and JEV_API_KEY is not set; to add the hosted one: deem api add jev https://api.typesafe.ai/v1 --pictures --key\n"
    );
    // the official SDKs' variable is neither read nor in the way
    let r = bin(&["run", "."], "x\n", &[("TYPESAFE_API_KEY", "k")], ".");
    assert_eq!(
        r.stderr,
        "deem: no API is added and JEV_API_KEY is not set; to add the hosted one: deem api add jev https://api.typesafe.ai/v1 --pictures --key\n"
    );
    let r = bin(&["run", "."], "x\n", &[("JEV_BASE_URL", "http://127.0.0.1:9/v1/systemone")], ".");
    assert_eq!(
        r.stderr,
        "deem: JEV_BASE_URL: \"http://127.0.0.1:9/v1/systemone\" names the endpoint; give the base URL, without `/systemone`\n"
    );
    let s = fake_api();
    let r = bin(&["run", "-r", "ask(\"q\"; .) | type"], "x\n", &[("JEV_BASE_URL", &s.url)], ".");
    assert_eq!(
        (r.code, r.stdout.as_str(), r.stderr.as_str()),
        (0, "number\n", ""),
        "a server of your own needs no key"
    );
    let r = bin(&["run", "--max-questions", "x", "."], "x\n", &[], ".");
    assert_eq!(r.stderr, "deem: --max-questions: \"x\" is not a whole number\n");
    let r = bin(&["run", "."], "x\n", &[("JEV_API_KEY", "k"), ("DEEM_MAX_RECORD_BYTES", "lots")], ".");
    assert_eq!(r.stderr, "deem: DEEM_MAX_RECORD_BYTES: \"lots\" is not a whole number\n");
    let r = bin(&["run", "."], "x\n", &[("JEV_API_KEY", "k"), ("JEV_REQUESTS_PER_SECOND", "0")], ".");
    assert_eq!(r.stderr, "deem: JEV_REQUESTS_PER_SECOND: 0 is not from 1 to 1000\n");
}

#[test]
fn help_version_models() {
    let r = bin(&["--help"], "", &[], ".");
    assert!(r.code == 0 && r.stdout.starts_with("deem - grep by meaning"));
    assert_eq!(bin(&["help"], "", &[], ".").stdout, r.stdout);
    assert_eq!(bin(&["run", "--help"], "", &[], ".").stdout, r.stdout);
    assert!(bin(&["--version"], "", &[], ".").stdout.starts_with("deem "));
    let s = stub(|_, line, _| (200, format!("{{\"line\": \"{line}\"}}")));
    let env = env(&s);
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let r = bin(&["models"], "", &env, ".");
    assert_eq!(r.stdout, "{\"line\": \"GET /v1/models HTTP/1.1\"}\n");
}

/// A request per record; a record's questions share its request.
#[test]
fn one_request_per_record() {
    let s = fake_api();
    let input = lines((0..7).map(|i| format!("record {i}")));
    let r = deem_bin(&s, &[r#"[ask("q1"; .), ask("q2"; .)]"#], &input);
    assert_eq!(s.hits.load(Ordering::SeqCst), 7);
    assert_eq!(r.stdout.lines().count(), 7);
}

#[test]
fn several_files() {
    let s = fake_api();
    let r = deem_bin(&s, &["-r", "select(test(\"ERROR\"))", "examples/app.log", "examples/app.log"], "");
    assert_eq!(r.stdout.lines().count(), 4);
}

#[test]
fn byte_limit() {
    let s = fake_api();
    let big = "é".repeat(200);
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", s.url.as_str()), ("DEEM_MAX_RECORD_BYTES", "100")];
    // what is judged is limited, not what is read
    let line = format!("{{\"id\":1,\"blob\":\"{big}\"}}\n");
    let r = bin(&["run", "-J", r#".id | tostring | ask("q"; .) | . >= 0"#], &line, &env, ".");
    assert_eq!((r.stdout.as_str(), r.stderr.as_str()), ("true\n", ""));
    // a long string is cut on a character boundary, the record printed whole
    let r = bin(&["run", r#"select(ask("q"; .) >= 0)"#], &format!("{big}\n"), &env, ".");
    assert_eq!(r.stdout, format!("{big}\n"));
    assert_eq!(
        r.stderr,
        "deem: cut 1 judged value(s) to DEEM_MAX_RECORD_BYTES (100); the model saw only the start of each\n"
    );
    // anything else is an error that says what to do
    let r = bin(&["run", "-J", r#"ask("q"; .)"#], &line, &env, ".");
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("over DEEM_MAX_RECORD_BYTES (100). Judge a smaller part"), "{}", r.stderr);
}

// The model sees exactly what `tojson` prints: numbers as written, keys in
// order. A value that isn't JSON is an error at the call, which `try` catches.
#[test]
fn judged_as_printed() {
    let rec = r#"{"z":1,"id":12345678901234567890,"price":1.10}"#;
    let r = deem_json(r#"ask("q"; .) | . >= 0"#, &format!("{rec}\n"));
    assert_eq!((r.stdout.as_str(), r.stderr.as_str()), ("true\n", ""));
    assert_eq!(r.log[0].state.get(), rec);

    let r = deem(r#"[try ask("q"; {n: nan}) catch .] | tojson"#, "a\n");
    let want = r#"["judge: the value to judge isn't JSON (NaN, Infinity, a non-string object key, or a byte string); see \"Differences from jq\""]"#;
    assert_eq!((r.stdout.trim_end(), r.log.len()), (want, 0), "{}", r.stderr);
    let r = deem(r#"try ask("q"; infinite) catch "x""#, "a\n");
    assert_eq!((r.stdout.as_str(), r.log.len()), ("x\n", 0), "{}", r.stderr);
}

#[test]
fn stats() {
    let s = fake_api();
    let r = deem_bin(&s, &["--stats", r#"ask("q"; .)"#], "a\nb\n");
    assert!(
        r.stderr.starts_with("deem: records 2 · questions 2 of 100000 · requests 2 · waves 1 (2 questions)\n"),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("score histogram (2 float answers)"), "{}", r.stderr);
}

/// A cascade sends what a pipe of deems sends: a request per line for the
/// first question, then one per line that passed for the second. The
/// program takes two waves, each deem in the pipe one, and both print the
/// same lines.
#[test]
fn a_cascade_sends_what_a_pipe_sends() {
    let input = lines((0..30).map(|i| format!("line {i}")));
    // `requests N` or `waves N`, from `--stats`
    let stat = |r: &Run, what: &str| -> usize {
        let key = format!("{what} ");
        let at = r.stderr.find(&key).unwrap_or_else(|| panic!("{}", r.stderr)) + key.len();
        r.stderr[at..].split(' ').next().unwrap().parse().unwrap()
    };
    let program = r#"select(ask("A?"; .) > 0.5) | select(ask("B?"; .) > 0.5)"#;
    let s = fake_api();
    let one = deem_bin(&s, &["--stats", "-r", program], &input);
    let (s1, s2) = (fake_api(), fake_api());
    let first = deem_raw(&s1, &["--stats", "A?"], &input);
    let second = deem_raw(&s2, &["--stats", "B?"], &first.stdout);
    let passed = first.stdout.lines().count();
    assert!(passed > 0 && passed < 30 && !second.stdout.is_empty(), "{passed}");
    assert_eq!(one.stdout, second.stdout);
    let sent = |s: &Stub| s.hits.load(Ordering::SeqCst);
    assert_eq!((sent(&s1), sent(&s2)), (30, passed));
    assert_eq!(sent(&s), sent(&s1) + sent(&s2));
    assert_eq!(
        (stat(&one, "requests"), stat(&first, "requests"), stat(&second, "requests")),
        (30 + passed, 30, passed)
    );
    assert_eq!((stat(&one, "waves"), stat(&first, "waves"), stat(&second, "waves")), (2, 1, 1));
}

#[test]
fn closed_stdout() {
    let s = fake_api();
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", s.url.as_str())];
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_deem"));
    cmd.args(["run", "range(1e9)"]).env_clear().envs(OFFLINE).envs(env).stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    {
        use std::io::{Read, Write};
        child.stdin.take().unwrap().write_all(b"x\n").unwrap();
        let mut out = child.stdout.take().unwrap();
        let mut buf = [0; 100];
        out.read_exact(&mut buf).unwrap();
    }
    let o = child.wait_with_output().unwrap();
    assert_eq!((o.status.code(), o.stderr.as_slice()), (Some(141), &b""[..]));
}

#[test]
fn trace() {
    let s = fake_api();
    let dir = std::env::temp_dir().join(format!("deem-trace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.jsonl");
    let program = r#"select(ask("q1"; .) > 0.5) | {a: ask("q2"; .), b: ask("q3"; .)}"#;
    let input = lines((0..30).map(|i| format!("line {i}")));
    let r = deem_bin(&s, &["--trace", path.to_str().unwrap(), program], &input);
    assert_eq!(r.stderr, "");
    let text = std::fs::read_to_string(&path).unwrap();
    let events: Vec<serde_json::Value> = text.lines().skip(1).map(|l| serde_json::from_str(l).unwrap()).collect();
    let count = |kind: &str| events.iter().filter(|e| e["ev"] == kind).count();
    // a round per record for q1, then one for each that passed, q2 and q3
    let passed = r.stdout.lines().count();
    assert_eq!(count("wave"), 30 + passed, "the cascade: q1, then q2 and q3");
    assert!(events.iter().filter(|e| e["ev"] == "wave").all(|e| e["wave"] == 1 || e["wave"] == 2));
    assert_eq!(count("request"), s.hits.load(Ordering::SeqCst));
    assert_eq!(count("answered"), count("request"));
    assert_eq!((count("record"), count("end")), (30, 30));
    assert_eq!(count("ask"), 30 + 2 * r.stdout.lines().count());
    let r = deem_bin(&s, &["--trace", "/nonexistent/dir/t.json", "."], "x\n");
    assert_eq!(r.code, 2);
    assert!(r.stderr.starts_with("deem: --trace: "), "{}", r.stderr);
    std::fs::remove_dir_all(dir).unwrap();
}

/// jaq prints what JSON can't hold as it is; deem says so once, on stderr,
/// and the run still succeeds.
#[test]
fn output_that_isnt_json_is_flagged_once() {
    let s = fake_api();
    let r = deem_bin(&s, &["-J", "{r: (1 / .n)}"], "{\"n\":1}\n{\"n\":0}\n{\"n\":0}\n");
    assert_eq!(r.code, 0);
    assert_eq!(r.stdout, "{\"r\":1.0}\n{\"r\":Infinity}\n{\"r\":Infinity}\n");
    let want = "deem: record 2 printed a value that isn't JSON (NaN, Infinity, a non-string object key, or a byte string); see \"Differences from jq\"\n";
    assert_eq!(r.stderr, want);
    let r = deem_bin(&s, &["-J", ".n / 2"], "{\"n\":1}\n");
    assert_eq!((r.stdout.as_str(), r.stderr.as_str()), ("0.5\n", ""));
}

/// Streaming input (`journalctl -f | deem …`): the input never ends and the
/// window never fills, so each line's question goes out once the batcher
/// has been quiet, and its answer is printed while the input stays open.
#[test]
fn streaming_input_is_answered_as_it_comes() {
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;
    let s = fake_api();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_deem"))
        .args(["run", r#"{line: ., a: ask("q"; .)}"#])
        .env_clear()
        .envs(OFFLINE)
        .envs(env(&s))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || BufReader::new(stdout).lines().map_while(Result::ok).for_each(|l| drop(tx.send(l))));
    for (i, line) in ["first", "second", "third"].iter().enumerate() {
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        // well beyond the 50 ms of quiet it waits for
        let out = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| panic!("no answer to {line:?}"));
        assert!(out.starts_with(&format!(r#"{{"line":"{line}","a":"#)), "{out}");
        assert_eq!(s.hits.load(Ordering::SeqCst), i + 1, "a request per line, while the input is open");
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// `--failed FILE`: each record that didn't finish, as it was read, one per
/// line; NUL-separated when `--split` says records may hold newlines.
#[test]
fn failed_records_are_written_to_run_again() {
    let s = fake_api();
    let file = std::env::temp_dir().join(format!("deem-failed-{}.txt", std::process::id()));
    let path = file.to_str().unwrap();
    let r = deem_bin(&s, &["--failed", path, r#"select(ask("FAIL"; .) > 0.5) | "kept""#], "fine\nboom one\nboom two\n");
    assert_eq!(r.code, 2);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "boom one\nboom two\n");
    let r = deem_bin(&s, &["--split", ",", "--failed", path, r#"ask("FAIL"; .)"#], "fine,boom\nline,x");
    assert_eq!(r.code, 2);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "boom\nline\0");
    // with -J, each value compact on its line, so -J reads the file back
    let pretty = "{\n  \"t\": \"fine\"\n}\n{\n  \"t\": \"boom\"\n}\n";
    let r = deem_bin(&s, &["-J", "--failed", path, r#".t | ask("FAIL"; .)"#], pretty);
    assert_eq!(r.code, 2);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"t\":\"boom\"}\n");
    let again = std::fs::read_to_string(&file).unwrap();
    assert_eq!(deem_bin(&s, &["-J", ".t"], &again).stdout, "\"boom\"\n");
    let r = deem_bin(&s, &["--failed", path, r#"ask("q"; .)"#], "fine\n");
    assert_eq!((r.code, std::fs::read_to_string(&file).unwrap().as_str()), (0, ""), "nothing failed");
    std::fs::remove_file(&file).unwrap();
    let r = deem_bin(&s, &["--failed", "/nonexistent/dir/f.txt", "."], "x\n");
    assert!(r.stderr.starts_with("deem: --failed: "), "{}", r.stderr);
}

// `deem run`: the word, then as many arguments as main takes, then files;
// options anywhere, `--` before an argument that starts with `-`.
#[test]
fn run_programs() {
    let s = fake_api();
    let dir = std::env::temp_dir().join(format!("deem-run-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".deem")).unwrap();
    std::fs::write(dir.join("in.txt"), "a\nb\n").unwrap();
    std::fs::write(
        dir.join(".deem/echo.deem"),
        "module {defaults: {threshold: 0.5, n: 1}};\n# echo: say it\ndef main($x): \"\\($x) \\(.) \\($threshold) \\($n)\";",
    )
    .unwrap();
    let mut env = env(&s);
    env.push(("HOME", dir.join("nohome").to_str().unwrap().into()));
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let run = |args: &[&str]| bin(args, "", &env, dir.to_str().unwrap());
    let r = run(&["run", "echo", "hi", "-t", "0.7", "in.txt", "--arg", "n", "2"]);
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "hi a 0.7 2\nhi b 0.7 2\n", ""));
    let r = run(&["run", "echo", "--", "-x", "in.txt"]);
    assert_eq!(r.stdout, "-x a 0.5 1\n-x b 0.5 1\n");
    let r = run(&["run", "./.deem/echo.deem", "{\"a\":1}", "in.txt"]);
    assert_eq!(r.stdout.lines().next(), Some("{\"a\":1} a 0.5 1"), "an argument is a string, printed raw");
    let r = run(&["run", "echo"]);
    assert_eq!((r.code, r.stderr.as_str()), (2, "deem: echo takes 1 argument (x), got 0\n"));
    let r = run(&["run", "echo", "x", "in.txt", "--arg", "m", "1"]);
    assert_eq!(r.stderr, "deem: echo has no value \"m\" (it has: threshold, n)\n");
    let r = run(&["run", "mix", "-t", "0.5", "q", "a,b", "in.txt"]);
    assert_eq!(r.stderr, "deem: mix has no value \"threshold\" (it has none)\n");
    let r = run(&["run", "echo2", "in.txt"]);
    assert_eq!(r.stderr, "deem: no program named \"echo2\" (looked in: built-in, .deem/, ~/.deem/)\n");
    let r = run(&["run", "ascii_downcase", "in.txt"]);
    assert_eq!(r.stdout, "a\nb\n", "jq text");
    let r = run(&["run", "disagree", "q1", "q2", "in.txt"]);
    assert_eq!(r.code, if r.stdout.is_empty() { 1 } else { 0 });
    assert_eq!(r.stderr, "");
    let r = run(&["run", "histogram", "-s", "q", "in.txt"]);
    assert_eq!(r.stderr, "deem: -s can't be used with an input \"all\" program: it already reads every record\n");

    let r = run(&["programs"]);
    assert!(r.stdout.starts_with("echo X [threshold=0.5] [n=1]"), "{}", r.stdout);
    assert!(r.stdout.lines().next().unwrap().ends_with(" .deem/echo.deem"), "{}", r.stdout);
    assert_eq!(r.stdout.lines().nth(1), Some("    say it"));
    assert_eq!(run(&["programs", "group"]).stdout, deem::programs::BUILTIN[0].1);
    assert_eq!(run(&["programs", "length"]).code, 2);

    // Without HOME, Windows' USERPROFILE is the home directory.
    let profile = dir.join("profile");
    std::fs::create_dir_all(profile.join(".deem")).unwrap();
    std::fs::write(profile.join(".deem/mine.deem"), "\"mine \\(.)\"").unwrap();
    let mut env: Vec<(&str, &str)> = env.into_iter().filter(|(k, _)| *k != "HOME").collect();
    env.push(("USERPROFILE", profile.to_str().unwrap()));
    let r = bin(&["run", "mine", "in.txt"], "", &env, dir.to_str().unwrap());
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "mine a\nmine b\n", ""));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tune_needs_a_terminal() {
    let s = fake_api();
    let env = env(&s);
    let has_setsid = std::process::Command::new("setsid").arg("--version").output().is_ok();
    if !has_setsid {
        return;
    }
    let mut cmd = std::process::Command::new("setsid");
    cmd.arg(env!("CARGO_BIN_EXE_deem")).args(["tune", "q"]).env_clear().envs(OFFLINE);
    for (k, v) in &env {
        cmd.env(k, v);
    }
    let o = cmd.stdin(std::process::Stdio::null()).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "deem: deem tune needs a terminal to ask you; its input can come from a file or a pipe\n"
    );
}

// `--failed` and `--trace` empty their file when the run starts, so one that
// is also an input is refused, and the input is left as it was: a rerun of
// `retry.log` with `--failed retry.log` would lose what it was to run again.
#[test]
fn a_written_file_is_not_an_input() {
    let s = fake_api();
    let dir = std::env::temp_dir().join(format!("deem-same-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("retry.log");
    let lines: String = (0..3000).map(|i| format!("ticket number {i}\n")).collect();
    std::fs::write(&file, &lines).unwrap();
    let path = file.to_str().unwrap();
    let other = dir.join("./retry.log");
    for flag in ["--failed", "--trace"] {
        let r = deem_bin(&s, &[flag, path, r#"ask("q"; .)"#, other.to_str().unwrap()], "");
        let want = format!(
            "deem: {flag} {path} is also an input, and deem would empty it before reading it; write to another file\n"
        );
        assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (2, "", want.as_str()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), lines, "the input is untouched");
    }
    let r = deem_bin(&s, &["--failed", dir.join("again.log").to_str().unwrap(), "length", path], "");
    assert_eq!((r.code, r.stdout.lines().count()), (0, 3000), "another file is fine");
    std::fs::remove_dir_all(&dir).unwrap();
}

// Options go before `run` too, and an option's name may hold spaces: those
// around a comma are dropped.
#[test]
fn options_before_run_and_spaces_in_options() {
    let s = fake_api();
    let r = deem_raw(&s, &["-J", "-r", "run", ".a"], "{\"a\": 1}\n");
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (0, "1\n", ""));
    let r = deem_raw(&s, &["which?", "-o", "bug , for my mom"], "x\ny\nz\nw\n");
    assert_eq!(r.code, 0, "{}", r.stderr);
    let options: Vec<&str> = r.stdout.lines().map(|l| l.split('\t').next().unwrap()).collect();
    assert!(options.iter().all(|o| ["bug", "for my mom"].contains(o)), "{options:?}");
    let r = deem_bin(&s, &["mix", "which?", "bug , for my mom"], "x\ny\n");
    let rows: Vec<&str> = r.stdout.lines().take(2).map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(rows, ["bug", "for my mom"]);
}

// A deem that a test starts reaches this machine and nothing else
// (`OFFLINE`): with a key, and a base URL on another host, its request goes
// to the proxy where nothing listens and is refused there, before the host
// is even looked up. So a test that forgets its stub's URL fails instead of
// asking the hosted API. (The host here can't exist: RFC 6761's `.invalid`.)
// http:// to another host is refused before anything is sent.
#[test]
fn no_test_reaches_another_machine() {
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", "https://deem-test.invalid/v1")];
    let r = bin(&["models"], "", &env, ".");
    assert_eq!(
        (r.code, as_on_linux(&r.stderr)),
        (2, "deem: request failed: io: Connection refused (os error 111)\n".into())
    );
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", "http://deem-test.invalid/v1")];
    let r = bin(&["models"], "", &env, ".");
    assert_eq!(r.code, 2);
    assert!(
        r.stderr.starts_with("deem: JEV_BASE_URL: \"http://deem-test.invalid/v1\" would send the key"),
        "{}",
        r.stderr
    );
}
