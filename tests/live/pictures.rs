//! Do pictures still work as deem assumes? The example screenshots
//! (`examples/screens/`, 960×600) against the real API. Each test asserts.

use super::*;
use serde_json::Value as Json;
use std::collections::BTreeMap;

const SCREENS: [&str; 5] = [
    "screens/checkout-error.png",
    "screens/loading.png",
    "screens/orders.png",
    "screens/orders-unstyled.png",
    "screens/status.png",
];

/// `box` needs Jev 1.14.
const PREVIEW: [(&str, &str); 1] = [("JEV_MODEL", "jev-preview")];

/// Each output line as JSON.
fn json_lines(out: &str) -> Vec<Json> {
    out.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}"))).collect()
}

/// A box's centre, as (down, across), each a fraction of the picture.
fn centre(b: &Json) -> (f64, f64) {
    let f = |k: &str| b[k].as_f64().unwrap_or_else(|| panic!("no {k} in {b}"));
    ((f("top") + f("bottom")) / 2.0, (f("left") + f("right")) / 2.0)
}

/// The README's first picture example: a page that shows an error, and
/// pages that don't, one of which only mentions errors.
#[test]
#[ignore]
fn an_error_on_screen() {
    let mut args = vec!["does the screenshot show an error?", "-p"];
    args.extend(SCREENS);
    let r = live(&args, "");
    assert_eq!((r.code, r.stderr.as_str()), (0, ""), "{}", r.stdout);
    eprintln!("{}", r.stdout);
    let score: BTreeMap<&str, f64> =
        r.stdout.lines().map(|l| l.split_once('\t').unwrap()).map(|(s, p)| (p, s.parse().unwrap())).collect();
    assert!(score["screens/checkout-error.png"] > 0.5, "the error page");
    for page in &SCREENS[1..] {
        assert!(score[page] < 0.5, "{page} shows no error");
    }
}

/// `--img`: a page against itself as the baseline, and its unstyled
/// version against it.
#[test]
#[ignore]
fn a_picture_against_a_baseline() {
    let program = r#"{screen: ., same: ask("does `screen` look like `baseline`, with the same layout and styling?"; {screen: ., baseline: $baseline})}"#;
    let args = [
        "run",
        "--img",
        "baseline",
        "screens/orders.png",
        program,
        "screens/orders.png",
        "screens/orders-unstyled.png",
    ];
    let r = live(&args, "");
    assert_eq!((r.code, r.stderr.as_str()), (0, ""), "{}", r.stdout);
    eprintln!("{}", r.stdout);
    let same: BTreeMap<String, f64> = json_lines(&r.stdout)
        .iter()
        .map(|o| (o["screen"].as_str().unwrap().to_owned(), o["same"].as_f64().unwrap()))
        .collect();
    assert!(same["screens/orders.png"] > 0.5, "the same page");
    assert!(same["screens/orders-unstyled.png"] < 0.5, "the unstyled page");
}

/// The `screen-check` example: each page's state, chosen among described
/// options, and the error page's error boxed where its banner is (0.21 to
/// 0.31 down, 0.24 to 0.97 across).
#[test]
#[ignore]
fn screen_check() {
    let mut args = vec!["run", "screen-check"];
    args.extend(SCREENS);
    let r = live_env(&args, "", &PREVIEW);
    assert_eq!((r.code, r.stderr.as_str()), (0, ""), "{}", r.stdout);
    eprintln!("{}", r.stdout);
    let out = json_lines(&r.stdout);
    let state: BTreeMap<&str, &str> =
        out.iter().map(|o| (o["screen"].as_str().unwrap(), o["state"].as_str().unwrap())).collect();
    let want = [("checkout-error", "error"), ("loading", "loading"), ("orders", "loaded")];
    let want = want.into_iter().chain([("orders-unstyled", "broken"), ("status", "loaded")]);
    for (page, s) in want {
        assert_eq!(state[format!("screens/{page}.png").as_str()], s, "{page}");
    }
    let error = out.iter().find(|o| o["state"] == "error").unwrap();
    let (down, across) = centre(&error["error"]);
    assert!((0.15..0.4).contains(&down) && (0.24..0.97).contains(&across), "the banner: {}", error["error"]);
}

/// The `locate` example: the Pay button (0.64 to 0.71 down, 0.24 to 0.41
/// across) on the checkout page, and not on a page still loading.
#[test]
#[ignore]
fn locate_the_pay_button() {
    let r = live_env(
        &["run", "locate", "the Pay button", "screens/checkout-error.png", "screens/loading.png"],
        "",
        &PREVIEW,
    );
    assert_eq!((r.code, r.stderr.as_str()), (0, ""), "{}", r.stdout);
    eprintln!("{}", r.stdout);
    let out = json_lines(&r.stdout);
    assert_eq!(out.len(), 1, "only the checkout page has one");
    assert_eq!(out[0]["picture"], "screens/checkout-error.png");
    let (down, across) = centre(&out[0]["at"]);
    assert!((0.58..0.77).contains(&down) && (0.2..0.45).contains(&across), "the button: {}", out[0]["at"]);
}

/// A picture costs about what deem estimates (width × height / 390 input
/// tokens): the token rate counts on it. Within a factor of two, with the
/// question's own few tokens.
#[test]
#[ignore]
fn a_picture_costs_what_deem_estimates() {
    let trace = std::env::temp_dir().join(format!("deem-live-cost-{}.jsonl", std::process::id()));
    let t = trace.to_str().unwrap();
    let r = live(&["--trace", t, "is this a web page?", "-p", "screens/orders.png"], "");
    assert_eq!((r.code, r.stderr.as_str()), (0, ""), "{}", r.stdout);
    let log = std::fs::read_to_string(&trace).unwrap();
    std::fs::remove_file(&trace).unwrap();
    let tokens: u64 = log
        .lines()
        .filter_map(|l| serde_json::from_str::<Json>(l).ok())
        .filter(|e| e["ev"] == "answered")
        .map(|e| e["tokens"].as_u64().unwrap())
        .sum();
    let estimate = (960 * 600_u64).div_ceil(390);
    eprintln!("a 960×600 picture: {tokens} input tokens, deem's estimate {estimate}");
    assert!((estimate / 2..estimate * 2).contains(&tokens), "{tokens} tokens against {estimate}");
}
