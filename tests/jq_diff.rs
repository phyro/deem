//! deem's language is jaq's, plus deem's additions: `judge`, the
//! prelude's `ask`, `choose`, `rate` and `box`, and the jq builtins jaq lacks
//! (`src/engine/compat.jq`).
//!
//! - `jaq_parity`: jq's test suite (`tests/golden/jq.test`), as a corpus of
//!   programs, runs under deem's engine and under plain jaq with the same
//!   additions. Deferred values must change nothing: the outputs match,
//!   except where deem differs from jaq on purpose (`tests/golden/jaq.skip`).
//! - `everyday`: common programs (`tests/golden/everyday.tsv`) under deem and
//!   under jq 1.7.1. Where they differ is listed, with why, in
//!   `tests/golden/everyday.diff`, so the README's differences from jq can't
//!   drift. jq's outputs are recorded in `everyday.out` by
//!   `tests/golden/regen.sh` (`DEEM_REGEN=1`), so `cargo test` needs no jq.
//! - `judge_programs`: the README's programs with `judge` run under deem,
//!   against the fake, and under plain jaq with `judge` a lookup in what deem
//!   was told. Deferring answers changes the order of evaluation, not the
//!   results. A question jaq reaches that deem never asked fails the lookup,
//!   and so does one whose state wasn't sent as `$x | tojson`.

mod common;

use common::*;
use deem::Chunk;
use deem::api::{Asked, Failure, Judge, Spec};
use deem::engine::{Ask, Emitted, J, Outcome, Program};
use jaq_core::data::JustLut;
use jaq_core::load::{self, Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars};
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::process::{Command, Stdio};

/// Plain jaq, with deem's additions and `$ENV` empty as in deem, on one
/// input. `vars` are global variables; `raw` prints strings unquoted.
fn jaq(program: &str, vars: &[(&str, J)], input: J, raw: bool) -> (String, bool) {
    type D = JustLut<J>;
    let compat = load::parse(deem::engine::COMPAT, |p| p.defs()).unwrap();
    let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs()).chain(compat);
    let funs = jaq_core::funs::<D>().chain(jaq_std::funs::<D>()).chain(jaq_json::funs::<D>());
    let arena = Arena::default();
    let Ok(modules) = Loader::new(defs).load(&arena, File { code: program, path: () }) else {
        return (String::new(), false);
    };
    let names: Vec<&str> = ["$ENV"].into_iter().chain(vars.iter().map(|v| v.0)).collect();
    let Ok(filter) = Compiler::default().with_funs(funs).with_global_vars(names).compile(modules) else {
        return (String::new(), false);
    };
    let vals = [J::obj(Default::default())].into_iter().chain(vars.iter().map(|v| v.1.clone()));
    let mut out = Chunk::default();
    for r in filter.id.run((Ctx::<D>::new(&filter.lut, Vars::new(vals)), input)) {
        match r {
            Ok(v) => out.push(&v, raw),
            // `halt` ends the program without an error
            Err(e) => return (String::from_utf8_lossy(&out.bytes).into_owned(), e.get_err().is_err()),
        }
        if out.bytes.len() > 1 << 20 {
            break;
        }
    }
    (String::from_utf8_lossy(&out.bytes).into_owned(), true)
}

/// No question is ever asked in the corpora.
struct NoAsk;

impl Ask for NoAsk {
    fn ask(&self, _: &J, _: Spec) -> Result<Asked, Failure> {
        Err(Failure::Failed("no judge in this test".into()))
    }
}

