//! Views of a trace file, made from the file alone: `deem trace FILE`
//! (text) and `--visual` (a timeline).

use super::{Event, Line};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// A run, rebuilt from its events. What never ended has no `to`.
#[derive(Default)]
struct Run {
    /// the last time in the log
    end: u64,
    asks: BTreeMap<u64, Asked>,
    /// (record, question, t)
    reuses: Vec<(usize, u64, u64)>,
    records: BTreeMap<usize, Rec>,
    waits: Vec<Waited>,
    /// (t, wave, its requests)
    waves: Vec<(u64, u64, Vec<u64>)>,
    requests: BTreeMap<u64, Req>,
    stops: Vec<(u64, String)>,
}

struct Asked {
    record: usize,
    text: String,
    after: Vec<u64>,
}

struct Rec {
    from: u64,
    to: Option<u64>,
    outcome: Option<String>,
}

struct Waited {
    record: usize,
    question: u64,
    from: u64,
    to: Option<u64>,
}

struct Req {
    from: u64,
    to: Option<u64>,
    questions: Vec<u64>,
    result: Option<String>,
    attempts: u64,
}

impl Req {
    fn result(&self) -> &str {
        self.result.as_deref().unwrap_or("still out when the trace ends")
    }
}

/// A trace file, read back. A last line cut short (a killed run) is skipped.
fn load(text: &str) -> Result<Run, String> {
    let mut lines = text.lines();
    let head: Option<Json> = lines.next().and_then(|l| serde_json::from_str(l).ok());
    if head.is_none_or(|h| h["deem_trace"] != 1) {
        return Err("not a deem trace: the first line should be {\"deem_trace\":1,…}".into());
    }
    let lines: Vec<&str> = lines.collect();
    let mut run = Run::default();
    for (i, line) in lines.iter().enumerate() {
        match serde_json::from_str::<Line>(line) {
            Ok(l) => run.add(l.t, l.ev),
            Err(_) if i + 1 == lines.len() => {}
            Err(_) => return Err(format!("not a deem trace: line {} is not an event", i + 2)),
        }
    }
    Ok(run)
}

impl Run {
    fn add(&mut self, t: u64, e: Event) {
        self.end = self.end.max(t);
        match e {
            Event::Record { record } => {
                self.records.insert(record, Rec { from: t, to: None, outcome: None });
            }
            Event::Ask { record, question, q, after } => {
                self.asks.insert(question, Asked { record, text: q, after });
            }
            Event::Reuse { record, question } => self.reuses.push((record, question, t)),
            Event::Wait { record, question } => self.waits.push(Waited { record, question, from: t, to: None }),
            Event::Resume { record, question } => {
                let open =
                    self.waits.iter_mut().rev().find(|w| (w.record, w.question, w.to) == (record, question, None));
                if let Some(w) = open {
                    w.to = Some(t);
                }
            }
            Event::End { record, outcome } => {
                let r = self.records.entry(record).or_insert(Rec { from: t, to: None, outcome: None });
                (r.to, r.outcome) = (Some(t), Some(outcome));
            }
            // a wave is every record's round of that depth: one event each
            Event::Wave { wave, requests, .. } => match self.waves.iter_mut().find(|w| w.1 == wave) {
                Some(w) => {
                    w.2.extend(requests);
                    w.2.sort();
                }
                None => self.waves.push((t, wave, requests)),
            },
            Event::Request { request, questions, .. } => {
                let r = Req { from: t, to: None, questions, result: None, attempts: 0 };
                self.requests.insert(request, r);
            }
            Event::Answered { request, result, attempts, .. } => {
                if let Some(r) = self.requests.get_mut(&request) {
                    (r.to, r.result, r.attempts) = (Some(t), Some(result), attempts);
                }
            }
            Event::Stop { why } => self.stops.push((t, why)),
            Event::Cut { .. } => {}
        }
    }

