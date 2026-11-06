//! Pictures: PNG and JPEG files named on the command line (or on stdin with
//! `-I`), each one record; `--img NAME FILE`; and the errors that keep
//! text and pictures apart. Against the fake API, which never decodes a
//! picture.

mod common;

use common::*;
use deem::{Input, RunOptions as Opts, open_input};
use std::sync::Arc;

/// Pictures in a fresh directory: (name, bytes) → their paths.
fn files(dir: &str, pics: &[(&str, Vec<u8>)]) -> Vec<String> {
    let dir = scratch(dir);
    pics.iter()
        .map(|(name, bytes)| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p.to_str().unwrap().to_owned()
        })
        .collect()
}

/// The ID of a request's one picture, as its asset names it.
fn only_asset(req: &deem::api::Request) -> Option<&str> {
    assert_eq!(req.assets.len(), 1, "{:?}", req.assets);
    let (id, a) = req.assets.iter().next()?;
    assert_eq!((a.kind, a.part.as_str()), ("image", id.as_str()));
    Some(id)
}

fn run_on(program: &str, input: Input, opts: &Opts) -> Run {
    let fake = Arc::new(Fake::default());
    let mut r = run_input(program, &[], input, opts, &config(), fake.clone());
    r.log = fake.log.lock().unwrap().clone();
    r
}

fn pictures(program: &str, paths: &[String], opts: &Opts) -> Run {
    run_on(program, open_input(paths, false).unwrap(), opts)
}

