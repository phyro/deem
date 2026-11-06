//! `deem tune`: score the input, ask the user to label a few lines spread
//! across the scores, and suggest the threshold that agrees with them most.
//! The one command that isn't a program: it asks the user, on the terminal.

use deem::api::Judge;
use deem::programs::SCORE;
use deem::{Config, RunOptions};
use std::io::{self, BufRead, Write};
use std::sync::Arc;

/// How many lines `deem tune` shows unless `-n` says otherwise.
pub const SHOWN: usize = 20;

pub const NO_TERMINAL: &str = "deem tune needs a terminal to ask you; its input can come from a file or a pipe";

/// Score the input, label on `tty`, report on `err`; the command to use,
/// with `files`, goes to `out`. Returns the exit code.
#[allow(clippy::too_many_arguments)]
pub fn tune(
    (question, count): (&str, usize),
    files: &[String],
    opts: &RunOptions,
    cfg: &Config,
    judge: Arc<dyn Judge>,
    input: Box<dyn BufRead + Send>,
    tty: (impl BufRead, impl Write),
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let mut scored = Vec::new();
    let args = [question.to_owned()];
    let r = deem::run(("the program", SCORE, &args), opts, cfg, judge, input, &mut scored, err);
    let lines = parse(&String::from_utf8_lossy(&scored));
    if lines.is_empty() {
        let _ = writeln!(err, "deem: no lines were scored");
        return if r.code == 2 { 2 } else { 1 };
    }
    // the flags that change what was scored, so the command scores the same
    let mut flags: Vec<String> = Vec::new();
    for (on, flag) in [(opts.json_in, "-J"), (opts.slurp, "-s")] {
        flags.extend(on.then(|| flag.to_owned()));
    }
    if let Some(re) = &opts.split {
        flags.extend(["--split".to_owned(), quote(re)]);
    }
    match suggest((question, count), (&flags, files), &lines, tty, out, err) {
        0 if r.code == 2 => 2,
        code => code,
    }
}

/// Label the scored `lines` on `tty` and suggest a threshold. Asks the
/// model nothing. Returns the exit code.
fn suggest(
    (question, count): (&str, usize),
    (flags, files): (&[String], &[String]),
    lines: &[(f64, String)],
    (tty_in, tty_out): (impl BufRead, impl Write),
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let labels = match label(lines, count, tty_in, tty_out) {
        Ok(l) => l,
        Err(e) => {
            let _ = writeln!(err, "deem: {e}");
            return 2;
        }
    };
    let labelled: Vec<(f64, bool)> = labels.iter().map(|&(i, yes)| (lines[i].0, yes)).collect();
    let Some(t) = threshold(&labelled) else {
        let _ = writeln!(err, "deem: no lines labelled, so no threshold");
        return 1;
    };
    let kept = lines.iter().filter(|(s, _)| *s > t.value).count();
    let _ = writeln!(
        err,
        "\nSuggested threshold: {} ({}/{} agree with you)\n  anything from {} to {} agrees as well\n  {} would keep {kept} of {} lines",
        t.value,
        t.agree,
        labelled.len(),
        t.range.0,
        t.range.1,
        t.value,
        lines.len()
    );
    let wrong: Vec<_> = labels.iter().filter(|&&(i, yes)| (lines[i].0 > t.value) != yes).collect();
    if !wrong.is_empty() {
        let _ = writeln!(err, "\nYou and deem disagreed on:");
        for &&(i, yes) in &wrong {
            let you = if yes { "yes" } else { "no " };
            let _ = writeln!(err, "  {:.2}  you: {you}  {}", lines[i].0, lines[i].1);
        }
    }
    let _ = writeln!(err);
    let mut cmd = vec!["deem".to_owned()];
    cmd.extend(flags.iter().cloned());
    if question.starts_with('-') {
        cmd.extend(["-t".into(), t.value.to_string(), "--".into(), quote(question)]);
    } else {
        cmd.extend([quote(question), "-t".into(), t.value.to_string()]);
    }
    cmd.extend(files.iter().map(|f| quote(f)));
    let _ = writeln!(out, "{}", cmd.join(" "));
    0
}

