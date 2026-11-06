//! The library's layers depend one way. Each folder (or top-level file) of
//! `src/` may name only the layers listed for it; `crate::lock` and the other
//! helpers in `lib.rs` are for everyone. The binary (`main.rs`, `cli/`) is
//! another crate, so the compiler already keeps it to the public API.
//!
//! And no test but tests/live can reach another machine: every process a
//! test starts gets `common::OFFLINE`.

use std::collections::BTreeSet;
use std::path::Path;

/// (layer, the layers it may use)
const ALLOWED: [(&str, &[&str]); 9] = [
    ("clock", &[]),
    ("engine", &["transport::api"]),
    ("trace", &["clock"]),
    ("transport", &["clock", "trace"]),
    ("config", &["transport"]),
    ("run", &["engine", "transport", "trace", "clock", "config"]),
    ("programs", &["engine"]),
    ("lib", &["clock", "config", "engine", "programs", "run", "trace", "transport"]),
    ("cli", &[]),
];

/// Items of `lib.rs` itself, usable from any layer.
const ROOT: [&str; 3] = ["lock", "wait", "wait_timeout"];

fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn layers_depend_one_way() {
    let mut all = Vec::new();
    files(Path::new("src"), &mut all);
    let mut wrong = BTreeSet::new();
    for path in all {
        let rel = path.strip_prefix("src").unwrap();
        let layer = rel.iter().next().unwrap().to_str().unwrap().trim_end_matches(".rs");
        let layer = if layer == "main" { "cli" } else { layer };
        let allowed = ALLOWED.iter().find(|(l, _)| *l == layer).unwrap_or_else(|| panic!("no rule for {layer}")).1;
        let text = std::fs::read_to_string(&path).unwrap();
        for (i, _) in text.match_indices("crate::") {
            let used: String =
                text[i + 7..].chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':').collect();
            let first = used.split("::").next().unwrap();
            let ok = first == layer
                || ROOT.contains(&first)
                || allowed.iter().any(|a| used == *a || used.starts_with(&format!("{a}::")) || *a == first);
            if !ok {
                wrong.insert(format!("{}: crate::{used}", path.display()));
            }
        }
    }
    assert!(wrong.is_empty(), "layers used against the rule:\n{}", wrong.into_iter().collect::<Vec<_>>().join("\n"));
}

/// A process a test may start without `OFFLINE`: neither is deem.
const NOT_DEEM: [&str; 3] =
    [r#"Command::new("setsid").arg("--version")"#, r#"Command::new("target/jq")"#, r#"Command::new("mkfifo")"#];

// Every process a test starts has `OFFLINE` in its environment, so a deem
// among them can't reach the hosted API whatever key and URL it is given
// (tests/cli.rs, `no_test_reaches_another_machine`). Only tests/live start
// deem with the network as it is, through `bin_online`, and only they make
// a client from the environment's URL.
#[test]
fn tests_start_nothing_that_can_reach_the_api() {
    let mut all = Vec::new();
    files(Path::new("tests"), &mut all);
    let mut wrong = Vec::new();
    for path in all {
        let file = path.display().to_string();
        let (live, common) = (file.starts_with("tests/live"), file.starts_with("tests/common"));
        if live || file == "tests/layers.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let at = format!("{file}:{}: {}", i + 1, l.trim());
            // the one place that starts deem: `bin_online`, which `bin` calls with `OFFLINE`
            let starts = l.contains("Command::new(") && !common && !NOT_DEEM.iter().any(|n| l.contains(n));
            if starts
                && !lines[i..(i + 6).min(lines.len())]
                    .iter()
                    .any(|l| l.contains("envs(OFFLINE)") || l.contains("envs(common::OFFLINE)"))
            {
                wrong.push(format!("{at}  (a process without OFFLINE)"));
            }
            if !common && l.contains("bin_online(") {
                wrong.push(format!("{at}  (bin_online is for tests/live)"));
            }
            if l.contains("Http::new(&cfg.base_url") || l.contains("from_env(|k| std::env::var(k)") {
                wrong.push(format!("{at}  (a client or a config from the environment)"));
            }
        }
        if common {
            assert_eq!(text.matches("Command::new(").count(), 1, "tests/common starts deem in bin_online only");
            assert_eq!(text.matches("bin_online(").count(), 2, "bin_online: defined, and called by bin");
        }
    }
    assert!(wrong.is_empty(), "tests that could reach another machine:\n{}", wrong.join("\n"));
}

// The live tests make a client for the real API, or start a deem that can
// reach it, in a function whose first statement is `gate()`: nothing is
// sent unless `DEEM_LIVE=1` is set, whatever key is in the environment.
#[test]
fn live_tests_open_the_gate_first() {
    let mut all = Vec::new();
    files(Path::new("tests/live"), &mut all);
    let (mut wrong, mut gated) = (Vec::new(), 0);
    for path in all {
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if !l.contains("Http::new(") && !l.contains("bin_online(") {
                continue;
            }
            // the function this line is in: the nearest `fn` above, not indented
            let start = (0..i).rev().find(|&j| lines[j].starts_with("fn ") || lines[j].starts_with("pub")).unwrap_or(0);
            let body =
                lines[start..i].iter().position(|l| l.trim_end().ends_with('{')).map_or(start, |k| start + k + 1);
            match lines.get(body).map(|l| l.trim()) {
                Some("gate();") => gated += 1,
                _ => wrong.push(format!("{}:{}: {}", path.display(), i + 1, l.trim())),
            }
        }
    }
    assert!(wrong.is_empty(), "a way to the real API that doesn't start with gate():\n{}", wrong.join("\n"));
    assert_eq!(gated, 3, "the binary, the library, and like_and_unlike's client");
}
