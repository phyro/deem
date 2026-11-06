//! A record runs again from the start each time it has new answers. What a
//! run does before its first unanswered question is done once, however
//! many times the record runs: its outputs, its lines on stderr, the times
//! it reads. Each test asserts waves and questions too.

mod common;

use common::*;
use deem::RunOptions as Opts;
use std::sync::Arc;
use std::time::Duration;

// Outputs before a question print once; the run that has the answer
// prints the rest.
#[test]
fn outputs_print_once() {
    let r = deem(r#"range(3), (ask("q"; .) > 0), range(3; 5)"#, "x\n");
    assert_eq!((r.code, r.stdout.as_str()), (0, "0\n1\n2\ntrue\n3\n4\n"), "{}", r.stderr);
    assert_eq!((r.waves(), r.stats.questions), (1, 1));
}

// `debug` and `stderr` write once, in order, from the first run that gets
// to them with every answer before them.
#[test]
fn stderr_is_written_once() {
    let s = fake_api();
    let env = [("JEV_API_KEY", "sk"), ("JEV_BASE_URL", s.url.as_str())];
    let program = r#""a" | debug | ask("q1"; .) as $x | ("b" | debug) | ($x > 0) as $_
        | ("c\n" | stderr) | ask("q2"; .) | (. > 0) | debug"#;
    let r = bin(&["run", program], "x\n", &env, ".");
    assert_eq!((r.code, r.stdout.as_str()), (0, "true\n"), "{}", r.stderr);
    assert_eq!(r.stderr, "[\"DEBUG:\",\"a\"]\n[\"DEBUG:\",\"b\"]\nc\n[\"DEBUG:\",true]\n");
}

// `now` reads the clock once per record: a question built from it is the
// same question in every run, so the record ends (were it read again, every
// run would ask a new one, up to the cap).
#[test]
fn now_is_read_once() {
    let opts = Opts { max_questions: Some(10), ..Opts::default() };
    let r = deem_opts(r#"ask("at \(now)?"; .) | . >= 0"#, "x\n", &opts);
    assert_eq!((r.code, r.stdout.as_str()), (0, "true\n"), "{}", r.stderr);
    assert_eq!((r.waves(), r.stats.questions), (1, 1));
    let r = deem(r#"now as $t | ask("q"; .) as $a | ($a > 0) as $_ | now >= $t"#, "x\n");
    assert_eq!((r.code, r.stdout.as_str()), (0, "true\n"), "{}", r.stderr);
}

// A time read after an unanswered question is read again by the run that
// has the answer, so a question built from it costs a wave, and the first
// one asked with it is wasted.
#[test]
fn now_after_a_question_costs_a_wave() {
    let r = deem(r#"ask("q"; .) as $a | ask("at \(now)?"; .) | [$a, .] | length"#, "x\n");
    assert_eq!((r.code, r.stdout.as_str()), (0, "2\n"), "{}", r.stderr);
    assert_eq!((r.waves(), r.stats.questions), (2, 3));
}

// A run that ends with a question unanswered isn't the last, even when it
// ends in an error: the error comes from the run that has every answer.
#[test]
fn an_error_after_a_question_waits_for_it() {
    let r = deem(r#"ask("q"; .) as $a | error("boom")"#, "x\n");
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (2, "", "deem: record 1: boom\n"));
    assert_eq!((r.waves(), r.log.len()), (1, 1));
    let r = deem(r#"ask("FAIL"; .) as $a | error("boom")"#, "boom\n");
    assert!(r.stderr.starts_with("deem: record 1: the answer to \"FAIL\" failed"), "{}", r.stderr);
}

// A run repeats its record's earlier questions from what the record has:
// they aren't asked again, and don't count against the cap.
#[test]
fn reruns_are_free() {
    let opts = Opts { max_questions: Some(3), ..Opts::default() };
    let r = deem_opts(r#"[range(3) as $i | ask("q\($i)"; .) | . >= 0] | length"#, "x\n", &opts);
    assert_eq!((r.code, r.stdout.as_str()), (0, "3\n"), "{}", r.stderr);
    assert_eq!((r.waves(), r.stats.questions, r.log.len()), (3, 3, 3));
}

// A run goes at most 64 outputs past its first unanswered question, so
// endless output while it waits stays bounded; the run with the answer
// prints them all.
#[test]
fn endless_output_is_bounded_while_waiting() {
    let r = deem(r#"ask("q"; .) as $a | limit(500; repeat($a > 0))"#, "x\n");
    assert_eq!((r.code, r.lines().len(), r.waves()), (0, 500, 1), "{}", r.stderr);
    let r = deem(r#"limit(500; repeat(1)), ask("q"; .)"#, "x\n");
    assert_eq!((r.code, r.lines().len(), r.waves()), (0, 501, 1), "{}", r.stderr);
}

// A record that can't print yet, behind a slow one, writes a little ahead
// and pauses: it isn't run on while its output has no room. When its turn
// comes it runs again, prints the rest, and the order holds.
#[test]
fn a_record_behind_a_slow_one_pauses() {
    let fake = Arc::new(Fake { slow: Some(Duration::from_millis(300)), ..Fake::default() });
    let program = r#"if . == "a" then ask("SLOW"; .) else range(5000), ask("q"; .) end"#;
    let r = run_with(program, "a\nb\n", &Opts::default(), &config(), fake);
    let first = noul("SLOW", &text("a")).to_string();
    let last = noul("q", &text("b")).to_string();
    let want: Vec<String> =
        std::iter::once(first).chain((0..5000).map(|i| i.to_string())).chain(std::iter::once(last)).collect();
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    assert_eq!(r.lines(), want);
    assert_eq!(r.stats.questions, 2);
}