    /// Where something still open when the log stops is drawn to.
    fn until(&self, to: Option<u64>) -> u64 {
        to.unwrap_or(self.end)
    }

    /// `3 records · 5 questions · 2 requests · 2 waves · 54.3 ms`
    fn headline(&self) -> String {
        format!(
            "{} · {} · {} · {} · {}\n",
            n(self.records.len(), "record"),
            n(self.asks.len(), "question"),
            n(self.requests.len(), "request"),
            n(self.waves.len(), "wave"),
            ms(self.end),
        )
    }
}

/// A question as the views name it: `#3 "would a user notice?"`.
fn question(id: u64, q: &str) -> String {
    let q: String = q.chars().take(60).collect();
    format!("#{id} {}", Json::String(q))
}

/// Lines of the text report are at most this wide, wrapped where they can be.
const COLUMNS: usize = 80;

/// A line cut at ` · ` or `, ` to fit `COLUMNS`, continued 4 deeper.
fn wrap(line: &str) -> String {
    let indent = " ".repeat(line.len() - line.trim_start().len() + 4);
    let (mut out, mut rest) = (String::new(), line.to_owned());
    while rest.chars().count() > COLUMNS {
        let fits: String = rest.chars().take(COLUMNS).collect();
        let cut = [" · ", ", "].iter().filter_map(|sep| fits.rfind(sep).map(|i| (i, *sep))).max();
        let Some((i, sep)) = cut.filter(|(i, _)| *i > indent.len()) else { break };
        // a `·` starts the next line; a comma ends this one
        let (end, next) = if sep == ", " { (i + 1, i + 2) } else { (i, i + 1) };
        out += &rest[..end];
        out.push('\n');
        rest = format!("{indent}{}", &rest[next..]);
    }
    out + &rest
}

/// `deem trace FILE`: the trace as text. Waves, the requests they sent, the
/// record each request carried with its questions, what each record had
/// waited for before asking them, and how each record spent its time.
pub fn report(text: &str) -> Result<String, String> {
    const SHOWN: usize = 12;
    let run = load(text)?;
    let mut out = run.headline();
    out += "#N numbers a question; \"after #M\": it waited for #M's answer since its last\n";
    for (t, wave, reqs) in &run.waves {
        let qs: usize = reqs.iter().filter_map(|r| run.requests.get(r)).map(|r| r.questions.len()).sum();
        let _ = writeln!(out, "\nwave {wave} at {}: {} in {}", ms(*t), n(qs, "question"), n(reqs.len(), "request"));
        for (id, r) in reqs.iter().filter_map(|id| Some((id, run.requests.get(id)?))) {
            // the request's questions, by record, in the order they were asked
            let mut by_record: Vec<(usize, Vec<u64>)> = Vec::new();
            for &q in &r.questions {
                let rec = run.asks.get(&q).map_or(0, |a| a.record);
                match by_record.last_mut() {
                    Some((last, qs)) if *last == rec => qs.push(q),
                    _ => by_record.push((rec, vec![q])),
                }
            }
            let tries = if r.attempts > 1 { format!(" · {} attempts", r.attempts) } else { String::new() };
            let to = r.to.map_or("…".into(), ms);
            let size = format!("{}, {}", n(by_record.len(), "record"), n(r.questions.len(), "question"));
            let _ = writeln!(out, "  request {id} · {}–{to} · {size} · {}{tries}", ms(r.from), r.result());
            for (rec, qs) in by_record.iter().take(SHOWN) {
                let text = |q: &u64| run.asks.get(q).map_or(String::new(), |a| question(*q, &a.text));
                let mut after: Vec<String> = Vec::new();
                for a in qs.iter().filter_map(|q| run.asks.get(q)).flat_map(|a| &a.after).map(|a| format!("#{a}")) {
                    if !after.contains(&a) {
                        after.push(a);
                    }
                }
                let after = if after.is_empty() { String::new() } else { format!("   after {}", after.join(", ")) };
                let _ =
                    writeln!(out, "    record {rec:<5} {}{after}", qs.iter().map(text).collect::<Vec<_>>().join(", "));
            }
            more(&mut out, "    ", by_record.len().saturating_sub(SHOWN), "record");
        }
    }
    for (t, why) in &run.stops {
        let _ = writeln!(out, "\nstopped at {}: {why}", ms(*t));
    }

    out += "\nrecords\n";
    for (rec, r) in run.records.iter().take(SHOWN) {
        let waits: Vec<String> = (run.waits.iter().filter(|w| w.record == *rec))
            .map(|w| match w.to {
                Some(to) => format!("#{} for {}", w.question, ms(to - w.from)),
                None => format!("#{} (still waiting)", w.question),
            })
            .collect();
        let reused: Vec<String> = run.reuses.iter().filter(|u| u.0 == *rec).map(|u| format!("#{}", u.1)).collect();
        let outcome = r.outcome.as_deref().unwrap_or("still running when the trace ends");
        let mut line = format!("  record {rec:<5} {:>9}  {outcome}", ms(run.until(r.to) - r.from));
        if !waits.is_empty() {
            line += &format!(" · waited on {}", waits.join(", "));
        }
        if !reused.is_empty() {
            line += &format!(" · reused {}", reused.join(", "));
        }
        let _ = writeln!(out, "{line}");
    }
    more(&mut out, "  ", run.records.len().saturating_sub(SHOWN), "record");
    Ok(out.lines().map(|l| wrap(l) + "\n").collect())
}

