//! `deem trace compare --with NAME FILE…`: the answers in traces of runs
//! against several APIs, each API's against NAME's, which are taken as
//! right. Answers are matched by what was asked: the same question (its
//! text, kind and options) about the same value, whichever program or file
//! asked it. Only ask and choose are compared.

use super::{Event, Line};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

const WIDTH: usize = 80;
/// How many of the largest differences are listed.
const LARGEST: usize = 8;
/// Fewer answers than this to a question are a smoke test, not a measurement.
const SMALL: usize = 20;

#[derive(Clone, PartialEq)]
enum Answer {
    Score(f64),
    Choice(String),
    /// an answer of a kind not compared (rate, box)
    Other,
    Failed,
}

/// A question as asked: its text, its kind and its options.
struct Question {
    text: String,
    /// `noul`, `choice`, `score` or `box`
    kind: String,
    /// a choice's options
    options: Vec<String>,
}

/// One API's answers, from all its traces.
#[derive(Default)]
struct Api {
    urls: BTreeSet<String>,
    models: BTreeSet<String>,
    /// the models its responses named
    answered_by: BTreeSet<String>,
    files: usize,
    /// question → value → the answer
    answers: HashMap<usize, BTreeMap<String, Answer>>,
    /// answers to what it had answered already, in this trace or another
    repeats: usize,
    /// questions in requests that were never sent (a picture that couldn't
    /// be read), so never answered: failed, with no value to match
    unsent: usize,
    /// each request's time, from sending to its answer, in µs
    durations: Vec<u64>,
}

#[derive(Default)]
struct All {
    apis: BTreeMap<String, Api>,
    questions: Vec<Question>,
    /// a question's text, kind and options → its index
    index: HashMap<String, usize>,
    /// a value as it is shown: pictures by their files
    shown: HashMap<String, String>,
}

/// `files`: (name, text) of each trace; `with`: the API whose answers are
/// taken as right.
pub fn compare(files: &[(String, String)], with: &str) -> Result<String, String> {
    let mut all = All::default();
    // the reference's traces first, so its questions come first
    let mut read = Vec::new();
    for (name, text) in files {
        read.push((name, api_of(name, text)?, text));
    }
    read.sort_by_key(|(_, api, _)| api != with);
    for (name, api, text) in read {
        all.add(&api, text).map_err(|e| format!("{name}: {e}"))?;
    }
    let names: Vec<&String> = all.apis.keys().collect();
    if !all.apis.contains_key(with) {
        let list = names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ");
        return Err(format!("--with {with}: no trace of it here; these are traces of {list}"));
    }
    if all.apis.len() < 2 {
        return Err(format!("only traces of {with}: give traces of another API to compare with it"));
    }
    Ok(all.report(with))
}

/// The API a trace is of: its name, or for the environment's settings, the URL.
fn api_of(file: &str, text: &str) -> Result<String, String> {
    let head = text.lines().next().and_then(|l| serde_json::from_str::<Json>(l).ok());
    match head.filter(|h| h["deem_trace"] == 1) {
        Some(h) => Ok(h["api"].as_str().or(h["url"].as_str()).unwrap_or("?").to_owned()),
        None => Err(format!("{file}: not a deem trace: the first line should be {{\"deem_trace\":1,…}}")),
    }
}