/// `score<TAB>record` lines, split at the first tab.
fn parse(out: &str) -> Vec<(f64, String)> {
    let line = |l: &str| l.split_once('\t').and_then(|(s, r)| Some((s.parse().ok()?, r.to_owned())));
    out.lines().filter_map(line).collect()
}

/// Up to `n` lines, not already `taken`, nearest to `n` evenly spaced
/// scores from `lo` to `hi`.
fn pick(scores: &[f64], n: usize, (lo, hi): (f64, f64), taken: &[usize]) -> Vec<usize> {
    let mut picked: Vec<usize> = Vec::new();
    for k in 0..n {
        let target = if n == 1 { (lo + hi) / 2.0 } else { lo + (hi - lo) * k as f64 / (n - 1) as f64 };
        let free = (0..scores.len()).filter(|i| !taken.contains(i) && !picked.contains(i));
        if let Some(i) = free.min_by(|&a, &b| (scores[a] - target).abs().total_cmp(&(scores[b] - target).abs())) {
            picked.push(i);
        }
    }
    picked
}

/// Where the "no"s turn into "yes"s, widened by 0.1: from the lowest yes to
/// the highest no, whichever comes first.
fn unsure(labelled: &[(f64, bool)]) -> (f64, f64) {
    let yes = labelled.iter().filter(|l| l.1).map(|l| l.0).reduce(f64::min);
    let no = labelled.iter().filter(|l| !l.1).map(|l| l.0).reduce(f64::max);
    let (lo, hi) = match (yes, no) {
        (Some(y), Some(n)) => (y.min(n), y.max(n)),
        (Some(y), None) => (0.0, y),
        (None, Some(n)) => (n, 1.0),
        (None, None) => (0.0, 1.0),
    };
    ((lo - 0.1).max(0.0), (hi + 0.1).min(1.0))
}

/// Ask about the lines on the terminal; the answers, by line. EOF stops early.
fn label(
    lines: &[(f64, String)],
    count: usize,
    mut tty_in: impl BufRead,
    mut tty_out: impl Write,
) -> io::Result<Vec<(usize, bool)>> {
    let scores: Vec<f64> = lines.iter().map(|l| l.0).collect();
    // 40% spread over every score first, the rest where the "no"s turn
    // into "yes"s; with no more lines than that, all of them
    let total = lines.len().min(count);
    let first = if total < count { total } else { (count * 2).div_ceil(5) };
    let (mut shown, mut labels) = (Vec::new(), Vec::new());
    writeln!(tty_out, "Scored {} lines. Showing {total} across the score range.\n", lines.len())?;
    let mut round = pick(&scores, first, (0.0, 1.0), &[]);
    while !round.is_empty() {
        for i in round {
            shown.push(i);
            let (score, line) = &lines[i];
            let yes = loop {
                write!(tty_out, "[{}/{total}] {score:.2}  {line}\n  yes? [y/n/s=skip] ", shown.len())?;
                tty_out.flush()?;
                let mut answer = String::new();
                if tty_in.read_line(&mut answer)? == 0 {
                    writeln!(tty_out)?;
                    return Ok(labels);
                }
                match answer.trim().to_ascii_lowercase().as_str() {
                    "y" | "yes" => break Some(true),
                    "n" | "no" => break Some(false),
                    "s" | "skip" => break None,
                    _ => {}
                }
            };
            labels.extend(yes.map(|y| (i, y)));
        }
        let labelled: Vec<(f64, bool)> = labels.iter().map(|&(i, y)| (scores[i], y)).collect();
        round = pick(&scores, total - shown.len(), unsure(&labelled), &shown);
    }
    Ok(labels)
}

/// A suggested threshold: how many labels it agrees with, and the range of
/// thresholds that agree as much.
#[derive(Debug, PartialEq)]
struct Threshold {
    value: f64,
    agree: usize,
    range: (f64, f64),
}

