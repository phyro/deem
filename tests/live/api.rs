//! Does the real API still behave as deem assumes? Each test asserts.

use super::*;

/// The README's Quick start.
#[test]
#[ignore]
fn quick_start() {
    let r = live(&["is the user reporting a problem?", "feedback.txt"], "");
    assert_eq!((r.code, r.stderr.as_str()), (0, ""));
    assert!(r.stdout.contains("charged twice"), "{}", r.stdout);
    let r = live(&["is the user reporting a problem?", "-p", "feedback.txt"], "");
    assert_eq!(r.stdout.lines().count(), 9);
}

/// Open question: does the model read "is this the same issue?" about
/// `{a, b}` as comparing them? `group` should put the two double charges
/// together and the login problem apart.
#[test]
#[ignore]
fn group_compares_a_and_b() {
    let input = lines(["charged twice for one order", "can't log in with Google", "you took my money two times"]);
    let r = live(&["run", "group", "is this the same issue?"], &input);
    assert_eq!(r.code, 0, "{}", r.stderr);
    eprintln!("{}", r.stdout);
    // lines print group by group: find each line's group by its text
    let group = |text: &str| r.stdout.lines().find_map(|l| l.split_once('\t').filter(|(_, t)| *t == text)).unwrap().0;
    let (charged, login, took) =
        (group("charged twice for one order"), group("can't log in with Google"), group("you took my money two times"));
    assert_eq!(charged, took, "the double charges together");
    assert_ne!(charged, login, "the login problem apart");
}