impl All {
    fn add(&mut self, name: &str, text: &str) -> Result<(), String> {
        let mut lines = text.lines();
        let head: Json = lines.next().and_then(|l| serde_json::from_str(l).ok()).unwrap_or_default();
        // a trace names its URL since requests' bodies are in it too
        if head["url"].is_null() {
            return Err("written by an older deem, without the requests compare needs: trace the run again".into());
        }
        let api = self.apis.entry(name.to_owned()).or_default();
        api.files += 1;
        let field = |k: &str| head[k].as_str().map(str::to_owned);
        api.urls.extend(field("url"));
        api.models.extend(field("model"));
        let lines: Vec<&str> = lines.collect();
        // request → (when it went, its body, its pictures)
        let mut sent: HashMap<u64, (u64, Json, BTreeMap<String, String>)> = HashMap::new();
        for (i, line) in lines.iter().enumerate() {
            let (t, ev) = match serde_json::from_str::<Line>(line) {
                Ok(l) => (l.t, l.ev),
                // a last line cut short by a kill
                Err(_) if i + 1 == lines.len() => break,
                Err(_) => return Err(format!("not a deem trace: line {} is not an event", i + 2)),
            };
            match ev {
                Event::Request { request, body: Some(body), pictures, .. } => {
                    sent.insert(request, (t, serde_json::from_str(&body.0).unwrap_or_default(), pictures));
                }
                Event::Request { body: None, questions, .. } => {
                    self.apis.get_mut(name).expect("added").unsent += questions.len();
                }
                Event::Answered { request, result, response, .. } => {
                    let Some((from, body, pictures)) = sent.remove(&request) else { continue };
                    let response: Json = response.and_then(|r| serde_json::from_str(&r.0).ok()).unwrap_or_default();
                    self.answered(name, t.saturating_sub(from), &body, &pictures, result == "ok", &response);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A request's answers, one for each of its questions.
    fn answered(
        &mut self,
        name: &str,
        took: u64,
        body: &Json,
        pictures: &BTreeMap<String, String>,
        ok: bool,
        response: &Json,
    ) {
        let state = body["state"].to_string();
        if !self.shown.contains_key(&state) {
            let mut shown = match &body["state"] {
                Json::String(s) => s.clone(),
                v => v.to_string(),
            };
            for (id, file) in pictures {
                shown = shown.replace(id, file);
            }
            self.shown.insert(state.clone(), shown);
        }
        let api = self.apis.get_mut(name).expect("added");
        api.durations.push(took);
        api.answered_by.extend(response["model"].as_str().map(str::to_owned));
        let Some(questions) = body["questions"].as_object() else { return };
        for (qn, q) in questions {
            let kind = q["type"].as_str().unwrap_or_default().to_owned();
            let options: Vec<String> = match &q["criteria"] {
                Json::Object(m) if kind == "choice" => m.keys().cloned().collect(),
                _ => Vec::new(),
            };
            let key = format!("{}\n{kind}\n{}", q["instructions"], q["criteria"]);
            let next = self.questions.len();
            let i = *self.index.entry(key).or_insert(next);
            if i == next {
                let text = q["instructions"].as_str().unwrap_or_default().to_owned();
                self.questions.push(Question { text, kind: kind.clone(), options });
            }
            let a = &response["answers"][qn];
            // an answer of another kind than the question's is no answer
            let answer = match (ok, kind.as_str()) {
                (false, _) => Answer::Failed,
                (true, "noul") => a["noul"].as_f64().map_or(Answer::Failed, Answer::Score),
                (true, "choice") => a["choice"].as_str().map_or(Answer::Failed, |c| Answer::Choice(c.into())),
                (true, _) if a.is_null() => Answer::Failed,
                (true, _) => Answer::Other,
            };
            let api = self.apis.get_mut(name).expect("added");
            let theirs = api.answers.entry(i).or_default();
            match theirs.get(&state) {
                // an answer stands; a failure gives way to one
                Some(Answer::Failed) | None => {
                    theirs.insert(state.clone(), answer);
                }
                Some(_) if answer != Answer::Failed => api.repeats += 1,
                Some(_) => {}
            }
        }
    }

    fn report(&self, with: &str) -> String {
        let truth = &self.apis[with];
        let others: Vec<(&str, &Api)> =
            (self.apis.iter()).filter(|(n, _)| *n != with).map(|(n, a)| (n.as_str(), a)).collect();
        // the questions with the most answers to compare first: a row of 300
        // says more than a row of 6; ties as first asked
        let mut order: Vec<usize> = (0..self.questions.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(answered(answers(truth, i))));
        // each question's values that every API answered, so the APIs are
        // compared on the same ones
        let common: Vec<Vec<&str>> = (0..self.questions.len())
            .map(|i| {
                (answers(truth, i).iter())
                    .filter(|(_, t)| **t != Answer::Failed)
                    .filter(|(s, _)| {
                        others.iter().all(|(_, a)| answers(a, i).get(*s).is_some_and(|a| *a != Answer::Failed))
                    })
                    .map(|(s, _)| s.as_str())
                    .collect()
            })
            .collect();
        // a kind's rows: the questions every API answered about some value,
        // the most compared first
        let rows = |kind: &str| -> Vec<usize> {
            let mut rows: Vec<usize> =
                (order.iter().copied()).filter(|&i| self.questions[i].kind == kind && !common[i].is_empty()).collect();
            rows.sort_by_key(|&i| std::cmp::Reverse(common[i].len()));
            rows
        };
        let (asks, choices) = (rows("noul"), rows("choice"));
        let mut out = self.apis_table();
        out += &banner(&format!("every API against {with}"));
        for (head, rows) in [("ask", &asks), ("choose", &choices)] {
            if !rows.is_empty() {
                out += &self.side(head, rows, &common, &others, with);
            }
        }
        out += &self.left_out(&order, &common, &others, with);
        for (name, api) in &others {
            out += &self.section(name, api, with, &order);
        }
        out
    }

    /// The questions the side-by-side tables leave out though NAME and some
    /// other API both answered them, as no value had an answer from every
    /// API: a table of its own, each column of the values that API and NAME
    /// both answered.
    fn left_out(&self, order: &[usize], common: &[Vec<&str>], others: &[(&str, &Api)], with: &str) -> String {
        let truth = &self.apis[with];
        let rows: Vec<(&str, Vec<String>)> = (order.iter().copied())
            .filter(|&i| matches!(self.questions[i].kind.as_str(), "noul" | "choice") && common[i].is_empty())
            .filter(|&i| others.iter().any(|(_, a)| !both(a, truth, i).is_empty()))
            .map(|i| {
                let cells = (others.iter())
                    .map(|(_, a)| {
                        let both = both(a, truth, i);
                        agreement(both.iter().filter(|(_, a, t)| same(a, t)).count(), both.len())
                    })
                    .collect();
                (self.questions[i].text.as_str(), cells)
            })
            .collect();
        if rows.is_empty() {
            return String::new();
        }
        let cw: Vec<usize> = (others.iter().enumerate())
            .map(|(j, (name, _))| {
                (rows.iter().map(|r| r.1[j].chars().count())).chain([name.chars().count()]).max().unwrap_or(0)
            })
            .collect();
        let qw = WIDTH.saturating_sub(cw.iter().map(|w| 2 + w).sum::<usize>()).max(20);
        let line = |q: &str, cells: &[String]| {
            let mut l = format!("{:<qw$}", fit(q, qw));
            for (c, w) in cells.iter().zip(&cw) {
                let _ = write!(l, "  {c:>w$}");
            }
            l + "\n"
        };
        let mut out = format!("\nnot every API answered these: each as often as it answers as {with} does\n");
        out += &line("", &others.iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>());
        for (q, cells) in &rows {
            out += &line(q, cells);
        }
        out
    }

    /// The APIs: where each is and which model it is, then how many
    /// answers it gave and how fast, each a table.
    fn apis_table(&self) -> String {
        let nw = self.apis.keys().map(|n| n.chars().count()).max().unwrap_or(0);
        let list = |s: &BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>().join(", ");
        let uw = self.apis.values().map(|a| list(&a.urls).chars().count()).max().unwrap_or(0);
        let mut out = String::new();
        for (name, api) in &self.apis {
            // a model the responses named that the traces didn't, by its
            // file's name if it is a path
            let by: Vec<&str> = (api.answered_by.iter())
                .filter(|m| !api.models.contains(*m))
                .map(|m| m.rsplit(['/', '\\']).next().unwrap_or(m))
                .collect();
            let by = if by.is_empty() { String::new() } else { format!(", answered by {}", by.join(", ")) };
            let _ = writeln!(out, "{name:<nw$}  {:<uw$}  {}{by}", list(&api.urls), list(&api.models));
        }
        let _ = writeln!(out, "\n{:<nw$}  {:>7}  {:>6}  {:>14}", "", "answers", "failed", "median request");
        for (name, api) in &self.apis {
            let all = api.answers.values().flat_map(|q| q.values());
            let answered = all.clone().filter(|a| **a != Answer::Failed).count();
            let failed = all.count() - answered + api.unsent;
            let median = median(&api.durations).map_or("-".into(), |m| format!("{:.0} ms", m / 1000.0));
            let _ = writeln!(out, "{name:<nw$}  {answered:>7}  {failed:>6}  {median:>14}");
        }
        let repeats: Vec<String> =
            (self.apis.iter()).filter(|(_, a)| a.repeats > 0).map(|(n, a)| format!("{n} {}", a.repeats)).collect();
        if !repeats.is_empty() {
            let _ = writeln!(out, "answered again, the first answer kept: {}", repeats.join(", "));
        }
        out
    }

    /// A table of how often each API answered as `with` did, a row a
    /// question and a column an API, under a row for all of them: of the
    /// values every API answered (`common`), so each column counts the
    /// same ones. The rows of fewer than SMALL come after a line that says
    /// what they are.
    fn side(&self, head: &str, rows: &[usize], common: &[Vec<&str>], others: &[(&str, &Api)], with: &str) -> String {
        let truth = &self.apis[with];
        let all = match rows.len() {
            1 => "1 question".to_owned(),
            n => format!("all {n} questions"),
        };
        let mut table: Vec<SideRow> = vec![(all, 0, vec![(0, 0); others.len()])];
        for &i in rows {
            let n = common[i].len();
            let theirs = answers(truth, i);
            let cells: Vec<(usize, usize)> = (others.iter())
                .map(|(_, a)| {
                    let ours = answers(a, i);
                    (common[i].iter().filter(|s| same(&ours[**s], &theirs[**s])).count(), n)
                })
                .collect();
            table[0].1 += n;
            for (all, (k, b)) in table[0].2.iter_mut().zip(&cells) {
                *all = (all.0 + k, all.1 + b);
            }
            table.push((self.questions[i].text.clone(), n, cells));
        }
        let text: Vec<Vec<String>> =
            table.iter().map(|r| r.2.iter().map(|&(k, b)| agreement(k, b)).collect()).collect();
        // each column under a head of two lines
        let nw = [table[0].1.to_string().len(), "answers".len(), "compared".len()].into_iter().max().unwrap_or(0);
        let cw: Vec<usize> = (others.iter().enumerate())
            .map(|(j, (name, _))| {
                (text.iter().map(|r| r[j].chars().count()))
                    .chain([name.chars().count(), "agrees".len()])
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let qw = WIDTH.saturating_sub(2 + nw + cw.iter().map(|w| 2 + w).sum::<usize>()).max(20);
        let line = |q: &str, n: &str, cells: &[String]| {
            let mut l = format!("{:<qw$}  {n:>nw$}", fit(q, qw));
            for (c, w) in cells.iter().zip(&cw) {
                let _ = write!(l, "  {c:>w$}");
            }
            l + "\n"
        };
        let names: Vec<String> = others.iter().map(|(n, _)| n.to_string()).collect();
        let mut out = format!("\n{head}: how often each API answers as {with} does\n");
        out += &line("", "answers", &names);
        out += &line("", "compared", &vec!["agrees".to_owned(); others.len()]);
        let width = qw + 2 + nw + cw.iter().map(|w| 2 + w).sum::<usize>();
        let mut small = false;
        for (k, ((q, n, _), cells)) in table.iter().zip(&text).enumerate() {
            if k > 0 && *n < SMALL && !small {
                small = true;
                out += &smoke(width);
            }
            out += &line(q.as_str(), &n.to_string(), cells);
        }
        out
    }

    /// One API's answers against `with`'s, on the values both answered:
    /// how each ask differs, what it chose for each of `with`'s choices,
    /// what wasn't compared, and the largest differences.
    fn section(&self, name: &str, api: &Api, with: &str, order: &[usize]) -> String {
        let truth_api = &self.apis[with];
        // a kind's questions both answered, the most compared first
        let rows = |kind: &str| -> Vec<(usize, usize)> {
            let mut rows: Vec<(usize, usize)> = (order.iter().copied())
                .filter(|&i| self.questions[i].kind == kind)
                .map(|i| (i, both(api, truth_api, i).len()))
                .filter(|&(_, n)| n > 0)
                .collect();
            rows.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            rows
        };
        let (asks, choices) = (rows("noul"), rows("choice"));
        let mut out = banner(&format!("{name} against {with}"));
        let mut apart: Vec<(f64, f64, usize, &str)> = Vec::new();
        if !asks.is_empty() {
            // which way they differ, each column under a head of two lines,
            // the names cut so both columns leave the question 20
            let room = (WIDTH - 20 - (2 + 5) - (2 + 6) - 4) / 2;
            let (n, w) = (fit(name, room - 5), fit(with, room - 4));
            let (yes, no) = ([format!("{n} yes,"), format!("{w} no")], [format!("{n} no,"), format!("{w} yes")]);
            let w = |h: &[String; 2]| h.iter().map(|s| s.chars().count()).max().unwrap_or(0);
            let (yw, nw) = (w(&yes), w(&no));
            let qw = WIDTH.saturating_sub((2 + 5) + (2 + 6) + (2 + yw) + (2 + nw)).max(20);
            let _ = writeln!(out, "\n{:<qw$}  {:>5}  {:>6}  {:>yw$}  {:>nw$}", "ask", "AUC", "offset", yes[0], no[0]);
            let _ = writeln!(out, "{:<qw$}  {:>5}  {:>6}  {:>yw$}  {:>nw$}", "", "", "", yes[1], no[1]);
            let width = qw + (2 + 5) + (2 + 6) + (2 + yw) + (2 + nw);
            let mut small = false;
            for &(i, n) in &asks {
                let scored: Vec<(&str, f64, f64)> = (both(api, truth_api, i).into_iter())
                    .filter_map(|(s, a, t)| match (a, t) {
                        (Answer::Score(x), Answer::Score(y)) => Some((s, *x, *y)),
                        _ => None,
                    })
                    .collect();
                apart.extend(scored.iter().map(|(s, x, y)| (*x, *y, i, *s)));
                let pairs: Vec<(f64, f64)> = scored.iter().map(|(_, x, y)| (*x, *y)).collect();
                let cells = match pairs.len() {
                    0 => ["-".to_owned(), "-".into(), "-".into(), "-".into()],
                    n => {
                        let auc = auc(&pairs).map_or("-".into(), |a| format!("{a:.2}"));
                        // + for a mean that rounds to 0, not -0.00
                        let mean = pairs.iter().map(|(x, y)| x - y).sum::<f64>() / n as f64 + 0.0;
                        let offset = format!("{mean:+.2}").replace("-0.00", "+0.00");
                        let count = |f: fn(&(f64, f64)) -> bool| pairs.iter().filter(|p| f(p)).count().to_string();
                        [auc, offset, count(|(x, y)| *x > 0.5 && *y <= 0.5), count(|(x, y)| *x <= 0.5 && *y > 0.5)]
                    }
                };
                if n < SMALL && !small {
                    small = true;
                    out += &smoke(width);
                }
                let [a, o, yes, no] = &cells;
                let _ =
                    writeln!(out, "{:<qw$}  {a:>5}  {o:>6}  {yes:>yw$}  {no:>nw$}", fit(&self.questions[i].text, qw));
            }
        }
        // the choices it didn't make as `with` did, each a table
        let mut head = false;
        for &(i, _) in &choices {
            let both = both(api, truth_api, i);
            if both.iter().all(|(_, a, t)| same(a, t)) {
                continue;
            }
            if !head {
                head = true;
                let _ = writeln!(out, "\nchoices that differ: {with}'s answer down, {name}'s across");
            }
            let _ =
                write!(out, "\n{}\n{}", fit(&self.questions[i].text, WIDTH), matrix(&self.questions[i].options, &both));
        }
        // what wasn't compared, and why
        let mut apart_notes = Vec::new();
        for &i in order {
            let q = &self.questions[i];
            let (truth, ours) = (answers(truth_api, i), answers(api, i));
            let only = |a: &BTreeMap<String, Answer>, b: &BTreeMap<String, Answer>| {
                a.keys().filter(|s| !b.contains_key(*s)).count()
            };
            let failed = |a: &BTreeMap<String, Answer>| a.values().filter(|a| **a == Answer::Failed).count();
            let notes: Vec<String> = match q.kind.as_str() {
                "noul" | "choice" => [
                    (only(truth, ours), format!("only in {with}")),
                    (only(ours, truth), format!("only in {name}")),
                    (failed(ours), format!("failed in {name}")),
                    (failed(truth), format!("failed in {with}")),
                ]
                .into_iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, what)| format!("{n} {what}"))
                .collect(),
                kind => {
                    let kind = if kind == "score" { "rate" } else { kind };
                    vec![format!("{kind}, not compared: {with} {}, {name} {}", truth.len(), ours.len())]
                }
            };
            if !notes.is_empty() {
                apart_notes.push((q.text.as_str(), notes.join(", ")));
            }
        }
        if !apart_notes.is_empty() {
            // the question as wide as the longest note leaves it
            let longest = apart_notes.iter().map(|(_, n)| n.chars().count()).max().unwrap_or(0);
            let nw = WIDTH.saturating_sub(2 + longest).clamp(20, 48);
            let _ = writeln!(out, "\nnot compared");
            for (q, notes) in &apart_notes {
                let _ = writeln!(out, "{}", fit(&format!("{:<nw$}  {notes}", fit(q, nw)), WIDTH));
            }
        }
        if !apart.is_empty() {
            // the answers on the other side of 0.5 first, the furthest first
            let flip = |(x, y): (f64, f64)| (x > 0.5) != (y > 0.5);
            apart.sort_by(|p, q| {
                (flip((q.0, q.1)).cmp(&flip((p.0, p.1))))
                    .then((q.0 - q.1).abs().total_cmp(&(p.0 - p.1).abs()))
                    .then(p.2.cmp(&q.2))
                    .then(p.3.cmp(q.3))
            });
            let (aw, ww) = (name.chars().count().max(4), with.chars().count().max(4));
            let indent = " ".repeat(2 + aw + 2 + ww + 2 + 5 + 2);
            let _ = writeln!(out, "\nlargest differences (≠: on the other side of 0.5)");
            let _ = writeln!(out, "  {name:>aw$}  {with:>ww$}  {:>5}", "Δ");
            // the question, then the value on a line of its own, so a long
            // question never hides which value it was
            for (x, y, i, s) in apart.into_iter().take(LARGEST) {
                let value = self.shown.get(s).map_or(s, String::as_str);
                let mark = if flip((x, y)) { '≠' } else { ' ' };
                let d = format!("{:+.2}", x - y);
                let row = format!("{mark} {x:>aw$.2}  {y:>ww$.2}  {d:>5}  {}", self.questions[i].text);
                let _ = writeln!(out, "{}", fit(&row, WIDTH));
                let _ = writeln!(out, "{}", fit(&format!("{indent}{value}"), WIDTH));
            }
        }
        out
    }
}

/// A row of a side-by-side table: its question, n, and each API's (same,
/// both answered).
type SideRow = (String, usize, Vec<(usize, usize)>);

/// The values both `api` and `truth` answered question `i` about, with
/// each one's answer.
fn both<'a>(api: &'a Api, truth: &'a Api, i: usize) -> Vec<(&'a str, &'a Answer, &'a Answer)> {
    let truth = answers(truth, i);
    (answers(api, i).iter())
        .filter_map(|(s, a)| Some((s.as_str(), a, truth.get(s)?)))
        .filter(|(_, a, t)| **a != Answer::Failed && **t != Answer::Failed)
        .collect()
}

/// How many of the answers aren't failures.
fn answered(answers: &BTreeMap<String, Answer>) -> usize {
    answers.values().filter(|a| **a != Answer::Failed).count()
}

/// `k` of `b` answers the same: of fewer than SMALL, as a fraction; of
/// more, as a count and its share.
fn agreement(k: usize, b: usize) -> String {
    match b {
        0 => "-".into(),
        b if b < SMALL => format!("{k}/{b}"),
        b => format!("{k} ({:.0}%)", 100.0 * k as f64 / b as f64),
    }
}

/// A part's title between two rules as wide as the report, after a blank
/// line, so where each part starts shows from afar.
fn banner(title: &str) -> String {
    let rule = "═".repeat(WIDTH);
    format!("\n{rule}\n{}\n{rule}\n", fit(title, WIDTH))
}

/// The line before a table's rows of fewer than SMALL answers, `w` wide.
fn smoke(w: usize) -> String {
    let say = format!("── fewer than {SMALL} answers: a smoke test, not a measurement ");
    let pad = w.saturating_sub(say.chars().count());
    fit(&(say + &"─".repeat(pad)), w) + "\n"
}

/// How often an API chose each option for each of the truth's answers: a
/// row for the truth's answer and a column for the API's, each an option
/// either chose, in the order of `options`; a cell a count (· for none).
fn matrix(options: &[String], both: &[(&str, &Answer, &Answer)]) -> String {
    let mut cells: HashMap<(&str, &str), usize> = HashMap::new();
    let mut names: Vec<&str> = options.iter().map(String::as_str).collect();
    for (_, a, t) in both {
        if let (Answer::Choice(a), Answer::Choice(t)) = (a, t) {
            *cells.entry((t.as_str(), a.as_str())).or_default() += 1;
            // an option the question didn't offer, after those it did
            for o in [t, a] {
                if !names.contains(&o.as_str()) {
                    names.push(o);
                }
            }
        }
    }
    // the same options down and across, so the same answers are the diagonal
    let rows: Vec<&str> = names.iter().copied().filter(|o| cells.keys().any(|(t, a)| t == o || a == o)).collect();
    let cols = rows.clone();
    let count = |t: &str, a: &str| cells.get(&(t, a)).map_or("·".into(), |n| n.to_string());
    let rw = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0).min(16);
    let digits = |a: &str| rows.iter().map(|t| count(t, a).chars().count()).max().unwrap_or(1);
    // the columns' names cut as short as the width needs, to 3
    let longest = cols.iter().map(|c| c.chars().count()).max().unwrap_or(0);
    let widths =
        |cut: usize| -> Vec<usize> { cols.iter().map(|c| c.chars().count().min(cut).max(digits(c))).collect() };
    let cut = (3..=longest.max(3))
        .rev()
        .find(|&cut| 2 + rw + widths(cut).iter().map(|w| 2 + w).sum::<usize>() <= WIDTH)
        .unwrap_or(3);
    let cw = widths(cut);
    let mut out = format!("  {:rw$}", "");
    for (c, w) in cols.iter().zip(&cw) {
        let _ = write!(out, "  {:>w$}", fit(c, cut));
    }
    out += "\n";
    for t in &rows {
        let _ = write!(out, "  {:<rw$}", fit(t, rw));
        for (a, w) in cols.iter().zip(&cw) {
            let _ = write!(out, "  {:>w$}", count(t, a));
        }
        out += "\n";
    }
    out
}

/// An API's answers to question `i`, by value.
fn answers(api: &Api, i: usize) -> &BTreeMap<String, Answer> {
    static NONE: BTreeMap<String, Answer> = BTreeMap::new();
    api.answers.get(&i).unwrap_or(&NONE)
}

/// The same answer: on the same side of 0.5, or the same option.
fn same(a: &Answer, t: &Answer) -> bool {
    match (a, t) {
        (Answer::Score(x), Answer::Score(y)) => (*x > 0.5) == (*y > 0.5),
        (a, t) => a == t,
    }
}

/// How well `x` orders what `y` says is yes (above 0.5) before what it says
/// is no: the chance that a yes scores higher than a no, ties half. None
/// without both.
fn auc(pairs: &[(f64, f64)]) -> Option<f64> {
    let x: Vec<f64> = pairs.iter().map(|p| p.0).collect();
    let r = ranks(&x);
    let yes: Vec<usize> = (0..pairs.len()).filter(|&i| pairs[i].1 > 0.5).collect();
    let (p, n) = (yes.len() as f64, (pairs.len() - yes.len()) as f64);
    if p == 0.0 || n == 0.0 {
        return None;
    }
    let sum: f64 = yes.iter().map(|&i| r[i]).sum();
    Some((sum - p * (p + 1.0) / 2.0) / (p * n))
}

/// Each value's rank from 1, ties sharing the mean of theirs.
fn ranks(v: &[f64]) -> Vec<f64> {
    let mut i: Vec<usize> = (0..v.len()).collect();
    i.sort_by(|&p, &q| v[p].total_cmp(&v[q]));
    let mut r = vec![0.0; v.len()];
    let mut s = 0;
    while s < i.len() {
        let e = (s..i.len()).find(|&e| v[i[e]] != v[i[s]]).unwrap_or(i.len());
        let mean = (s + e - 1) as f64 / 2.0 + 1.0;
        i[s..e].iter().for_each(|&j| r[j] = mean);
        s = e;
    }
    r
}

fn median(v: &[u64]) -> Option<f64> {
    let mut v = v.to_vec();
    v.sort_unstable();
    match v.len() {
        0 => None,
        n if n % 2 == 1 => Some(v[n / 2] as f64),
        n => Some((v[n / 2 - 1] + v[n / 2]) as f64 / 2.0),
    }
}

/// `s`, cut to `w` characters with … if it is longer.
fn fit(s: &str, w: usize) -> String {
    match s.chars().count() > w {
        true => s.chars().take(w.saturating_sub(1)).collect::<String>() + "…",
        false => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trace of `api`: each request a value, one ask "q?", and its
    /// answer as the API sent it (None: the request was never sent).
    fn trace(api: &str, model: &str, said: &str, asks: &[(&str, Option<Json>)]) -> String {
        let head =
            serde_json::json!({"deem_trace": 1, "clock": "real", "api": api, "url": "http://x/v1", "model": model});
        let mut out = format!("{head}\n");
        for (n, (state, answer)) in asks.iter().enumerate() {
            let q = serde_json::json!({"q0": {"instructions": "q?", "type": "noul"}});
            let Some(answer) = answer else {
                let req = serde_json::json!({"t": n, "ev": "request", "request": n, "sender": 0, "questions": [n]});
                out += &format!("{req}\n");
                continue;
            };
            let body = serde_json::json!({"model": model, "state": state, "questions": q});
            let req =
                serde_json::json!({"t": n, "ev": "request", "request": n, "sender": 0, "questions": [n], "body": body});
            let (result, response) = match answer {
                Json::String(e) => (e.clone(), Json::Null),
                a => ("ok".to_owned(), serde_json::json!({"model": said, "answers": {"q0": a}})),
            };
            let ans = serde_json::json!({"t": n + 1000, "ev": "answered", "request": n, "result": result,
                "attempts": 1, "tokens": 1, "scores": [], "response": response});
            out += &format!("{req}\n{ans}\n");
        }
        out
    }

    // An answer of the wrong kind, a request that failed, one never sent,
    // and a model the responses name: each counted where it belongs, and the
    // differences listed with their own values.
    #[test]
    fn answers_that_are_not_scores() {
        let noul = |x: f64| Some(serde_json::json!({"type": "noul", "noul": x}));
        let a = trace(
            "a",
            "m1",
            "m1",
            &[("r1", noul(0.9)), ("r2", noul(0.1)), ("r3", noul(0.8)), ("r4", Some("api 500".into()))],
        );
        let wrong = Some(serde_json::json!({"type": "bool", "bool": true}));
        let b = trace(
            "b",
            "m1",
            "/x/m2.gguf",
            &[("r1", wrong), ("r2", noul(0.2)), ("r3", noul(0.3)), ("r4", noul(0.5)), ("r5", None)],
        );
        let out = compare(&[("a.jsonl".into(), a.clone()), ("b.jsonl".into(), b)], "a").unwrap();
        assert!(out.starts_with("a  http://x/v1  m1\nb  http://x/v1  m1, answered by m2.gguf\n"), "{out}");
        let row = |n: &str| out.lines().find(|l| l.starts_with(&format!("{n} ")) && !l.contains("http")).unwrap();
        assert!(row("a").starts_with("a        3       1"), "{out}");
        assert!(row("b").starts_with("b        3       2"), "{out}");
        // of 3 a answered, both answered 2: r3 the other way
        let side = out.lines().find(|l| l.starts_with("q?  ")).unwrap();
        assert!(side.split_whitespace().eq(["q?", "2", "1/2"]), "{out}");
        let detail: Vec<&str> =
            out.lines().filter(|l| l.starts_with("q?  ")).nth(1).unwrap().split_whitespace().collect();
        assert_eq!(detail, ["q?", "1.00", "-0.20", "0", "1"], "{out}");
        // which way: each column's head names both APIs
        assert!(out.contains("b yes,  b no,\n") && out.contains("  a no  a yes\n"), "{out}");
        assert!(out.contains("1 failed in b, 1 failed in a\n"), "{out}");
        // the answer the other way first, though r2's are as far apart
        let largest: Vec<&str> = out.split("of 0.5)\n").nth(1).unwrap().lines().skip(1).collect();
        assert_eq!(
            largest,
            [
                "≠ 0.30  0.80  -0.50  q?",
                "                     r3",
                "  0.20  0.10  +0.10  q?",
                "                     r2"
            ],
            "{out}"
        );
        // a trace from before traces named their URL can't be compared
        let old = a.replacen(r#","url":"http://x/v1""#, "", 1);
        let e = compare(&[("old.jsonl".into(), old), ("a.jsonl".into(), a)], "a").unwrap_err();
        assert!(e.starts_with("old.jsonl: written by an older deem"), "{e}");
    }

    /// A trace of `api` with a request for each (value, question, kind,
    /// answer).
    fn traced(api: &str, asked: &[(String, &str, &str, Json)]) -> String {
        let head = serde_json::json!({"deem_trace": 1, "api": api, "url": "http://x/v1", "model": "m"});
        let mut out = format!("{head}\n");
        for (n, (state, q, kind, answer)) in asked.iter().enumerate() {
            let mut question = serde_json::json!({"instructions": q, "type": kind});
            if *kind == "choice" {
                question["criteria"] = serde_json::json!({"x": null, "y": null, "z": null});
            }
            let body = serde_json::json!({"model": "m", "state": state, "questions": {"q0": question}});
            let req =
                serde_json::json!({"t": n, "ev": "request", "request": n, "sender": 0, "questions": [n], "body": body});
            let mut a = serde_json::json!({"type": kind});
            a[*kind] = answer.clone();
            let ans = serde_json::json!({"t": n + 7, "ev": "answered", "request": n, "result": "ok", "attempts": 1,
                "tokens": 1, "scores": [], "response": {"answers": {"q0": a}}});
            out += &format!("{req}\n{ans}\n");
        }
        out
    }

    // Every table's columns line up, whatever the names, counts and
    // questions: 1,200 values, a question longer than the line, a long API
    // name, three APIs.
    #[test]
    fn columns_line_up() {
        let long = "is this a question so long that it can't fit in the column the table has for it, at all?";
        let asked = |shift: f64| -> Vec<(String, &str, &str, Json)> {
            let mut v = Vec::new();
            for i in 0..1200 {
                let x = ((i * 37 % 100) as f64 / 100.0 + shift).clamp(0.0, 1.0);
                v.push((format!("v{i}"), "q?", "noul", serde_json::json!(x)));
                // c chooses the next option for a third of them
                let o = if shift < 0.0 && i % 3 == 0 { i + 1 } else { i };
                v.push((format!("v{i}"), "pick?", "choice", serde_json::json!(["x", "y", "z"][o % 3])));
            }
            for i in 0..3 {
                v.push((format!("w{i}"), long, "noul", serde_json::json!(0.3 * i as f64 + shift.abs())));
            }
            v
        };
        let files = [
            ("a.jsonl".to_owned(), traced("a", &asked(0.0))),
            ("b.jsonl".to_owned(), traced("a-much-longer-api-name", &asked(0.2))),
            ("c.jsonl".to_owned(), traced("c", &asked(-0.3))),
        ];
        let out = compare(&files, "a").unwrap();
        assert!(out.lines().all(|l| l.chars().count() <= WIDTH), "{out}");
        // a table: its header, then its rows, to a blank line
        let table = |head: &str| -> Vec<Vec<&str>> {
            let mut tables = Vec::new();
            let mut lines = out.lines();
            while let Some(l) = lines.next() {
                if l.trim_start().starts_with(head) {
                    let rows = std::iter::once(l).chain(lines.by_ref().take_while(|l| !l.is_empty()));
                    tables.push(rows.collect());
                }
            }
            tables
        };
        for head in ["ask", "choose", "answers"] {
            let tables = table(head);
            assert!(!tables.is_empty(), "{head}\n{out}");
            for mut t in tables {
                // a side-by-side table's title, above its columns
                if t[0].contains(": how often") {
                    t.remove(0);
                }
                let n = t[0].chars().count();
                assert!(t.len() > 1 && t.iter().all(|l| l.chars().count() == n), "{head}:\n{}", t.join("\n"));
            }
        }
        let pick: Vec<&str> = out.lines().find(|l| l.starts_with("pick?  ")).unwrap().split_whitespace().collect();
        assert_eq!(pick[1..], ["1200", "1200", "(100%)", "800", "(67%)"], "{out}");
        // c's choices: a row for each of a's, its lines as wide as its head
        let choices = out.split("\npick?\n").nth(1).unwrap().lines().take_while(|l| !l.is_empty());
        let choices: Vec<&str> = choices.collect();
        assert_eq!(choices[0], "     x    y    z", "{out}");
        assert!(choices.iter().all(|l| l.chars().count() == choices[0].chars().count()), "{out}");
        assert_eq!(choices[1..], ["  x  ·  400    ·", "  y  ·  400    ·", "  z  ·    ·  400"], "{out}");
        // the largest differences: each number ends where its column's name does
        for scores in out.split("of 0.5)\n").skip(1) {
            let head: Vec<char> = scores.lines().next().unwrap().chars().collect();
            let ends: Vec<usize> = (1..=head.len())
                .filter(|&e| {
                    head[e - 1] != ' ' && head.get(e) != Some(&head[e - 1]) && head.get(e).is_none_or(|c| *c == ' ')
                })
                .collect();
            assert_eq!(ends.len(), 3, "{scores}");
            for l in scores.lines().skip(1).step_by(2).take_while(|l| !l.is_empty() && !l.contains("against")) {
                let l: Vec<char> = l.chars().collect();
                assert!(ends.iter().all(|&e| l[e - 1].is_ascii_digit() && l[e] == ' '), "{scores}");
            }
        }
    }

    #[test]
    fn numbers() {
        assert_eq!(ranks(&[0.3, 0.1, 0.3, 0.9]), [2.5, 1.0, 2.5, 4.0]);
        // every yes above every no; every yes below; a tie is half
        assert_eq!(auc(&[(0.9, 0.8), (0.7, 0.9), (0.1, 0.2)]), Some(1.0));
        assert_eq!(auc(&[(0.1, 0.8), (0.9, 0.2)]), Some(0.0));
        assert_eq!(auc(&[(0.5, 0.8), (0.5, 0.2)]), Some(0.5));
        assert_eq!(auc(&[(0.5, 0.8)]), None);
        assert_eq!(median(&[3, 1, 2]), Some(2.0));
        assert_eq!(median(&[]), None);
        assert_eq!(fit(&"x".repeat(81), 80).chars().count(), 80);
        // a fraction under SMALL; over, a count and its share
        assert_eq!(agreement(4, 6), "4/6");
        assert_eq!(agreement(259, 300), "259 (86%)");
        assert_eq!(agreement(0, 40), "0 (0%)");
        assert_eq!(agreement(0, 0), "-");
        assert_eq!(smoke(80).trim_end().chars().count(), 80);
    }
}