/// A program on one JSON input, under deem's engine or plain jaq, printed
/// compact. Some programs loop or grow without end, so each case runs in a
/// child process (this test binary, `engine_case`) with a memory limit, a
/// time limit and an output limit.
fn engine(mode: &str, program: &str, input: &str) -> (String, bool) {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new("sh")
        .args(["-c", "ulimit -v 2000000; exec \"$0\" engine_case --exact --nocapture --test-threads=1"])
        .arg(exe)
        .envs(common::OFFLINE)
        .env("DEEM_CASE_MODE", mode)
        .env("DEEM_CASE_PROGRAM", program)
        .env("DEEM_CASE_INPUT", input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > std::time::Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return ("<timeout>".into(), false);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let out = child.wait_with_output().unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    match out.lines().find_map(|l| l.strip_prefix("DEEM_RESULT ")) {
        Some(r) => serde_json::from_str(r).unwrap(),
        None => ("<crashed>".into(), false),
    }
}

/// The child side of `engine`: does nothing unless `engine` started it.
#[test]
fn engine_case() {
    let (Ok(mode), Ok(program), Ok(input)) =
        (std::env::var("DEEM_CASE_MODE"), std::env::var("DEEM_CASE_PROGRAM"), std::env::var("DEEM_CASE_INPUT"))
    else {
        return;
    };
    let run = || {
        let input = input.trim_start_matches('\u{feff}');
        let Ok(input) = jaq_json::read::parse_single(input.as_bytes()) else {
            return (String::new(), false);
        };
        if mode == "jaq" {
            return jaq(&program, &[], input, false);
        }
        let Ok(p) = Program::compile(&program, &[], &[]) else {
            return (String::new(), false);
        };
        let mut out = Chunk::default();
        // nothing is asked, so nothing is waited for
        let outcome = p.run(input, &NoAsk, &mut |_| Some(0), &mut |vs| {
            vs.iter().for_each(|v| out.push(v, false));
            if out.bytes.len() < 1 << 20 { Emitted::More } else { Emitted::Closed }
        });
        (String::from_utf8_lossy(&out.bytes).into_owned(), outcome == Outcome::Done)
    };
    println!("\nDEEM_RESULT {}", serde_json::to_string(&run()).unwrap());
}

/// A list of cases that differ, by line, with why: `N <TAB> reason`.
fn listed(path: &str) -> BTreeMap<usize, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let line = |l: &str| {
        let (n, why) = l.split_once('\t').unwrap_or((l, ""));
        (n.parse().unwrap(), why.to_owned())
    };
    text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).map(line).collect()
}