/// `deem trace --visual FILE`: the run as a timeline, a row per request and
/// per record on one time axis. A request carrying a question that was asked
/// after another request's answer starts with `└──▶` from where that request
/// ended: it could not go out sooner.
pub fn visual(text: &str) -> Result<String, String> {
    // 13 for the label, the timeline, and a note: 80 columns in all
    const WIDTH: usize = 45;
    const SHOWN: usize = 40;
    let run = load(text)?;
    let total = run.end.max(1);
    let col = |t: u64| ((u128::from(t) * WIDTH as u128 / u128::from(total)) as usize).min(WIDTH);
    let bar = |from: u64, to: Option<u64>| {
        let (from, to) = (col(from), col(run.until(to)));
        (from, to.max(from + 1))
    };
    let row = |label: String, cells: Vec<char>, note: String| {
        let line = format!("{label:<13}{}  {note}", cells.into_iter().collect::<String>());
        line.trim_end().to_owned() + "\n"
    };

    // question → the request that carried it
    let carried: BTreeMap<u64, u64> =
        run.requests.iter().flat_map(|(n, r)| r.questions.iter().map(move |q| (*q, *n))).collect();
    let groups = Groups::new(&run);

    let mut out = run.headline();
    out += "█      a request in flight\n";
    out += "━      a record running\n";
    out += "░      a record waiting for an answer\n";
    out += "┐ └    the lower request needed the upper one's answer, so it went out after it\n";
    out += "[A B]  the questions a request carried, as listed under \"questions\"\n\n";
    let _ = writeln!(out, "{:13}0 ms{:>w$}", "", ms(total), w = WIDTH - 4);
    let mut waves = vec![' '; WIDTH + 1];
    for (t, _, _) in &run.waves {
        waves[col(*t).min(WIDTH - 1)] = '▼';
    }
    out += &row("waves".into(), waves, String::new());

    // Bars cover [start, end). An arrow runs from the end of each request
    // another one needed (`┐`), down, and into the one that needed it (`└`,
    // `┴` where arrows join, `▶` if it went out later than the last of them).
    let shown: Vec<(&u64, &Req)> = run.requests.iter().take(SHOWN).collect();
    let mut rows: Vec<Vec<char>> = vec![vec![' '; WIDTH + 1]; shown.len()];
    for (row, (_, r)) in rows.iter_mut().zip(&shown) {
        let (from, to) = bar(r.from, r.to);
        row[from..to].fill('█');
    }
    let mut notes = Vec::new();
    for (i, &(&n, r)) in shown.iter().enumerate() {
        let mut deps: Vec<u64> = (r.questions.iter())
            .flat_map(|q| run.asks.get(q).into_iter().flat_map(|a| &a.after))
            .filter_map(|a| carried.get(a).copied())
            .filter(|&d| d != n)
            .collect();
        deps.sort_unstable();
        deps.dedup();
        // (row, column) of each shown request it needed, drawn above it
        let ends: Vec<(usize, usize)> = deps
            .iter()
            .filter_map(|d| shown.iter().position(|s| s.0 == d))
            .filter(|&j| j < i)
            .map(|j| (j, bar(shown[j].1.from, shown[j].1.to).1))
            .collect();
        if let (Some(first), Some(last)) = (ends.iter().map(|e| e.1).min(), ends.iter().map(|e| e.1).max()) {
            let (from, to) = bar(r.from, r.to);
            let row = &mut rows[i];
            row[first..=last].fill('─');
            if from > last + 1 {
                row[last + 1..from].fill('─');
                row[from - 1] = '▶';
            } else {
                // it went out as the last answer came back: its bar starts after the corner
                row[last + 1..to.max(last + 2).min(WIDTH + 1)].fill('█');
            }
            for &(j, e) in &ends {
                rows[i][e] = if e == first { '└' } else { '┴' };
                rows[j][e] = join(rows[j][e], '┐');
                for row in &mut rows[j + 1..i] {
                    row[e] = join(row[e], '│');
                }
            }
        }
        let mut carries: Vec<usize> = r.questions.iter().filter_map(|q| groups.of.get(q).copied()).collect();
        carries.sort_unstable();
        carries.dedup();
        let carries = format!("[{}]", carries.into_iter().map(letter).collect::<Vec<_>>().join(" "));
        let mut note = match deps.as_slice() {
            [] => carries,
            [d] => format!("{carries} after request {d}"),
            ds => format!("{carries} after requests {}", list(ds)),
        };
        if r.result() != "ok" {
            note = format!("{note} · {}", r.result());
        }
        notes.push(note);
    }
    for (((n, _), cells), note) in shown.iter().zip(rows).zip(notes) {
        out += &row(format!("request {n}"), cells, note);
    }
    more(&mut out, "", run.requests.len().saturating_sub(SHOWN), "request");

    for (rec, r) in run.records.iter().take(SHOWN) {
        let mut cells = vec![' '; WIDTH + 1];
        let (from, to) = bar(r.from, r.to);
        cells[from..to].fill('━');
        for w in run.waits.iter().filter(|w| w.record == *rec) {
            let (from, to) = bar(w.from, w.to);
            cells[from..to].fill('░');
        }
        let note = match r.outcome.as_deref() {
            Some("done") => String::new(),
            Some(o) => o.to_owned(),
            None => "still running when the trace ends".into(),
        };
        out += &row(format!("record {rec}"), cells, note);
    }
    more(&mut out, "", run.records.len().saturating_sub(SHOWN), "record");

    // the questions, grouped, and what each group was asked after
    out += "\nquestions (… stands for a number: an answer, or one from the record)\n";
    for (i, g) in groups.list.iter().enumerate() {
        let reqs: Vec<u64> = g.requests.iter().copied().collect();
        let sent = match reqs.as_slice() {
            [] => "not sent".to_owned(),
            [r] => format!("in request {r}"),
            rs => format!("in requests {}", list(rs)),
        };
        let mut line = format!("  {:<3}{} · {} {sent}", letter(i), Json::String(g.text.clone()), g.count);
        if !g.after.is_empty() {
            line += &format!(" · after {}", g.after.iter().map(|&a| letter(a)).collect::<Vec<_>>().join(", "));
        }
        let _ = writeln!(out, "{line}");
    }
    Ok(out)
}

