//! Odd but legal programs: what they print, and what they cost. Each case
//! asserts waves, requests and questions, not just output.

mod common;

use common::*;
use deem::RunOptions as Opts;

/// Run `program` on the record "a" and check (code, stdout, waves,
/// requests, questions).
fn check(program: &str, want: (i32, &str, usize, usize, usize)) {
    let r = deem(program, "a\n");
    let qs: usize = r.log.iter().map(|q| q.questions.len()).sum();
    assert_eq!((r.code, r.stdout.as_str(), r.waves(), r.log.len(), qs), want, "{program}\n{}", r.stderr);
}

fn a(q: &str) -> String {
    format!("{}\n", noul(q, &text("a")))
}

// Only what the program evaluates is asked. A question it asks and never
// looks at is asked too, as its answer could fail, which is an error there.
#[test]
fn unevaluated_questions_are_never_asked() {
    check(r#"first(ask("q1"; .), ask("q2"; .))"#, (0, &a("q1"), 1, 1, 1));
    check(r#"limit(1; ask("q1"; .), ask("q2"; .))"#, (0, &a("q1"), 1, 1, 1));
    check(r#"limit(0; ask("q"; .))"#, (1, "", 0, 0, 0));
    check(r#"ask("q"; empty)"#, (1, "", 0, 0, 0));
    check(r#"ask("q"; .) as $x | if $x > 2 then ask("never"; .) else "no" end"#, (0, "no\n", 1, 1, 1));
    check(r#"if false then ask("q"; .) else "no" end"#, (0, "no\n", 0, 0, 0));
    check(r#"(ask("q"; .) | "unused"), "after""#, (0, "unused\nafter\n", 1, 1, 1));
}

// Generators in either argument ask one question per output, together: two
// values judged are two requests in one wave, two questions about one value
// one request.
#[test]
fn generator_arguments_ask_together() {
    let r = deem(r#"[ask("q"; (1, 2))] | length"#, "a\n");
    assert_eq!((r.stdout.as_str(), r.waves(), r.log.len(), r.log[0].questions.len()), ("2\n", 1, 2, 1));
    let r = deem(r#"[ask(("q1", "q2"); .)] | length"#, "a\n");
    assert_eq!((r.stdout.as_str(), r.waves(), r.log.len(), r.log[0].questions.len()), ("2\n", 1, 1, 2));
}

// Loops whose questions don't depend on each other still take one wave; a
// loop whose condition is an answer takes one wave per step.
#[test]
fn loops() {
    check(r#"reduce range(5) as $i (0; . + ask("q\($i)"; "x")) | . > 0"#, (0, "true\n", 1, 1, 5));
    check(r#"[foreach range(3) as $i (0; ask("q\($i)"; "x"))] | length"#, (0, "3\n", 1, 1, 3));
    check(
        r#"def f: if . > 0 then ask("q\(.)"; "x"), (. - 1 | f) else empty end; [3 | f] | length"#,
        (0, "3\n", 1, 1, 3),
    );
    let r = deem(r#"0 | until(ask("q\(.)"; "x") > 0.9; . + 1)"#, "a\n");
    let steps: usize = r.stdout.trim().parse().unwrap();
    assert_eq!((r.waves(), r.log.len()), (steps + 1, steps + 1), "a wave per step");
    assert!(noul(&format!("q{steps}"), &text("x")) > 0.9);
}

// A record with more than 64 questions outstanding waits for its first
// answers before queuing more.
#[test]
fn a_record_parks_at_64_unanswered() {
    check(r#"[range(64)] | map(ask(tostring; "x")) | length"#, (0, "64\n", 1, 1, 64));
    check(r#"[range(120)] | map(ask(tostring; "x")) | length"#, (0, "120\n", 2, 2, 120));
}

// An answer can't be a path, and destructuring it fails where it is used.
#[test]
fn answers_are_values_not_paths() {
    let r = deem(r#"path(ask("q"; .))"#, "a\n");
    assert_eq!((r.code, r.log.len()), (2, 0));
    assert_eq!(r.stderr, "deem: record 1: invalid path expression with input \"a\"\n");
    let r = deem(r#"ask("q"; .) as [$x] | $x"#, "a\n");
    assert_eq!((r.code, r.log.len()), (2, 1));
    assert!(r.stderr.starts_with("deem: record 1: cannot index "), "{}", r.stderr);
    check(
        r#"ask("q"; .) | [isnan, type, (. * 100 | floor | . >= 1 and . <= 99)]"#,
        (0, "[false,\"number\",true]\n", 1, 1, 1),
    );
}

// A value that isn't JSON can't be judged; nothing is sent.
#[test]
fn only_json_is_judged() {
    for ctx in ["nan", "infinite", "-infinite"] {
        let r = deem(&format!(r#"ask("q"; {ctx})"#), "a\n");
        assert_eq!((r.code, r.log.len()), (2, 0), "{ctx}");
        assert!(r.stderr.contains("the value to judge isn't JSON"), "{}", r.stderr);
    }
}

// CRLF input: the model sees the line without its `\r`.
#[test]
fn crlf_lines() {
    let r = deem(r#"ask("q"; .)"#, "a\r\nb\r\n");
    assert_eq!(r.stdout, format!("{}{}", a("q"), format_args!("{}\n", noul("q", &text("b")))));
    assert_eq!((r.waves(), r.log.len()), (1, 2));
}

// A spec is checked before anything is sent: only its six fields, a string
// rubric, and at least two distinct options when `among` is given.
#[test]
fn spec_errors() {
    for (spec, want) in [
        (
            r#"{q: "q", amoung: ["x", "y"]}"#,
            r#"unknown field "amoung"; a spec has q, among, levels, box, yes, no, like, unlike"#,
        ),
        (r#"{q: "q", among: []}"#, "`among` is empty; give at least two options"),
        (r#"{q: "q", among: {}}"#, "`among` is empty; give at least two options"),
        (r#"{q: "q", among: {x: "only"}}"#, "a question with one option has one answer; give at least two"),
        (r#"{q: "q", among: ["x", 1]}"#, "`among` must be an array of strings, or an object of descriptions"),
        (r#"{q: "q", among: "x,y"}"#, "`among` must be an array of strings, or an object of descriptions"),
        (r#"{q: "q", among: {x: nan, y: 1}}"#, r#"the description of "x" can't be sent as JSON: NaN"#),
        (r#"{q: "q", levels: ["low"]}"#, "1 levels; a scale has 2 to 10"),
        (r#"{q: "q", levels: [range(11)]}"#, "11 levels; a scale has 2 to 10"),
        (r#"{q: "q", levels: "low,high"}"#, "`levels` must be an array, lowest level first"),
        (r#"{q: "q", levels: ["a", nan]}"#, "level 1 can't be sent as JSON: NaN"),
        (
            r#"{q: "q", levels: ["a", "b"], among: ["x", "y"]}"#,
            "`levels` is a scale; it doesn't go with `among`, `yes` or `no`",
        ),
        (r#"{q: "q", levels: ["a", "b"], yes: "y"}"#, "`levels` is a scale; it doesn't go with `among`, `yes` or `no`"),
        (r#"{q: "q", box: "yes"}"#, "`box` is true or false"),
        (r#"{q: "q", box: true, among: ["x", "y"]}"#, "a box question takes no `among`"),
        (r#"{q: "q", box: true, like: ["x"]}"#, "a box question takes no `like`"),
        (r#"{q: "q", box: true}"#, "a box is found in a picture, and the value has none"),
        (r#"{q: "q", among: ["x"]}"#, "a question with one option has one answer; give at least two"),
        (r#"{q: "q", among: ["x", "y", "x"]}"#, r#"`among` lists "x" twice"#),
        (r#"{q: "q", among: [range(300) | tostring]}"#, "300 options; the API takes at most 255"),
        (
            r#"{q: "q", among: ["x", "y"], yes: "y"}"#,
            "`yes` is for a yes/no question; a choice or a scale describes each option or level instead",
        ),
        (
            r#"{q: "q", levels: ["a", "b"], unlike: ["u"]}"#,
            "`unlike` is for a yes/no question; a choice or a scale describes each option or level instead",
        ),
        (r#"{q: "q", yes: 1}"#, "`yes` must be a string"),
        (r#"{q: "q", no: {}}"#, "`no` must be a string"),
    ] {
        let r = deem(&format!("judge({spec}; .)"), "a\n");
        assert_eq!(
            (r.code, r.stderr.as_str(), r.log.len()),
            (2, format!("deem: record 1: judge: {want}\n").as_str(), 0)
        );
    }
    check(r#"choose("c"; []; .)"#, (2, "", 0, 0, 0));
    // absent or null is none
    check(r#"judge({q: "q", among: null, yes: null, no: "n"}; .).value | type"#, (0, "number\n", 1, 1, 1));
}

// `among` as an object: each option goes with what it means, and the
// answer is still a name.
#[test]
fn options_with_descriptions() {
    let p = r#"choose("topic?"; {billing: "invoices, refunds", bug: {broken: true}, other: null}; .)"#;
    let r = deem(p, "a\n");
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(["billing\n", "bug\n", "other\n"].contains(&r.stdout.as_str()), "{}", r.stdout);
    let body: serde_json::Value = serde_json::from_str(&r.log[0].body()).unwrap();
    let want = serde_json::json!({"billing": "invoices, refunds", "bug": {"broken": true}, "other": null});
    assert_eq!(body["questions"]["q0"]["criteria"], want);
    assert_eq!((r.waves(), r.log.len()), (1, 1));
    // the description is part of the question: a different one is asked again
    let p =
        r#"[choose("t?"; {a: "x", b: "y"}; .), choose("t?"; {a: "x", b: "z"}; .), choose("t?"; {a: "x", b: "y"}; .)]"#;
    let r = deem(p, "a\n");
    assert_eq!((r.code, r.waves(), r.log.len()), (0, 1, 1));
    assert_eq!(r.log[0].questions.len(), 2);
}

// `rate`: a Score question, its levels sent in order as the criteria; the
// value is a number on the scale.
#[test]
fn rate_is_a_score() {
    let r = deem(r#"rate("how urgent?"; ["can wait", {when: "this week"}, "now"]; .)"#, "a\nb\n");
    assert_eq!(r.code, 0, "{}", r.stderr);
    for line in r.lines() {
        let x: f64 = line.parse().unwrap();
        assert!((0.0..=2.0).contains(&x), "{x}");
    }
    let body: serde_json::Value = serde_json::from_str(&r.log[0].body()).unwrap();
    let q = &body["questions"]["q0"];
    assert_eq!(
        (q["type"].as_str(), &q["criteria"]),
        (Some("score"), &serde_json::json!(["can wait", {"when": "this week"}, "now"]))
    );
    assert_eq!((r.waves(), r.log.len()), (1, 2));
}

/// A model scripted by keyword: `answer(record, question)`, where the
/// record is the judged value (a string as itself, anything else as JSON).
struct Script {
    answer: fn(&str, &str) -> serde_json::Value,
    log: std::sync::Mutex<Vec<deem::api::Request>>,
}

impl deem::api::Judge for Script {
    fn judge(&self, req: &deem::api::Request) -> Result<deem::api::Response, deem::api::ApiError> {
        self.log.lock().unwrap().push(req.clone());
        let answers = asked(req)
            .into_iter()
            .enumerate()
            .map(|(i, (rec, q))| {
                let rec = rec.as_str().map_or_else(|| rec.to_string(), str::to_owned);
                let value = (self.answer)(&rec, &q);
                (format!("q{i}"), deem::api::Answer { value, confidence: None, probabilities: None })
            })
            .collect();
        Ok(deem::api::Response { answers, input_tokens: None, raw: None })
    }
}

/// Run the program file at `path` over `input` against the script.
fn program(path: &str, input: &str, opts: deem::RunOptions, answer: fn(&str, &str) -> serde_json::Value) -> Run {
    let src = std::fs::read_to_string(path).unwrap();
    let script = std::sync::Arc::new(Script { answer, log: Default::default() });
    let mut r = run_with(&src, input, &opts, &config(), script.clone());
    r.log = script.log.lock().unwrap().clone();
    r
}

fn score(yes: bool) -> serde_json::Value {
    serde_json::json!(if yes { 0.9 } else { 0.1 })
}

/// The questions in each request, fewest first: senders send a wave's
/// requests in any order.
fn per_request(r: &Run) -> Vec<usize> {
    let mut n: Vec<usize> = r.log.iter().map(|q| q.questions.len()).collect();
    n.sort();
    n
}

// A decision tree in code: every ticket's branch is chosen in wave 1, and
// the branches' follow-ups (two refunds, a platform, an urgency) share
// wave 2, a request for each ticket that has one.
#[test]
fn router() {
    let input = lines([
        r#"{"id":"T-1","body":"I was charged twice this month and want a refund"}"#,
        r#"{"id":"T-2","body":"The app crashes every time I open settings on Android"}"#,
        r#"{"id":"T-3","body":"Why did my invoice go up after the plan change?"}"#,
        r#"{"id":"T-4","body":"WIN FREE MONEY click here"}"#,
        r#"{"id":"T-5","body":"How do I export my data?"}"#,
    ]);
    let r = program("tests/deem/router.deem", &input, json(), |rec, q| match q {
        "what kind of ticket is this?" => serde_json::json!(if rec.contains("refund") || rec.contains("invoice") {
            "billing"
        } else if rec.contains("crashes") {
            "bug"
        } else if rec.contains("FREE") {
            "spam"
        } else {
            "question"
        }),
        "is the customer asking for a refund?" => score(rec.contains("refund")),
        "which platform?" => serde_json::json!(if rec.contains("Android") { "android" } else { "web" }),
        "does it stop the user completely?" => score(rec.contains("every time")),
        q => panic!("unexpected question {q:?}"),
    });
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    assert_eq!(
        r.lines(),
        [
            r#"{"id":"T-1","route":"refunds"}"#,
            r#"{"id":"T-2","route":"bugs-android","urgent":true}"#,
            r#"{"id":"T-3","route":"billing"}"#,
            r#"{"id":"T-5","route":"faq"}"#,
        ]
    );
    assert_eq!((r.waves(), per_request(&r)), (2, vec![1, 1, 1, 1, 1, 1, 1, 2]));
}

// The `decision-tree` example: a decision tree as data, walked recursively.
// One wave per level, a request for each record in each.
#[test]
fn decision_tree() {
    let input = lines([
        "ERROR login bypass via token reuse",
        "ERROR db pool exhausted",
        "WARN disk 85% full",
        "INFO user signed in",
    ]);
    let r = program("examples/.deem/decision-tree.deem", &input, Default::default(), |rec, q| match q {
        "is this an error?" => score(rec.starts_with("ERROR")),
        "is it about security?" => score(rec.contains("bypass")),
        "is it a warning?" => score(rec.starts_with("WARN")),
        q => panic!("unexpected question {q:?}"),
    });
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    let actions: Vec<String> =
        r.lines().iter().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["action"].to_string()).collect();
    assert_eq!(actions, [r#""page-security""#, r#""page-oncall""#, r#""ticket""#, r#""ignore""#]);
    assert_eq!((r.waves(), per_request(&r)), (2, vec![1; 8]));
}

// A weighted rubric: all criteria in wave 1, the follow-up only for what
// scored well, in wave 2.
#[test]
fn rubric() {
    let input = lines([
        "the checkout page keeps spinning forever",
        "love the new dark mode",
        "the android app logs me out every hour",
    ]);
    let r = program("tests/deem/rubric.deem", &input, Default::default(), |rec, q| {
        let praise = rec.starts_with("love");
        match q {
            "is it actionable?" => score(!praise),
            "is it specific?" => serde_json::json!(if praise { 0.3 } else { 0.8 }),
            "is it about the product?" => score(true),
            "which team owns this?" => serde_json::json!(if rec.contains("checkout") { "payments" } else { "mobile" }),
            q => panic!("unexpected question {q:?}"),
        }
    });
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    // (3*0.9 + 2*0.8 + 0.9) / 6 = 0.8667; (3*0.1 + 2*0.3 + 0.9) / 6 = 0.3
    assert_eq!(
        r.lines(),
        [
            r#"{"text":"the checkout page keeps spinning forever","score":87,"team":"payments"}"#,
            r#"{"text":"love the new dark mode","score":30}"#,
            r#"{"text":"the android app logs me out every hour","score":87,"team":"mobile"}"#,
        ]
    );
    assert_eq!((r.waves(), per_request(&r)), (2, vec![1, 1, 3, 3, 3]));
}

// One slurped record, one question per source IP, each seeing only its
// own lines: one wave.
#[test]
fn bruteforce() {
    let input = std::fs::read_to_string("examples/auth.log").unwrap();
    let opts = deem::RunOptions { slurp: true, ..Default::default() };
    let r = program("tests/deem/bruteforce.deem", &input, opts, |rec, q| match q {
        "do these lines show a password-guessing attack?" => score(rec.matches("Failed password").count() >= 3),
        q => panic!("unexpected question {q:?}"),
    });
    assert_eq!((r.code, r.stderr.as_str(), r.stdout.as_str()), (0, "", "[\"203.0.113.9\"]\n"));
    let seen: Vec<usize> = r.asked().concat().iter().map(|(rec, _)| rec.as_array().unwrap().len()).collect();
    assert_eq!(seen, [1, 6], "10.0.0.4's one line, then 203.0.113.9's six");
    assert_eq!((r.waves(), r.log.len()), (1, 2), "a request for each IP's lines");
}

// Arguments and named values are bound as variables, never pasted in.
#[test]
fn arguments_stay_values() {
    let echo = "module {input: \"each\"}; def main($a; $b): [$a, $b, .] | tojson;";
    let tricky = [r#"say "hi""#, r#"\(error("x"))"#];
    let r = deem_args(echo, &tricky, "rec\n", &Default::default());
    let got: Vec<String> = serde_json::from_str(r.stdout.trim()).unwrap();
    assert_eq!(got, [tricky[0], tricky[1], "rec"]);
    let r = deem_args("def main($q): ask($q; .);", &[r#"is "this" \(x)?"#], "a\n", &Default::default());
    assert_eq!((r.code, r.stdout.clone()), (0, a(r#"is "this" \(x)?"#)), "{}", r.stderr);
    let r = deem_args("def main: 1;", &[], "a\n", &Default::default());
    assert_eq!(r.stdout, "1\n");
}

#[test]
fn arity_and_shape_errors() {
    let opts = Default::default();
    let err = |p: &str, args: &[&str]| deem_args(p, args, "a\n", &opts).stderr;
    assert_eq!(err("def main($q): .;", &[]), "deem: the program takes 1 argument (q), got 0\n");
    assert_eq!(err("def main($a; $b): .;", &["x"]), "deem: the program takes 2 arguments (a, b), got 1\n");
    assert_eq!(err(".", &["x"]), "deem: a program without main takes no arguments, got 1\n");
    let e = err("def main: .; main", &[]);
    assert!(e.starts_with("deem: 1:5: a program that defines main is only definitions"), "{e}");
    let e = err("def f: .;", &[]);
    assert!(e.contains("needs `def main: …;`"), "{e}");
    // Global values are visible inside definitions.
    let r = deem_args("module {defaults: {n: 2}}; def f: $n + 1; def main($q): [f, $q];", &["x"], "a\n", &opts);
    assert_eq!(r.stdout, "[3,\"x\"]\n", "{}", r.stderr);
}

// `--rawfile NAME FILE`, as in jq: `$NAME` is the file's text, and the
// program needn't declare it.
#[test]
fn rawfile_binds_text() {
    let dir = std::env::temp_dir().join(format!("deem-rawfile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("rubric.txt");
    std::fs::write(&file, "\u{feff}urgent: \"now\"\ncaf\u{e9}\n").unwrap();
    let path = file.to_str().unwrap().to_owned();
    let with = |program: &str, files: &[(&str, &str)]| {
        let rawfile = files.iter().map(|(n, f)| (n.to_string(), f.to_string())).collect();
        deem_opts(program, "a\n", &deem::RunOptions { rawfile, ..Default::default() })
    };
    let r = with("$rubric | tojson", &[("rubric", &path)]);
    assert_eq!((r.code, r.stdout.as_str()), (0, "\"urgent: \\\"now\\\"\\ncafé\\n\"\n"), "{}", r.stderr);
    // it reaches the model only where the program puts it
    let r = with(r#"ask("is this urgent, by `rubric`?"; {rubric: $rubric, msg: .})"#, &[("rubric", &path)]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.log[0].state.get().contains("urgent: \\\"now\\\""), "{}", r.log[0].state.get());
    let missing = dir.join("missing.txt");
    let r = with("$x", &[("x", missing.to_str().unwrap())]);
    assert!(r.stderr.starts_with("deem: --rawfile: ") && r.stderr.contains("missing.txt"), "{}", r.stderr);
    assert_eq!(with("$x", &[("x", &path), ("x", &path)]).stderr, "deem: $x is set twice\n");
    assert_eq!(
        with("$a-b", &[("a-b", &path)]).stderr,
        "deem: $a-b isn't a name a program can use; use letters, digits and _\n"
    );
    let declared = r#"module {defaults: {x: "d"}}; $x"#;
    assert_eq!(with(declared, &[("x", &path)]).stderr, "deem: $x is the program's own; bind another name\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn named_values() {
    let route = r#"module {input: "each", defaults: {urgent: 0.8, who: "me", n: 3}};
[$urgent, $who, $n]"#;
    let with = |named: &[(&str, &str)]| {
        let opts = deem::RunOptions {
            named: named.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
            ..Default::default()
        };
        deem_opts(route, "a\n", &opts)
    };
    assert_eq!(with(&[]).stdout, "[0.8,\"me\",3]\n");
    let r = with(&[("urgent", "0.9"), ("who", "\"you\" \\(x)"), ("n", "4")]);
    assert_eq!(r.stdout, "[0.9,\"\\\"you\\\" \\\\(x)\",4]\n", "{}", r.stderr);
    assert_eq!(
        with(&[("urgnet", "0.9")]).stderr,
        "deem: the program has no value \"urgnet\" (it has: urgent, who, n)\n"
    );
    assert_eq!(with(&[("urgent", "abc")]).stderr, "deem: --arg urgent: \"abc\" is not a number\n");
    assert_eq!(with(&[("urgent", "nan")]).stderr, "deem: --arg urgent: \"nan\" is not a number\n");
    let r = deem_opts(
        ".",
        "a\n",
        &deem::RunOptions { named: vec![("threshold".into(), "0.5".into())], ..Default::default() },
    );
    assert_eq!(r.stderr, "deem: the program has no value \"threshold\" (it has none)\n");
    // -t is --arg threshold, and the -a program declares it.
    let opts = deem::RunOptions { named: vec![("threshold".into(), "0.99".into())], ..Default::default() };
    let r = deem_args(deem::programs::FILTER, &["q"], "a\nb\n", &opts);
    assert_eq!((r.code, r.stdout.as_str()), (1, ""));
    let r = deem_args(deem::programs::FILTER, &["q"], "a\nb\n", &Default::default());
    let keep: String = ["a", "b"].iter().filter(|x| noul("q", &text(x)) > 0.5).map(|x| format!("{x}\n")).collect();
    assert_eq!(r.stdout, keep);
}

#[test]
fn directive_errors() {
    let err = |p: &str| deem(p, "a\n").stderr;
    for (p, want) in [
        ("module {input: \"al\"}; .", "1:9: input is \"each\" or \"all\""),
        (
            "module {inputs: \"all\"}; .",
            "1:9: unknown key \"inputs\" in the module directive (it takes input and defaults)",
        ),
        ("module {defaults: {t: [1]}}; .", "1:20: the default of t is a number or a string"),
        ("module {defaults: {t: .5}}; .", "write 0.5, not .5"),
        ("module {defaults: 1}; .", "defaults is an object"),
        ("module {defaults: {\"a b\": 1}}; .", "a named value is an identifier, not \"a b\""),
        ("module {defaults: {q: 1}}; def main($q): .;", "q is both a named value and an argument of main"),
        ("module \"x\"; .", "1:8: the module directive is an object"),
    ] {
        let e = err(p);
        assert!(e.starts_with("deem: ") && e.contains(want), "{p}: {e}");
    }
    assert_eq!(deem("module {input: \"each\", defaults: {t: -1}}; $t", "a\n").stdout, "-1\n");
    let r = deem_args("module {input: \"each\"} ;\n# c\ndef main($q): $q;", &["x"], "a\n", &Default::default());
    assert_eq!(r.stdout, "x\n", "{}", r.stderr);
}

fn builtin(name: &str) -> &'static str {
    deem::programs::BUILTIN.iter().find(|b| b.0 == name).unwrap().1
}

fn some_lines(n: usize) -> Vec<String> {
    records48().into_iter().cycle().take(n).enumerate().map(|(i, l)| format!("{l} #{i}")).collect()
}

// The question forms: one wave, a request per line.
#[test]
fn question_forms_one_wave() {
    let input = lines(some_lines(48));
    let opts = Default::default();
    for (program, args) in [
        (deem::programs::FILTER, vec!["q"]),
        (deem::programs::SCORE, vec!["q"]),
        (deem::programs::LABEL, vec!["q", "a,b"]),
        (deem::programs::RATE, vec!["q", "low, high"]),
    ] {
        let r = deem_args(program, &args, &input, &opts);
        assert_eq!((r.code, r.stderr.as_str(), r.waves(), r.log.len()), (0, "", 1, 48), "{program}");
    }
    let r = deem_args(deem::programs::SCORE, &["q"], "{\"a\":1}\n", &json());
    assert_eq!(r.stdout, format!("{}\t{{\"a\":1}}\n", noul("q", "{\"a\":1}")), "records are printed as read");
    let r = deem_args(deem::programs::LABEL, &["q", "yes,no"], "x\n", &opts);
    assert!(["yes\tx\n", "no\tx\n"].contains(&r.stdout.as_str()), "{}", r.stdout);
    // a level is a number on the scale, 0 the first; the levels are sent
    // as they are named, the spaces around a comma dropped
    let r = deem_args(deem::programs::RATE, &["q", "can wait , this week,now"], "x\n", &opts);
    let (level, line) = r.stdout.trim_end().split_once('\t').unwrap();
    assert!((0.0..=2.0).contains(&level.parse::<f64>().unwrap()) && line == "x", "{}", r.stdout);
    let sent = question_sent(&r.log[0], "q0");
    assert_eq!(
        (sent["type"].as_str(), &sent["criteria"]),
        (Some("score"), &serde_json::json!(["can wait", "this week", "now"]))
    );
}

// disagree, histogram and mix: every question in one wave.
#[test]
fn analysis_programs_one_wave() {
    let recs = some_lines(50);
    let input = lines(&recs);
    let opts = Default::default();
    let r = deem_args(builtin("histogram"), &["q"], &input, &opts);
    assert_eq!((r.code, r.waves(), r.stats.questions, r.log.len()), (0, 1, 51, 51), "50 lines and the baseline");
    // ten bins, then the dashes, the lines above the threshold, the baseline
    let bins = &r.lines()[..10];
    let counts: usize = bins.iter().map(|l| l.split('\t').nth(1).unwrap().parse::<usize>().unwrap()).sum();
    assert_eq!((r.lines().len(), counts), (13, 50));
    assert!(bins.iter().all(|l| l.starts_with("0.")));

    let r = deem_args(builtin("mix"), &["q", "bug,billing,other"], &input, &opts);
    assert_eq!((r.code, r.waves(), r.stats.questions), (0, 1, 51));
    let rows: Vec<&str> = r.lines().iter().map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(rows[..3], ["bug", "billing", "other"]);

    let r = deem_args(builtin("disagree"), &["q1", "q2"], &input, &opts);
    assert_eq!((r.code, r.waves(), r.stats.questions), (0, 1, 100));
    let differ = recs.iter().filter(|x| (noul("q1", &text(x)) > 0.5) != (noul("q2", &text(x)) > 0.5)).count();
    assert_eq!(r.lines().len(), differ);
}

// group: a wave per round, and a round per group (the last one asks
// nothing if it is a single line).
#[test]
fn group_is_a_wave_per_group() {
    let recs = some_lines(40);
    let unlimited = Opts { named: vec![("groups".into(), "0".into())], ..Default::default() };
    let r = deem_args(builtin("group"), &["same?"], &lines(&recs), &unlimited);
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    let groups: Vec<usize> = r.lines().iter().map(|l| l.split_once('\t').unwrap().0.parse().unwrap()).collect();
    let texts: Vec<&str> = r.lines().iter().map(|l| l.split_once('\t').unwrap().1).collect();
    assert!(groups.is_sorted(), "group by group");
    let at = |t: &&str| recs.iter().position(|x| x.as_str() == *t).unwrap();
    for g in 1..=*groups.iter().max().unwrap() {
        let mine: Vec<usize> = texts.iter().zip(&groups).filter(|(_, n)| **n == g).map(|(t, _)| at(t)).collect();
        assert!(mine.is_sorted(), "group {g}: in input order");
    }
    let mut all = texts.clone();
    all.sort();
    let mut want = recs.clone();
    want.sort();
    assert_eq!(all, want, "every line once");
    let n = *groups.iter().max().unwrap();
    let last_alone = groups.iter().filter(|&&g| g == n).count() == 1;
    assert!(n > 1);
    assert_eq!(r.waves(), n - usize::from(last_alone));
}

// group's limit bounds its cost: with `groups=3`, three rounds, three
// waves, and a request for each line left in each; the rest print under
// `-`, in input order.
#[test]
fn group_stops_at_its_limit() {
    let recs = some_lines(40);
    let three = Opts { named: vec![("groups".into(), "3".into())], ..Default::default() };
    let r = deem_args(builtin("group"), &["same?"], &lines(&recs), &three);
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    let (grouped, rest): (Vec<&str>, Vec<&str>) = r.lines().into_iter().partition(|l| !l.starts_with("-\t"));
    assert!(grouped.iter().all(|l| ["1\t", "2\t", "3\t"].iter().any(|g| l.starts_with(g))), "{grouped:?}");
    assert!(!rest.is_empty(), "40 varied lines make more than three groups");
    let rest: Vec<&str> = rest.iter().map(|l| &l[2..]).collect();
    let at = |t: &&str| recs.iter().position(|x| x.as_str() == *t).unwrap();
    assert!(rest.iter().map(at).is_sorted(), "the rest in input order");
    assert_eq!(grouped.len() + rest.len(), 40, "every line once");
    let size = |g: &str| grouped.iter().filter(|l| l.starts_with(&format!("{g}\t"))).count();
    let left = [40, 40 - size("1"), 40 - size("1") - size("2")];
    let want: usize = left.iter().map(|n| n - 1).sum();
    let asked: usize = r.log.iter().map(|q| q.questions.len()).sum();
    assert_eq!((r.waves(), r.log.len(), asked), (3, want, want), "a request for each line left, each round");
}
