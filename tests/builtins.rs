//! The programs that come with deem, against a model whose answers are set
//! by hand: exactly what each prints, all of it on stdout, worked out from
//! what the README and the program's comments promise. Their cost in waves
//! is in tests/programs.rs; that they run the same under jaq is in
//! tests/jq_diff.rs.

mod common;

use common::*;
use deem::api::{Answer, ApiError, Judge, Request, Response};
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;

/// A model that answers each (record, question) with `answer`.
struct Script(fn(&Json, &str) -> Answer);

impl Judge for Script {
    fn judge(&self, req: &Request) -> Result<Response, ApiError> {
        let answers = asked(req).iter().enumerate().map(|(i, (rec, q))| (format!("q{i}"), (self.0)(rec, q))).collect();
        Ok(Response { answers, input_tokens: None, raw: None })
    }
}

fn score(v: f64) -> Answer {
    Answer { value: json!(v), confidence: None, probabilities: None }
}

fn choice(value: &str, probabilities: &[(&str, f64)]) -> Answer {
    let p: BTreeMap<String, f64> = probabilities.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    Answer { value: json!(value), confidence: None, probabilities: Some(p) }
}

/// `deem run ARGS…` with `stdin`, against `answer`.
fn run(answer: fn(&Json, &str) -> Answer, args: &[&str], stdin: &str) -> Run {
    let s = api(Script(answer));
    let args: Vec<&str> = ["run"].iter().chain(args).copied().collect();
    bin(&args, stdin, &[("JEV_API_KEY", "sk"), ("JEV_BASE_URL", &s.url)], ".")
}

fn bar(n: usize) -> String {
    "█".repeat(n)
}

// Bins are tenths, the top one closed: 0.1 goes in 0.1, 1.0 in 0.9. A bar
// is the bin's share of the lines, 40 blocks for all of them, so a file
// twice as long draws the same bars. After a line of dashes come the lines
// above the threshold, strictly above, and the baseline: an empty line's
// score.
#[test]
fn histogram() {
    fn answer(rec: &Json, _: &str) -> Answer {
        let line = rec.as_str().unwrap();
        score(match line.split('#').next().unwrap() {
            "" => 0.123,
            "a" => 0.0,
            "b" => 0.05,
            "c" => 0.1,
            "d" => 0.5,
            "e" => 0.95,
            _ => 1.0,
        })
    }
    let want = [
        format!("0.0\t2\t{}", bar(13)), // 2 of 6: 13.3 blocks
        format!("0.1\t1\t{}", bar(7)),  // 1 of 6: 6.7
        "0.2\t0\t".into(),
        "0.3\t0\t".into(),
        "0.4\t0\t".into(),
        format!("0.5\t1\t{}", bar(7)),
        "0.6\t0\t".into(),
        "0.7\t0\t".into(),
        "0.8\t0\t".into(),
        format!("0.9\t2\t{}", bar(13)),
        "-".repeat(20),
        format!("above 0.5\t2\t{}", bar(13)),
        "baseline\t0.12".into(),
    ];
    let r = run(answer, &["histogram", "q"], "a\nb\nc\nd\ne\nf\n");
    assert_eq!(r.lines(), want);
    assert_eq!(r.stderr, "");

    let twice = run(answer, &["histogram", "q"], "a#1\nb#1\nc#1\nd#1\ne#1\nf#1\na#2\nb#2\nc#2\nd#2\ne#2\nf#2\n");
    let rows = |r: &Run| r.lines().iter().map(|l| l.split('\t').nth(2).unwrap_or("").to_owned()).collect::<Vec<_>>();
    assert_eq!(rows(&twice), rows(&r), "bars are shares");

    let r = run(answer, &["histogram", "-t", "0.9", "q"], "a\nb\nc\nd\ne\nf\n");
    assert_eq!(r.lines()[11], format!("above 0.9\t2\t{}", bar(13)));
}