/// Questions with the same text, numbers aside: usually one `ask` in the
/// program, whose text may carry an earlier answer.
struct Groups {
    list: Vec<Group>,
    /// question → its group
    of: BTreeMap<u64, usize>,
}

#[derive(Default)]
struct Group {
    text: String,
    count: usize,
    requests: BTreeSet<u64>,
    /// groups whose answers its questions were asked after
    after: BTreeSet<usize>,
}

impl Groups {
    fn new(run: &Run) -> Self {
        let (mut list, mut group, mut by_text) = (Vec::<Group>::new(), BTreeMap::new(), BTreeMap::new());
        for (q, a) in &run.asks {
            let text: String = mask(&a.text).chars().take(60).collect();
            let i = *by_text.entry(text.clone()).or_insert_with(|| {
                list.push(Group { text, ..Group::default() });
                list.len() - 1
            });
            list[i].count += 1;
            group.insert(*q, i);
        }
        for (q, a) in &run.asks {
            let i = group[q];
            for d in a.after.iter().filter_map(|d| group.get(d)) {
                list[i].after.insert(*d);
            }
        }
        for (n, r) in &run.requests {
            for q in &r.questions {
                if let Some(&i) = group.get(q) {
                    list[i].requests.insert(*n);
                }
            }
        }
        Self { list, of: group }
    }
}