/// Every case that differs must be listed, and every listed case differ.
fn check(path: &str, differ: &BTreeSet<usize>, report: &str) {
    let list = listed(path);
    let unlisted: String = report
        .split("\n\n")
        .filter(|c| {
            let n: usize = c.split(':').next().unwrap_or("").parse().unwrap_or(0);
            n > 0 && !list.contains_key(&n)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let fixed: Vec<_> = list.keys().filter(|l| !differ.contains(l)).collect();
    assert!(unlisted.is_empty(), "differences not in {path}:\n{unlisted}");
    assert!(fixed.is_empty(), "these no longer differ; remove them from {path}: {fixed:?}");
}

/// jq.test: groups of lines separated by blank lines; the program, the
/// input, then the expected outputs. `%%FAIL` groups expect a compile error.
fn jq_test() -> Vec<(usize, String, String)> {
    let text = std::fs::read_to_string("tests/golden/jq.test").unwrap();
    let mut cases = Vec::new();
    let mut group: Vec<(usize, &str)> = Vec::new();
    for (i, line) in text.lines().chain([""]).enumerate() {
        if line.starts_with('#') {
            continue;
        }
        if !line.is_empty() {
            group.push((i + 1, line));
            continue;
        }
        match group.as_slice() {
            [(_, f), (n, program), ..] if f.starts_with("%%FAIL") => {
                cases.push((*n, program.to_string(), "null".into()))
            }
            [(n, program), (_, input), ..] => cases.push((*n, program.to_string(), input.to_string())),
            _ => {}
        }
        group.clear();
    }
    cases
}

#[test]
fn jaq_parity() {
    let cases = jq_test();
    assert!(cases.len() > 400, "{} cases", cases.len());
    let (mut differ, mut report) = (BTreeSet::new(), String::new());
    for (line, program, input) in &cases {
        let (want, got) = (engine("jaq", program, input), engine("deem", program, input));
        if got != want {
            differ.insert(*line);
            report += &format!("{line}: {program}\n  input {input}\n  jaq   {want:?}\n  deem  {got:?}\n\n");
        }
    }
    check("tests/golden/jaq.skip", &differ, &report);
    eprintln!("{} of {} programs print the same under deem and jaq", cases.len() - differ.len(), cases.len());
}

/// jq's stdout for a program and an input, as recorded, and whether it
/// succeeded.
struct Jq {
    path: &'static str,
    golden: BTreeMap<(String, String), (String, bool)>,
    seen: BTreeMap<(String, String), (String, bool)>,
}

fn regen() -> bool {
    std::env::var_os("DEEM_REGEN").is_some()
}

impl Jq {
    fn new(path: &'static str) -> Self {
        let text = if regen() { String::new() } else { std::fs::read_to_string(path).unwrap_or_default() };
        let entry = |l: &str| {
            let v: Json = serde_json::from_str(l).unwrap();
            let key = (v["program"].as_str().unwrap().to_owned(), v["input"].as_str().unwrap().to_owned());
            (key, (v["stdout"].as_str().unwrap().to_owned(), v["ok"].as_bool().unwrap()))
        };
        Self { path, golden: text.lines().map(entry).collect(), seen: BTreeMap::new() }
    }

    fn run(&mut self, program: &str, input: &str) -> (String, bool) {
        let key = (program.to_owned(), input.to_owned());
        let out = match self.golden.get(&key) {
            Some(out) => out.clone(),
            None if regen() => {
                let mut child = Command::new("target/jq")
                    .args(["-c", program])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("target/jq: run tests/golden/regen.sh");
                child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
                let o = child.wait_with_output().unwrap();
                (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.success())
            }
            None => panic!("no recorded jq output for {program:?}; run tests/golden/regen.sh"),
        };
        self.seen.insert(key, out.clone());
        out
    }
}

impl Drop for Jq {
    fn drop(&mut self) {
        if regen() {
            let lines: Vec<String> = (self.seen.iter())
                .map(|((p, i), (o, ok))| json!({"program": p, "input": i, "stdout": o, "ok": ok}).to_string())
                .collect();
            std::fs::write(self.path, lines.join("\n") + "\n").unwrap();
        }
    }
}

#[test]
fn everyday() {
    let mut jq = Jq::new("tests/golden/everyday.out");
    let text = std::fs::read_to_string("tests/golden/everyday.tsv").unwrap();
    let (mut differ, mut report) = (BTreeSet::new(), String::new());
    for (i, l) in text.lines().enumerate().filter(|(_, l)| !l.starts_with('#') && !l.is_empty()) {
        let (program, input) = l.split_once('\t').unwrap();
        let (want, got) = (jq.run(program, input), engine("deem", program, input));
        if got != want {
            differ.insert(i + 1);
            report += &format!("{}: {program}\n  input {input}\n  jq    {want:?}\n  deem  {got:?}\n\n", i + 1);
        }
    }
    check("tests/golden/everyday.diff", &differ, &report);
}

/// The README's programs with `judge`, on the sample data.
#[test]
fn judge_programs() {
    let cases = std::fs::read_to_string("tests/golden/cases.tsv").unwrap();
    // each case paces itself at the API's rate, so they run side by side
    std::thread::scope(|scope| {
        for line in cases.lines() {
            scope.spawn(move || judge_program(line));
        }
    });
}

fn judge_program(line: &str) {
    let [name, input, program] = line.splitn(3, '\t').collect::<Vec<_>>()[..] else { panic!("{line}") };
    let program = match program.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).unwrap(),
        None => program.to_owned(),
    };
    // as in the docs: `-J` for a .jsonl file, text for any other
    let text = !input.ends_with(".jsonl");
    let input = std::fs::read_to_string(input).unwrap();
    let r = if text { deem(&program, &input) } else { deem_json(&program, &input) };
    assert_eq!(r.stderr, "", "{name}");
    let tool = deem::engine::Sig::of(&program).unwrap().params.is_some();
    let (program, table) = told(&r, &program);
    // a program file with `main` is only definitions: call it
    let program = if tool { program + "\nmain" } else { program };
    let mut want = String::new();
    for l in input.lines().filter(|l| !l.is_empty()) {
        let x = if text { J::from(l.to_owned()) } else { jaq_json::read::parse_single(l.as_bytes()).unwrap() };
        let (out, ok) = jaq(&program, &[("$T", table.clone())], x, text);
        assert!(ok, "{name}: jaq failed on {l}");
        want += &out;
    }
    assert_eq!(r.stdout, want, "{name}");
}