/// Between two neighbouring labelled scores every threshold keeps the same
/// lines, so try each gap (and below and above them all); keep the ones
/// with the most agreement, and among those the widest gap. Its middle,
/// rounded, is the threshold.
fn threshold(labelled: &[(f64, bool)]) -> Option<Threshold> {
    if labelled.is_empty() {
        return None;
    }
    let mut points: Vec<f64> = labelled.iter().map(|l| l.0.clamp(0.0, 1.0)).chain([0.0, 1.0]).collect();
    points.sort_by(f64::total_cmp);
    points.dedup();
    let gaps = points.windows(2).map(|w| {
        let t = (w[0] + w[1]) / 2.0;
        let agree = labelled.iter().filter(|&&(s, yes)| (s > t) == yes).count();
        (agree, w[1] - w[0], (w[0], w[1]))
    });
    let (agree, _, (lo, hi)) =
        gaps.max_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(b.2.0.total_cmp(&a.2.0)))?;
    let mid = (lo + hi) / 2.0;
    let value = [100.0, 1000.0].iter().map(|p| (mid * p).round() / p).find(|v| *v >= lo && *v < hi).unwrap_or(mid);
    Some(Threshold { value, agree, range: (lo, hi) })
}

/// A word the shell reads back as itself.
fn quote(s: &str) -> String {
    let plain = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./,:=+".contains(c));
    if plain { s.to_owned() } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    #[test]
    fn parses_scores() {
        assert_eq!(parse("0.5\ta\tb\nx\n0.25\t{}\n"), [(0.5, "a\tb".into()), (0.25, "{}".into())]);
    }

    #[test]
    fn picks_spread_then_where_it_turns() {
        let scores: Vec<f64> = (0..101).map(|i| i as f64 / 100.0).collect();
        let first = pick(&scores, 8, (0.0, 1.0), &[]);
        assert_eq!(first, [0, 14, 29, 43, 57, 71, 86, 100]);
        let near = |(a, b): (f64, f64), (c, d): (f64, f64)| (a - c).abs() < 1e-9 && (b - d).abs() < 1e-9;
        assert!(near(unsure(&[(0.1, false), (0.4, false), (0.7, true), (0.9, true)]), (0.3, 0.8)));
        assert!(near(unsure(&[(0.6, false), (0.5, true)]), (0.4, 0.7)));
        assert!(near(unsure(&[(0.3, true)]), (0.0, 0.4)));
        let second = pick(&scores, 12, (0.3, 0.8), &first);
        assert_eq!(second.len(), 12);
        assert!(second.iter().all(|&i| (30..=80).contains(&i) && !first.contains(&i)), "{second:?}");
        assert_eq!(pick(&[0.5, 0.5], 8, (0.0, 1.0), &[]).len(), 2, "never more than there are");
    }

    #[test]
    fn threshold_agrees_most() {
        let t = threshold(&[(0.1, false), (0.4, false), (0.7, true), (0.9, true)]).unwrap();
        assert_eq!(t, Threshold { value: 0.55, agree: 4, range: (0.4, 0.7) });
        // one "no" among the "yes"es: still the widest clean gap
        let t = threshold(&[(0.2, false), (0.3, false), (0.6, true), (0.65, false), (0.8, true)]).unwrap();
        assert_eq!((t.value, t.agree, t.range), (0.45, 4, (0.3, 0.6)));
        let t = threshold(&[(0.3, true), (0.6, true)]).unwrap();
        assert_eq!((t.value, t.agree), (0.15, 2));
        assert_eq!(threshold(&[]), None);
    }

    #[test]
    fn labels_on_a_terminal() {
        let lines: Vec<(f64, String)> = (0..30).map(|i| (i as f64 / 29.0, format!("line {i}"))).collect();
        let answers = (0..20)
            .map(|k| {
                if k < 4 {
                    "n\n"
                } else if k == 5 {
                    "s\n"
                } else {
                    "y\n"
                }
            })
            .collect::<String>();
        let mut shown = Vec::new();
        let labels = label(&lines, SHOWN, answers.as_bytes(), &mut shown).unwrap();
        assert_eq!(labels.len(), 19, "one skipped");
        let shown = String::from_utf8(shown).unwrap();
        assert!(shown.contains("[20/20] "), "{shown}");
        assert!(shown.starts_with("Scored 30 lines. Showing 20 across the score range."));
        // EOF stops early; nonsense asks again
        let labels = label(&lines, SHOWN, &b"maybe\ny\nn\n"[..], Vec::new()).unwrap();
        assert_eq!(labels.len(), 2);
        let few = &lines[..5];
        assert_eq!(label(few, SHOWN, &b"y\ny\ny\ny\ny\ny\n"[..], Vec::new()).unwrap().len(), 5, "fewer than 20: all");
        // -n 30: 12 across the range, then 18 near the boundary
        let many: Vec<(f64, String)> = (0..100).map(|i| (i as f64 / 99.0, format!("line {i}"))).collect();
        let mut shown = Vec::new();
        let yes: String = (0..30).map(|_| "y\n").collect();
        assert_eq!(label(&many, 30, yes.as_bytes(), &mut shown).unwrap().len(), 30);
        assert!(String::from_utf8(shown).unwrap().contains("[30/30] "));
    }

    /// A user at the terminal who answers yes when the line shown scores
    /// above 0.5: reads what deem wrote to the terminal, answers the last line.
    #[derive(Clone, Default)]
    struct User(Rc<RefCell<Vec<u8>>>, Rc<RefCell<VecDeque<u8>>>);

    impl Write for User {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl io::Read for User {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut pending = self.1.borrow_mut();
            if pending.is_empty() {
                let shown = String::from_utf8_lossy(&self.0.borrow_mut().split_off(0)).into_owned();
                let Some(line) = shown.lines().rev().find(|l| l.starts_with('[')) else { return Ok(0) };
                let score: f64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
                pending.extend(if score > 0.5 { b"y\n" } else { b"n\n" });
            }
            let n = buf.len().min(pending.len());
            for (i, b) in pending.drain(..n).enumerate() {
                buf[i] = b;
            }
            Ok(n)
        }
    }

    // labels on the terminal, reports on stderr, prints the command to use
    #[test]
    fn suggests_a_threshold() {
        let lines: Vec<(f64, String)> = (0..40).map(|i| (((i * 37) % 40) as f64 / 40.0, format!("line {i}"))).collect();
        let files = ["my file.txt".to_owned()];
        let user = User::default();
        let tty = (io::BufReader::new(user.clone()), user);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = suggest(("is it?", SHOWN), (&[], &files), &lines, tty, &mut out, &mut err);
        let (out, err) = (String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap());
        assert_eq!(code, 0, "{err}");
        assert!(out.starts_with("deem 'is it?' -t 0."), "{out}");
        assert!(out.ends_with(" 'my file.txt'\n"), "{out}");
        assert!(err.contains("(20/20 agree with you)\n  anything from "), "{err}");
        let t: f64 = out.split(' ').nth(4).unwrap().parse().unwrap();
        let kept = lines.iter().filter(|(s, _)| *s > t).count();
        assert!(err.contains(&format!("would keep {kept} of 40 lines")), "{err}");

        // the command scores as tune did: -J reads JSON there too
        let user = User::default();
        let tty = (io::BufReader::new(user.clone()), user);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let flags = ["-J".to_owned()];
        assert_eq!(suggest(("is it?", SHOWN), (&flags, &files), &lines, tty, &mut out, &mut err), 0);
        assert!(String::from_utf8(out).unwrap().starts_with("deem -J 'is it?' -t 0."));
    }

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(quote("sample.txt"), "sample.txt");
        assert_eq!(quote("is it? it's"), r"'is it? it'\''s'");
    }
}