// Each picture file is one record; the question forms print its path, and
// a program prints a picture as its path too: the reference is deem's.
#[test]
fn a_picture_file_is_a_record() {
    let ps = files("records", &[("a.png", png(800, 600)), ("b.jpg", jpeg(400, 400)), ("c.png", png(300, 300))]);
    let (score, args) = deem::programs::program("is there a person?", &deem::programs::Mode::Score);
    let fake = Arc::new(Fake::default());
    let mut r = run_input(score, &args, open_input(&ps, false).unwrap(), &Opts::default(), &config(), fake.clone());
    r.log = fake.log.lock().unwrap().clone();
    assert_eq!(r.code, 0, "{}", r.stderr);
    let printed: Vec<&str> = r.lines().iter().map(|l| l.split_once('\t').unwrap().1).collect();
    assert_eq!(printed, ps.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!((r.waves(), r.log.len()), (1, 3), "a request each, one wave");
    let r = pictures(r#"{photo: ., n: 1}, ., "in text: \(.)""#, &ps[..1], &Opts::default());
    let p = &ps[0];
    assert_eq!(r.stdout, format!("{{\"photo\":\"{p}\",\"n\":1}}\n{p}\nin text: {p}\n"), "{}", r.stderr);
}

// Files are pictures or text, never both in one run; other picture formats
// are named.
#[test]
fn pictures_and_text_apart() {
    let ps = files("apart", &[("a.png", png(800, 600)), ("notes.txt", b"hello\n".to_vec())]);
    let e = open_input(&ps, false).err().unwrap();
    assert_eq!(
        e,
        format!("{} is a picture and {} is text: a run reads pictures or text; run them apart", ps[0], ps[1])
    );
    assert!(matches!(open_input(&ps[1..], false), Ok(Input::Text(_))));
    let gif = files("gif", &[("a.gif", b"GIF89a\x90\x01\x2c\x01\xf7\x00\x00".to_vec())]);
    assert_eq!(
        open_input(&gif, false).err().unwrap(),
        format!("{} is a GIF; deem sends PNG or JPEG: convert it", gif[0])
    );
    let e = open_input(&ps[..1], true).err().unwrap();
    assert_eq!(e, "-I reads pictures' paths from stdin; give the pictures as files without it");
    for (opts, flag) in [
        (Opts { json_in: true, ..Opts::default() }, "-J"),
        (Opts { slurp: true, ..Opts::default() }, "-s"),
        (Opts { split: Some(",".into()), ..Opts::default() }, "--split"),
    ] {
        let r = pictures(".", &ps[..1], &opts);
        assert_eq!((r.code, r.stderr), (2, format!("deem: {flag} reads text, and these files are pictures\n")));
    }
}

// With -I, each stdin line is a picture's path: what `find` prints, or a
// first deem's output.
#[test]
fn paths_on_stdin() {
    let ps = files("paths", &[("a.png", png(800, 600)), ("b.png", png(400, 400))]);
    let stdin = format!("{}\n\n{}\r\n", ps[0], ps[1]);
    let input = Input::Paths(Box::new(std::io::Cursor::new(stdin.into_bytes())));
    let r = run_on(".", input, &Opts::default());
    assert_eq!((r.code, r.lines()), (0, vec![ps[0].as_str(), ps[1].as_str()]), "{}", r.stderr);
    let input = Input::Paths(Box::new(std::io::Cursor::new(b"x".to_vec())));
    let r = run_on(".", input, &Opts { json_in: true, ..Opts::default() });
    assert_eq!(r.stderr, "deem: -J reads text, and -I reads pictures' paths\n");
}

// A picture deem can't send fails its record, before anything is sent, and
// `--failed` keeps its path, to run again with -I.
#[test]
fn a_bad_picture_fails_its_record() {
    let ps = files("bad", &[("ok.png", png(800, 600)), ("small.png", png(200, 200)), ("text.png", b"hi".to_vec())]);
    let failed = scratch("bad-failed").join("failed.txt");
    let opts = Opts { failed: Some(failed.to_str().unwrap().into()), ..Opts::default() };
    // `text.png` isn't a picture by its bytes, so it would be text: the run
    // is refused before anything runs
    assert!(open_input(&ps, false).is_err());
    let r = run_on(r#"ask("q"; .)"#, Input::Pictures(ps[..2].to_vec()), &opts);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains(&format!("record 2: {} is 200×200; the API takes 280 to 1568", ps[1])), "{}", r.stderr);
    assert_eq!(r.log.len(), 1, "only the good one is asked");
    assert_eq!(std::fs::read_to_string(&failed).unwrap(), format!("{}\n", ps[1]));
}

// `--img NAME FILE` binds `$NAME` to a picture; `--rawfile` is for text.
#[test]
fn img_binds_a_picture() {
    let ps = files("img", &[("badge.png", png(400, 400)), ("note.txt", b"x\n".to_vec())]);
    let img = |name: &str, file: &str| Opts { img: vec![(name.into(), file.into())], ..Opts::default() };
    let r = deem_opts(
        r#"ask("is `badge` the same person as `who`?"; {badge: $badge, who: .})"#,
        "alice\n",
        &img("badge", &ps[0]),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let state: serde_json::Value = serde_json::from_str(r.log[0].state.get()).unwrap();
    assert_eq!(state["badge"].as_str(), only_asset(&r.log[0]), "{state}");
    let r = deem_opts("$b", "x\n", &img("b", &ps[1]));
    assert_eq!(r.stderr, format!("deem: --img b: {} is not a PNG or JPEG\n", ps[1]));
    let opts = Opts { rawfile: vec![("b".into(), ps[0].clone())], ..Opts::default() };
    assert_eq!(
        deem_opts("$b", "x\n", &opts).stderr,
        format!("deem: --rawfile: {} is a picture; bind it with --img\n", ps[0])
    );
    let r = deem_opts("$b", "x\n", &img("b", &ps[0]));
    assert_eq!(r.stdout, format!("{}\n", ps[0]), "printed as its path");
}

// A program over every record gets the pictures' references as an array.
#[test]
fn every_picture_as_one() {
    let ps = files("all", &[("a.png", png(800, 600)), ("b.png", png(400, 400))]);
    let r = pictures("module {input: \"all\"};\ndef main: length, .[1];", &ps, &Opts::default());
    assert_eq!(r.stdout, format!("2\n{}\n", ps[1]), "{}", r.stderr);
}

// A picture goes where the docs put it: a whole string in the value (or an
// option, a level, an example), named in the question in backticks.
#[test]
fn where_a_picture_goes() {
    let ps = files("where", &[("a.png", png(800, 600))]);
    let ok = |program: &str| {
        let r = pictures(program, &ps, &Opts::default());
        assert_eq!(r.code, 0, "{program}: {}", r.stderr);
        r
    };
    // a picture alone goes as {"image": …}
    let r = ok(r#"ask("is there a person?"; .)"#);
    let state: serde_json::Value = serde_json::from_str(r.log[0].state.get()).unwrap();
    assert_eq!(state["image"].as_str(), only_asset(&r.log[0]), "{state}");
    assert_eq!(state.as_object().unwrap().len(), 1);
    let r = ok(r#"ask("does `photo` support `claim`?"; {claim: "it broke", photo: .})"#);
    let id = only_asset(&r.log[0]).unwrap();
    assert_eq!(r.log[0].state.get(), format!(r#"{{"claim":"it broke","photo":"{id}"}}"#));
    ok(r#"choose("which?"; {me: ., you: "someone else"}; "who")"#);
    ok(r#"rate("how alike?"; ["unlike", ., "the same"]; "x")"#);
    ok(r#"judge({q: "like this?", like: [.]}; "x").value"#);
    let p = &ps[0];
    for (program, want) in [
        (
            r#"ask("q"; "is the \(.) friendly?")"#,
            format!(
                r#"a picture must be a whole string: put it in the value and name it in backticks (in "is the {p} friendly?")"#
            ),
        ),
        (
            r#"ask("q"; {(.): 1})"#,
            format!(r#"a picture must be a whole string: put it in the value and name it in backticks (in "{p}")"#),
        ),
        (
            r#"ask("is the \(.) friendly?"; "x")"#,
            "a picture can't be in the question's text: put it in the value and name it in backticks".to_owned(),
        ),
        (
            r#"choose("q"; {a: "see \(.)", b: "x"}; "x")"#,
            format!(r#"a picture must be a whole string: put it in the value and name it in backticks (in "see {p}")"#),
        ),
    ] {
        let r = pictures(program, &ps, &Opts::default());
        assert_eq!((r.code, r.stderr), (2, format!("deem: record 1: judge: {want}\n")), "{program}");
        assert!(r.log.is_empty(), "nothing is sent");
    }
}

// A request shows each picture once as an asset, by an ID that is a hash of
// its bytes, so the same run sends the same bytes every time.
#[test]
fn sending_pictures() {
    let ps = files("send", &[("a.png", png(800, 600)), ("b.png", png(400, 400)), ("c.jpg", jpeg(600, 600))]);
    let two = r#"{a: ask("is there a person?"; .), b: choose("which is it?"; {this: ., other: "something else"}; .)}"#;
    let r = pictures(two, &ps, &Opts::default());
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!((r.waves(), r.log.len()), (1, 3), "one wave, a request a picture");
    for req in &r.log {
        let id = only_asset(req).unwrap();
        // in the state, and in the option: one asset, shown twice
        assert_eq!(req.body().matches(id).count(), 4, "two uses, and the asset's name and part: {}", req.body());
        assert!(!req.body().contains("<image:"), "{}", req.body());
    }
    let again = pictures(two, &ps, &Opts::default());
    // (sent by several senders at once, in any order)
    let bodies = |r: &Run| r.log.iter().map(|q| q.body()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(bodies(&r), bodies(&again), "the same bytes every run");
    // two files with the same bytes are one picture to the API
    let twins = files("twins", &[("x.png", png(800, 600)), ("y.png", png(800, 600))]);
    let opts = Opts { img: vec![("y".into(), twins[1].clone())], ..Opts::default() };
    let r = pictures(r#"ask("same?"; {x: ., y: $y})"#, &twins[..1], &opts);
    assert_eq!((r.code, r.log[0].assets.len()), (0, 1), "{}", r.stderr);
}

// `--trace` has each request as sent, its pictures by their IDs, and the
// file each ID was read from; never a reference.
#[test]
fn a_trace_names_the_pictures_sent() {
    let ps = files("traced", &[("a.png", png(800, 600))]);
    let path = scratch("traced-log").join("run.jsonl");
    let opts = Opts { trace: Some(path.to_str().unwrap().to_owned()), ..Opts::default() };
    let r = pictures(r#"ask("is there a person?"; .)"#, &ps, &opts);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let trace = std::fs::read_to_string(&path).unwrap();
    assert!(!trace.contains("<image:"), "{trace}");
    let req: serde_json::Value = trace
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .find(|e: &serde_json::Value| e["ev"] == "request")
        .unwrap();
    let id = only_asset(&r.log[0]).unwrap();
    assert_eq!(req["body"]["state"], serde_json::json!({"image": id}));
    assert_eq!(req["body"]["assets"][id]["part"], id);
    assert_eq!(req["pictures"], serde_json::json!({id: ps[0]}));
}

// A picture gone by the time its request goes: the request isn't sent, so
// its trace event has no body (and no reference ever reaches the trace).
#[test]
fn a_request_not_sent_has_no_body() {
    let ps = files("gone", &[("a.png", png(800, 600)), ("b.png", png(800, 600))]);
    // one sender: the first request sent takes the other picture away
    struct Remove(Vec<String>, Fake);
    impl deem::api::Judge for Remove {
        fn judge(&self, r: &deem::api::Request) -> Result<deem::api::Response, deem::api::ApiError> {
            for p in self.0.iter().filter(|p| !r.assets.values().any(|a| &a.path == *p)) {
                let _ = std::fs::remove_file(p);
            }
            self.1.judge(r)
        }
    }
    let path = scratch("gone-log").join("run.jsonl");
    let opts = Opts { trace: Some(path.to_str().unwrap().to_owned()), ..Opts::default() };
    let cfg = deem::Config { workers: 1, ..config() };
    let judge = Arc::new(Remove(ps.clone(), Fake::default()));
    // one record, both pictures read: its round's two requests, in order
    let program = r#"module {input: "all"}; [.[] | ask("is there a person?"; .)]"#;
    let r = run_input(program, &[], open_input(&ps, false).unwrap(), &opts, &cfg, judge);
    assert_eq!(r.code, 2, "the second answer fails the record: {}", r.stderr);
    let trace = std::fs::read_to_string(&path).unwrap();
    assert!(!trace.contains("<image:"), "{trace}");
    let reqs: Vec<serde_json::Value> = trace
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|e: &serde_json::Value| e["ev"] == "request")
        .collect();
    let bodies: Vec<bool> = reqs.iter().map(|e| e.get("body").is_some()).collect();
    assert_eq!(bodies, [true, false], "the first was sent, the second wasn't");
}

// Data can't name a picture: text that looks like a reference is text,
// even with the picture on the command line.
#[test]
fn data_cant_name_a_picture() {
    let ps = files("forge", &[("id_card.png", png(800, 600))]);
    let opts = Opts { img: vec![("id".into(), ps[0].clone())], ..Opts::default() };
    let line = format!("hi <image:{}>\n\"<image:00000000000000000000000000000000:{}>\"\n", ps[0], ps[0]);
    let r = deem_opts(r#"ask("is this spam?"; .)"#, &line, &opts);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.log.iter().all(|q| q.assets.is_empty()), "no picture is sent");
}

// A question the program never asks isn't sent, so its picture is never
// read.
#[test]
fn an_unasked_picture_isnt_sent() {
    let ps = files("unasked", &[("a.png", png(800, 600))]);
    let r = pictures(r#"if false then ask("q"; .) else "done" end"#, &ps, &Opts::default());
    assert_eq!((r.code, r.stdout.as_str(), r.log.len()), (0, "done\n", 0));
}

// Pictures cost tokens: a request over the API's 32,000 for the value and a
// question is refused before it is sent, naming the pictures.
#[test]
fn pictures_count_against_the_token_limit() {
    let names: Vec<(String, Vec<u8>)> = (0..6).map(|i| (format!("p{i}.png"), png(1568, 1568))).collect();
    let named: Vec<(&str, Vec<u8>)> = names.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
    let ps = files("tokens", &named);
    let img = ps.iter().enumerate().map(|(i, p)| (format!("p{i}"), p.clone())).collect();
    let opts = Opts { img, ..Opts::default() };
    let r = deem_opts(r#"choose("which?"; {a: $p0, b: $p1, c: $p2, d: $p3, e: $p4, f: $p5}; .)"#, "x\n", &opts);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("tokens with its pictures ("), "{}", r.stderr);
    assert!(r.stderr.contains("over the API's 32000 for the value and a question"), "{}", r.stderr);
    assert!(r.log.is_empty(), "nothing is sent");
    // four fit
    let r = deem_opts(r#"choose("which?"; {a: $p0, b: $p1, c: $p2, d: $p3}; .)"#, "x\n", &opts);
    assert_eq!(r.code, 0, "{}", r.stderr);
}

/// What the client sends for one request: its head and body, through a raw
/// socket that reads a chunked body or a sized one.
fn raw_request(req: &deem::api::Request) -> (Vec<String>, Vec<u8>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let t = std::thread::spawn(move || {
        let (conn, _) = listener.accept().unwrap();
        let mut r = BufReader::new(conn.try_clone().unwrap());
        let line = |r: &mut BufReader<_>| {
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            l
        };
        let mut head = Vec::new();
        loop {
            let l = line(&mut r);
            if l.trim().is_empty() {
                break;
            }
            head.push(l.trim().to_owned());
        }
        let header =
            |k: &str| head.iter().find_map(|h| h.to_ascii_lowercase().strip_prefix(k).map(|v| v.trim().to_owned()));
        let mut body = Vec::new();
        if let Some(n) = header("content-length:") {
            body.resize(n.parse().unwrap(), 0);
            r.read_exact(&mut body).unwrap();
        } else {
            loop {
                let n = usize::from_str_radix(line(&mut r).trim(), 16).unwrap();
                let mut chunk = vec![0; n + 2];
                r.read_exact(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                body.extend(&chunk[..n]);
            }
        }
        let ok = r#"{"answers": {"q0": {"type": "noul", "noul": 0.5}}}"#;
        let mut w = conn;
        w.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{ok}", ok.len()).as_bytes()).unwrap();
        (head, body)
    });
    use deem::api::Judge;
    deem::api::Http::new(&url, "k", std::time::Duration::from_secs(5)).judge(req).unwrap();
    t.join().unwrap()
}

// Over HTTP a request with a picture is multipart: the request as the part
// `request`, the picture as a file part named by its ID, and the images
// header. A request without one is plain JSON, without the header.
#[test]
fn pictures_go_as_multipart() {
    let ps = files("multipart", &[("a.png", png(800, 600))]);
    let r = pictures(r#"ask("q"; .)"#, &ps, &Opts::default());
    let req = &r.log[0];
    let id = only_asset(req).unwrap().to_owned();
    let (head, body) = raw_request(req);
    let has = |h: &str| head.iter().any(|l| l.eq_ignore_ascii_case(h));
    assert!(has("typesafe-danger-zone-images: true"), "{head:?}");
    assert!(head.iter().any(|l| l.to_ascii_lowercase().starts_with("content-type: multipart/form-data; boundary=")));
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains("name=\"request\""), "{body}");
    assert!(body.contains(&format!(r#""assets":{{"{id}":{{"type":"image","part":"{id}"}}}}"#)), "{body}");
    assert!(body.contains(&format!("name=\"{id}\"; filename=\"{id}.png\"")), "{body}");
    assert!(body.contains("Content-Type: image/png") || body.contains("content-type: image/png"), "{body}");
    assert!(body.contains("\u{fffd}PNG"), "the file itself");
    assert!(!body.contains(&ps[0]) && !body.contains("/a.png"), "the path stays here");
    let text = deem_opts(r#"ask("q"; .)"#, "x\n", &Opts::default());
    let (head, _) = raw_request(&text.log[0]);
    assert!(!head.iter().any(|l| l.to_ascii_lowercase().starts_with("typesafe-danger-zone-images")));
    assert!(head.iter().any(|l| l.eq_ignore_ascii_case("content-type: application/json")), "{head:?}");
}

// The binary, over HTTP, against the fake: picture files need no flag, -I
// reads their paths, and a second deem reads the first one's output.
#[test]
fn the_binary_sends_pictures() {
    let s = fake_api();
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", s.url.as_str())];
    let screens = ["screens/loading.png", "screens/orders.png", "screens/status.png"];
    let mut args = vec!["is the page loaded?", "-p"];
    args.extend(screens);
    let r = bin(&args, "", &env, "examples");
    assert_eq!(r.code, 0, "{}", r.stderr);
    let printed: Vec<&str> = r.lines().iter().map(|l| l.split_once('\t').unwrap().1).collect();
    assert_eq!(printed, screens);
    let r = bin(&["-I", "is the page loaded?", "-p"], &format!("{}\n", screens.join("\n")), &env, "examples");
    assert_eq!((r.code, r.lines().len()), (0, 3), "{}", r.stderr);
    let r = bin(&["is the page loaded?", "screens/orders.png", "README.md"], "", &env, "examples");
    assert_eq!(r.code, 2);
    assert_eq!(
        r.stderr,
        "deem: screens/orders.png is a picture and README.md is text: a run reads pictures or text; run them apart\n"
    );
}

// `box(q; v)`: where in the picture, as fractions of its height (top,
// bottom) and width (left, right); asked with the picture's other
// questions, in one request.
#[test]
fn box_finds_where() {
    let ps = files("box", &[("a.png", png(800, 600))]);
    let r = pictures(r#"{seen: ask("is there a mug?"; .), at: box("the mug"; .)}"#, &ps, &Opts::default());
    assert_eq!(r.code, 0, "{}", r.stderr);
    let out: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
    let at = &out["at"];
    let f = |k: &str| at[k].as_f64().unwrap();
    assert!(f("top") < f("bottom") && f("left") < f("right"), "{at}");
    assert_eq!((r.waves(), r.log.len()), (1, 1), "one request");
    let q: serde_json::Value = serde_json::from_str(r.log[0].questions["q1"].get()).unwrap();
    assert_eq!(q, serde_json::json!({"instructions": "the mug", "type": "box"}), "no criteria");
}

// A box off the picture, or upside down, fails its question; a model before
// Jev 1.14 refuses a box with a bare "Invalid request.", so deem says what to
// do.
#[test]
fn box_answers_are_checked() {
    let ps = files("box-check", &[("a.png", png(800, 600))]);
    let input = || open_input(&ps, false).unwrap();
    for (reply, want) in [
        (
            r#"{"type": "box", "box": {"top": 0.5, "bottom": 0.2, "left": 0.1, "right": 0.3}, "confidence": null}"#,
            "not a box: top, bottom, left and right in [0,1]",
        ),
        (r#"{"type": "box", "box": {"top": 0.1, "bottom": 1.2, "left": 0.1, "right": 0.3}}"#, "not a box"),
    ] {
        let body = format!(r#"{{"answers": {{"q0": {reply}}}}}"#);
        let s = stub(move |_, _, _| (200, body.clone()));
        let judge = Arc::new(deem::api::Http::new(&s.url, "k", config().timeout));
        let r = run_input(r#"box("the mug"; .)"#, &[], input(), &Opts::default(), &config(), judge);
        assert!(r.stderr.contains(want), "{}", r.stderr);
    }
    let good = r#"{"answers": {"q0": {"type": "box", "box": {"top": 0.2, "bottom": 0.5, "left": 0.1, "right": 0.3}, "confidence": null}}}"#;
    let s = stub(move |_, _, _| (200, good.into()));
    let judge = Arc::new(deem::api::Http::new(&s.url, "k", config().timeout));
    let r = run_input(r#"judge({q: "the mug", box: true}; .)"#, &[], input(), &Opts::default(), &config(), judge);
    let want = r#"{"value":{"top":0.2,"bottom":0.5,"left":0.1,"right":0.3},"confidence":null,"probabilities":null}"#;
    assert_eq!((r.code, r.stdout.trim()), (0, want), "{}", r.stderr);
    let s = stub(|_, _, _| (400, r#"{"error_type": "api_usage_error", "message": "Invalid request."}"#.into()));
    let judge = Arc::new(deem::api::Http::new(&s.url, "k", config().timeout));
    let r = run_input(r#"box("the mug"; .)"#, &[], input(), &Opts::default(), &config(), judge);
    assert!(r.stderr.contains("a box question needs Jev 1.14 or later: set JEV_MODEL=jev-preview"), "{}", r.stderr);
}

// The plan's three playground examples, as written there, on pictures of
// the same sizes: each is one wave and one request, with every picture one
// asset.
#[test]
fn the_playground_examples() {
    // each its own size, so each its own bytes
    let names: Vec<String> = ["card.png", "lobster.png", "empress.png", "king.png", "phone.jpg"]
        .map(String::from)
        .into_iter()
        .chain((0..6).map(|i| format!("opt{i}.png")))
        .chain((0..5).map(|i| format!("mess{i}.png")))
        .collect();
    let pics: Vec<(String, Vec<u8>)> =
        names.iter().enumerate().map(|(i, n)| (n.clone(), png(400 + i as u32, 400))).collect();
    let named: Vec<(&str, Vec<u8>)> = pics.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
    let ps = files("playground", &named);
    let img = |names: &[(&str, usize)]| names.iter().map(|(n, i)| (n.to_string(), ps[*i].clone())).collect();
    let tarot = r#"{card: ., lobster: $lobster} as $v
  | {celestial:   box("Find the celestial body in `card`."; $v),
     unconscious: box("What in `card` symbolizes the deep unconscious?"; $v),
     parent:      choose("Does `card` evoke the mother or the father?"; {Mother: $empress, Father: $king}; $v),
     next_move:   choose("I just pulled `card`. I should:";
                         {grind: "Grind as hard as I can", screenplay: "Write a screenplay",
                          bartender: "Become a bartender", shrimp_farm: "Start a shrimp farm"}; $v),
     friendly:    ask("Is the `lobster` friendly?"; $v)}"#;
    let opts = Opts { img: img(&[("lobster", 1), ("empress", 2), ("king", 3)]), ..Opts::default() };
    let r = pictures(tarot, &ps[..1], &opts);
    assert_eq!((r.code, r.waves(), r.log.len(), r.log[0].assets.len()), (0, 1, 1, 4), "{}", r.stderr);

    let claim = r#"{claim: "My phone slid off the kitchen counter onto the tile floor.", photo: .} as $c
  | {damage:   choose("How severe is the visible physical damage to the phone in `photo`";
                      {None: "No visible damage", Minor: "Light scratches or scuffs",
                       Moderate: "Localized cracks, chips, or small dents",
                       Severe: "Extensive cracking, deep dents, or missing pieces",
                       Extreme: "Phone is visibly crushed, broken apart, or substantially deformed"}; $c),
     supports: ask("Does `photo` support `claim`?"; $c),
     iphone:   ask("Is the phone in `photo` an iPhone?"; $c),
     android:  ask("Is the phone in `photo` an android phone?"; $c)}"#;
    let r = pictures(claim, &ps[4..5], &Opts::default());
    assert_eq!((r.code, r.waves(), r.log.len(), r.log[0].assets.len()), (0, 1, 1, 1), "{}", r.stderr);
    let state: serde_json::Value = serde_json::from_str(r.log[0].state.get()).unwrap();
    assert_eq!(state["photo"].as_str(), r.log[0].assets.keys().next().map(String::as_str));

    let potato = r#"{best:  choose("Which option best represents this story overall?";
                 {$potatoes, $happy_couple, $mad_couple, $family_dinner, $dunce_hat, $stern_dad}; .),
   messy: rate("Rate the messiness of this situation"; [$m1, $m2, $m3, $m4, $m5]; .)}"#;
    let names = ["potatoes", "happy_couple", "mad_couple", "family_dinner", "dunce_hat", "stern_dad"];
    let mut bound: Vec<(String, String)> =
        names.iter().enumerate().map(|(i, n)| (n.to_string(), ps[5 + i].clone())).collect();
    bound.extend((0..5).map(|i| (format!("m{}", i + 1), ps[11 + i].clone())));
    let story = r#"{"reddit_story": {"title": "I swapped the potatoes", "line_1": "It went badly."}}"#;
    let opts = Opts { img: bound, json_in: true, ..Opts::default() };
    let r = deem_opts(potato, &format!("{story}\n"), &opts);
    assert_eq!((r.code, r.waves(), r.log.len()), (0, 1, 1), "{}", r.stderr);
    assert_eq!(r.log[0].assets.len(), 11);
}

/// A model that knows the example pictures by their files' names: `answer`
/// gets the names of a request's pictures and the question as sent.
struct Screens {
    answer: fn(&[&str], &serde_json::Value) -> serde_json::Value,
    log: std::sync::Mutex<Vec<deem::api::Request>>,
}

impl deem::api::Judge for Screens {
    fn judge(&self, req: &deem::api::Request) -> Result<deem::api::Response, deem::api::ApiError> {
        self.log.lock().unwrap().push(req.clone());
        let names: Vec<&str> = req.assets.values().map(|a| a.path.rsplit('/').next().unwrap()).collect();
        let answers = req
            .questions
            .keys()
            .map(|k| {
                let value = (self.answer)(&names, &question_sent(req, k));
                (k.clone(), deem::api::Answer { value, confidence: None, probabilities: None })
            })
            .collect();
        Ok(deem::api::Response { answers, input_tokens: None, raw: None })
    }
}

/// Run the example program `name` on the example screenshots, with `args`.
fn example(
    name: &str,
    args: &[&str],
    opts: &Opts,
    answer: fn(&[&str], &serde_json::Value) -> serde_json::Value,
) -> Run {
    let screens = ["checkout-error", "loading", "orders", "orders-unstyled", "status"]
        .map(|s| format!("examples/screens/{s}.png"));
    let src = std::fs::read_to_string(format!("examples/.deem/{name}.deem")).unwrap();
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let judge = Arc::new(Screens { answer, log: Default::default() });
    let mut r = run_input(&src, &args, open_input(&screens, false).unwrap(), opts, &config(), judge.clone());
    r.log = judge.log.lock().unwrap().clone();
    r
}

// The `same-page` example: each screenshot with the baseline, in one request
// a screenshot, all in one wave. The baseline against itself is one picture.
#[test]
fn same_page_example() {
    let opts = Opts { img: vec![("baseline".into(), "examples/screens/orders.png".into())], ..Opts::default() };
    let r = example("same-page", &[], &opts, |names, _| serde_json::json!(if names.len() == 1 { 0.9 } else { 0.1 }));
    assert_eq!((r.code, r.stdout.as_str()), (0, "examples/screens/orders.png\n"), "{}", r.stderr);
    // (the requests go out at once, in any order)
    let mut assets: Vec<usize> = r.log.iter().map(|q| q.assets.len()).collect();
    assets.sort();
    assert_eq!((r.waves(), assets), (1, vec![1, 2, 2, 2, 2]));
}

// The `locate` example: whether the thing is there and where, in one
// request a picture, all in one wave; only the pictures that show it print.
#[test]
fn locate_example() {
    let r = example("locate", &["an error message"], &Opts::default(), |names, q| match q["type"].as_str().unwrap() {
        "box" => serde_json::json!({"top": 0.2, "bottom": 0.3, "left": 0.25, "right": 0.95}),
        _ => serde_json::json!(if names == ["checkout-error.png"] { 0.9 } else { 0.1 }),
    });
    let want = r#"{"picture":"examples/screens/checkout-error.png","seen":0.9,"at":{"top":0.2,"bottom":0.3,"left":0.25,"right":0.95}}"#;
    assert_eq!((r.code, r.stdout.trim()), (0, want), "{}", r.stderr);
    let questions: Vec<usize> = r.log.iter().map(|q| q.questions.len()).collect();
    assert_eq!((r.waves(), questions), (1, vec![2, 2, 2, 2, 2]));
}

// The `screen-check` example: what each screenshot shows, all in one wave;
// then where the error is, a second wave for the error page alone. The
// status page mentions errors and shows none.
#[test]
fn screen_check_example() {
    let r = example("screen-check", &[], &Opts::default(), |names, q| match q["type"].as_str().unwrap() {
        "choice" => {
            let options: Vec<&str> = q["criteria"].as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(options, ["broken", "error", "loaded", "loading"]);
            serde_json::json!(match names[0] {
                "checkout-error.png" => "error",
                "loading.png" => "loading",
                "orders-unstyled.png" => "broken",
                _ => "loaded",
            })
        }
        "box" => serde_json::json!({"top": 0.2, "bottom": 0.3, "left": 0.25, "right": 0.95}),
        kind => panic!("unexpected question {kind}"),
    });
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.lines(),
        [
            r#"{"screen":"examples/screens/checkout-error.png","state":"error","error":{"top":0.2,"bottom":0.3,"left":0.25,"right":0.95}}"#,
            r#"{"screen":"examples/screens/loading.png","state":"loading"}"#,
            r#"{"screen":"examples/screens/orders.png","state":"loaded"}"#,
            r#"{"screen":"examples/screens/orders-unstyled.png","state":"broken"}"#,
            r#"{"screen":"examples/screens/status.png","state":"loaded"}"#,
        ]
    );
    let mut questions: Vec<usize> = r.log.iter().map(|q| q.questions.len()).collect();
    questions.sort();
    assert_eq!((r.waves(), questions), (2, vec![1, 1, 1, 1, 1, 1]), "five states, then one box");
}
