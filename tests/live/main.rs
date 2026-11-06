//! Against the real model, and only when asked: see README.md here. The
//! only tests, with tests/golden/regen.sh, that touch the network.
//!
//! - `api`: does the real API still behave as deem assumes?
//! - `versus`: on three labelled datasets (labels.tsv, logs.tsv,
//!   reviews.tsv), how far text in a record moves answers, and where `like`
//!   and `unlike` examples go in a request.
//! - `pictures`: do pictures still work as deem assumes, on the example
//!   screenshots?

/// A report printed a section at a time, so that tests running side by
/// side don't interleave their lines.
#[derive(Default)]
struct Section(String);

impl Section {
    fn print(&mut self) {
        eprint!("{}", std::mem::take(&mut self.0));
    }
}

/// A line of a `Section`; before the modules, which use it.
macro_rules! say {
    ($out:expr, $($arg:tt)*) => {{
        $out.0 += &format!($($arg)*);
        $out.0.push('\n');
    }};
}

#[path = "../common/mod.rs"]
mod common;

mod api;
mod pictures;
mod versus;

use common::*;

/// Panics unless `DEEM_LIVE=1` is set: a key in the environment is not
/// consent to spend.
fn gate() {
    assert_eq!(
        std::env::var("DEEM_LIVE").as_deref(),
        Ok("1"),
        "these tests call the real API and cost money; set DEEM_LIVE=1 to run them (tests/live/README.md)"
    );
    assert!(std::env::var("JEV_API_KEY").is_ok(), "JEV_API_KEY is needed for the live tests");
}

/// Run the deem binary from `examples/` against the real API. `JEV_MODEL`
/// and `JEV_BASE_URL` pass through, so any model can be measured.
fn live(args: &[&str], stdin: &str) -> Run {
    live_env(args, stdin, &[])
}

/// `live`, with `set` in the environment over what passes through: a test
/// that needs a model of its own.
fn live_env(args: &[&str], stdin: &str, set: &[(&str, &str)]) -> Run {
    gate();
    let mut env = Vec::new();
    for name in ["JEV_API_KEY", "JEV_MODEL", "JEV_BASE_URL"] {
        if let Ok(v) = std::env::var(name) {
            env.push((name, v));
        }
    }
    env.retain(|(k, _)| !set.iter().any(|(s, _)| s == k));
    env.extend(set.iter().map(|(k, v)| (*k, (*v).to_owned())));
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    bin_online(args, stdin, &env, "examples")
}

/// Run `program` with `opts` in this process against the real API.
fn live_run(program: &str, input: &str, opts: &deem::RunOptions) -> Run {
    gate();
    let cfg = deem::Config::from_env(|k| std::env::var(k).ok()).unwrap();
    let judge = std::sync::Arc::new(deem::api::Http::new(&cfg.base_url, &cfg.key, cfg.timeout));
    run_with(program, input, opts, &cfg, judge)
}