/// `given a score of 0.42` → `given a score of …`
fn mask(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let starts = c.is_ascii_digit() || (c == '-' && chars.peek().is_some_and(char::is_ascii_digit));
        if !starts {
            out.push(c);
            continue;
        }
        while chars.next_if(|c| c.is_ascii_digit() || *c == '.').is_some() {}
        if out.ends_with('…') {
            continue;
        }
        out.push('…');
    }
    out
}

/// A, B, … Z, AA, AB, …
fn letter(i: usize) -> String {
    let c = char::from(b'A' + (i % 26) as u8);
    if i < 26 { c.to_string() } else { format!("{}{c}", letter(i / 26 - 1)) }
}

fn list(ns: &[u64]) -> String {
    ns.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
}

/// A line of the timeline drawn over a cell: bars stay on top.
fn join(under: char, line: char) -> char {
    match (under, line) {
        (' ', l) => l,
        ('─', '│') => '┼',
        ('└', '│') => '├',
        ('┴', '│') => '┼',
        ('│', '┐') | ('┐', '│') => '┤',
        ('█', '┐') => '┐',
        (u, _) => u,
    }
}

fn ms(us: u64) -> String {
    format!("{:.1} ms", us as f64 / 1000.0)
}

fn n(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn more(out: &mut String, pad: &str, left: usize, what: &str) {
    if left > 0 {
        let _ = writeln!(out, "{pad}… and {left} more {what}{}", if left == 1 { "" } else { "s" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_wrap_at_separators() {
        let line =
            format!("  record 1       58.4 ms  done · waited on #4 for 7.4 ms, {} · reused #4", "#15 for 49.5 ms");
        let w = wrap(&line);
        assert_eq!(w, "  record 1       58.4 ms  done · waited on #4 for 7.4 ms, #15 for 49.5 ms\n      · reused #4");
        assert!(w.lines().all(|l| l.chars().count() <= COLUMNS));
        assert_eq!(wrap("short · line"), "short · line");
        let long = format!("  {}", "x".repeat(100));
        assert_eq!(wrap(&long), long, "nowhere to cut: left as it is");
        let list: Vec<String> = (1..40).map(|i| format!("#{i} for 1.0 ms")).collect();
        let w = wrap(&format!("  record 7  done · waited on {}", list.join(", ")));
        assert!(w.lines().count() > 3 && w.lines().all(|l| l.chars().count() <= COLUMNS), "{w}");
        assert!(w.lines().skip(1).all(|l| l.starts_with("      ")), "{w}");
    }
}