/// The table of what deem asked and was told, and `program` under plain jaq
/// with `judge` a lookup in it. A question is found by the state's bytes as
/// sent, which must be `$x | tojson`: the model sees exactly what jaq prints
/// (a number, boolean or null as that text in a string: a state is a string,
/// an object or an array).
fn told(r: &Run, program: &str) -> (String, J) {
    // keyed by the question and the state, so a lookup doesn't scan
    let mut table = serde_json::Map::new();
    for req in &r.log {
        let answers = common::Fake::default().judge(req).unwrap().answers;
        for (i, (sent, q)) in asked_raw(req).into_iter().enumerate() {
            table.insert(format!("{q}\u{0}{sent}"), answers[&format!("q{i}")].value.clone());
        }
    }
    let lookup = r#"def sent: tojson | if .[:1] | IN("\"", "{", "[") then . else tojson end;
def judge($s; $x): ($s.q + "\u0000" + ($x | sent)) as $k
  | if $T | has($k) then {value: $T[$k], confidence: null, probabilities: null}
    else error("deem never asked \($s.q)") end;"#;
    // The definitions go after the directive, which must come first.
    let (directive, program) = match program.starts_with("module ") {
        true => program.split_once(';').map(|(d, p)| (format!("{d};"), p)).unwrap(),
        false => (String::new(), program),
    };
    let program = format!("{directive}\n{lookup}\n{}\n{program}", deem::engine::PRELUDE);
    (program, jaq_json::read::parse_single(Json::Object(table).to_string().as_bytes()).unwrap())
}

/// Each built-in program under plain jaq, on the array of records, with
/// deem's answers as the lookup table, prints what deem prints.
#[test]
fn builtin_programs() {
    let lines: Vec<String> = common::records48().into_iter().take(30).collect();
    for (name, args) in [
        ("group", vec!["is this the same issue?"]),
        ("histogram", vec!["is this urgent?"]),
        ("mix", vec!["topic?", "bug,billing,other"]),
        ("disagree", vec!["is this a problem?", "is the user complaining?"]),
    ] {
        let src = deem::programs::BUILTIN.iter().find(|b| b.0 == name).unwrap().1;
        let r = deem_args(src, &args, &common::lines(&lines), &Default::default());
        assert_eq!((r.code, r.stderr.as_str()), (0, ""), "{name}");
        let (program, table) = told(&r, src);
        let call: Vec<String> = (0..args.len()).map(|i| format!("$__{i}")).collect();
        let program = format!("{program}\nmain({})", call.join("; "));
        let mut vars = vec![("$T", table), ("$threshold", J::from(0.5)), ("$groups", J::from(20isize))];
        let names = ["$__0", "$__1"];
        vars.extend(args.iter().enumerate().map(|(i, a)| (names[i], J::from(a.to_string()))));
        let input: J = lines.iter().map(|l| J::from(l.clone())).collect();
        let (want, ok) = jaq(&program, &vars, input, true);
        assert!(ok, "{name}: jaq failed");
        assert_eq!(r.stdout, want, "{name}");
        assert!(!want.is_empty(), "{name}");
    }
}