// Rows follow the options as given. A close call is a line whose winner is
// less than 0.15 ahead of the runner-up; the pair is named in the options'
// order, and pairs are listed most frequent first, behind a line of dashes;
// with no close call that part is left out. The baseline, an empty line's
// probabilities, comes last, highest first.
#[test]
fn mix() {
    fn answer(rec: &Json, _: &str) -> Answer {
        match rec.as_str().unwrap() {
            "l1" => choice("bug", &[("bug", 0.9), ("billing", 0.05), ("other", 0.05)]),
            "l2" => choice("billing", &[("billing", 0.5), ("other", 0.4), ("bug", 0.1)]),
            "l3" => choice("other", &[("other", 0.45), ("billing", 0.4), ("bug", 0.15)]),
            "l4" => choice("bug", &[("bug", 0.5), ("other", 0.36), ("billing", 0.14)]),
            "l5" => choice("billing", &[("billing", 0.7), ("bug", 0.2), ("other", 0.1)]),
            "l6" => choice("billing", &[("billing", 0.55), ("bug", 0.38), ("other", 0.07)]), // 0.17 ahead: clear
            _ => choice("other", &[("other", 0.41), ("bug", 0.33), ("billing", 0.26)]),
        }
    }
    let r = run(answer, &["mix", "q", "bug,billing,other"], "l1\nl2\nl3\nl4\nl5\nl6\n");
    let (dashes, baseline) = ("-".repeat(20), ["baseline other\t0.41", "baseline bug\t0.33", "baseline billing\t0.26"]);
    let mut want = vec![
        format!("bug\t2\t{}", bar(13)),
        format!("billing\t3\t{}", bar(20)),
        format!("other\t1\t{}", bar(7)),
        dashes.clone(),
        format!("close billing/other\t2\t{}", bar(13)),
        format!("close bug/other\t1\t{}", bar(7)),
        dashes.clone(),
    ];
    want.extend(baseline.map(str::to_owned));
    assert_eq!(r.lines(), want);
    assert_eq!(r.stderr, "");
    let r = run(answer, &["mix", "q", "bug,billing,other"], "l1\nl5\n");
    let mut want =
        vec![format!("bug\t1\t{}", bar(20)), format!("billing\t1\t{}", bar(20)), "other\t0\t".into(), dashes];
    want.extend(baseline.map(str::to_owned));
    assert_eq!(r.lines(), want, "no close call: no such part");
}

// The lines where the two wordings decide differently at the threshold
// (0.5 is not above it), the biggest gap first; a JSON record as JSON.
#[test]
fn disagree() {
    fn answer(rec: &Json, q: &str) -> Answer {
        let key = rec.as_str().map_or_else(|| rec.to_string(), str::to_owned);
        let (a, b) = match key.as_str() {
            "a" => (0.9, 0.1),
            "b" => (0.6, 0.7),
            "c" => (0.4, 0.55),
            "d" => (0.5, 0.51),
            "e" => (0.2, 0.3),
            _ => (0.8, 0.3),
        };
        score(if q == "q1" { a } else { b })
    }
    let r = run(answer, &["disagree", "q1", "q2"], "a\nb\nc\nd\ne\n");
    assert_eq!(r.lines(), ["0.9\t0.1\ta", "0.4\t0.55\tc", "0.5\t0.51\td"]);
    assert_eq!(r.stderr, "");
    let r = run(answer, &["disagree", "-t", "0.35", "q1", "q2"], "a\nb\nc\nd\ne\n");
    assert_eq!(r.lines(), ["0.9\t0.1\ta"], "c and d agree at 0.35");
    let r = run(answer, &["disagree", "-J", "q1", "q2"], "{\"x\": 1}\n");
    assert_eq!(r.lines(), ["0.8\t0.3\t{\"x\":1}"]);
}

// The earliest line left starts each group, and every line left is asked
// whether it is the same as that one: `{a: first, b: line}`. Lines print
// group by group, each with its group's number, in input order within it.
#[test]
fn group() {
    fn answer(rec: &Json, _: &str) -> Answer {
        let topic = |s: &Json| {
            let s = s.as_str().unwrap();
            if s.contains("charge") {
                "money".to_owned()
            } else if s.contains("log") {
                "login".to_owned()
            } else {
                s.to_owned()
            }
        };
        score(if topic(&rec["a"]) == topic(&rec["b"]) { 0.9 } else { 0.1 })
    }
    let input = "charged twice\nlogin fails\ndouble charge\ncannot log in\nlove it\n";
    let r = run(answer, &["group", "same?"], input);
    assert_eq!(r.lines(), ["1\tcharged twice", "1\tdouble charge", "2\tlogin fails", "2\tcannot log in", "3\tlove it"]);
    assert_eq!(r.stderr, "");
    let r = run(answer, &["group", "-t", "0.95", "same?"], input);
    let groups: Vec<&str> = r.lines().iter().map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(groups, ["1", "2", "3", "4", "5"], "above the threshold nothing is the same");
    // past the limit, the lines left print under `-`, in input order
    let r = run(answer, &["group", "--arg", "groups", "1", "same?"], input);
    assert_eq!(r.lines(), ["1\tcharged twice", "1\tdouble charge", "-\tlogin fails", "-\tcannot log in", "-\tlove it"]);
}
