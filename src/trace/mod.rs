//! `--trace FILE`: what happened in a run, one JSON line per event, written
//! as it happens. Starts and ends are separate events, so a run that never
//! finishes still shows what it was waiting on, and a reader skips a last
//! line cut short by a kill. The first line names the format, and what the
//! run sent to (`api`, from the config file, is absent for the
//! environment's settings):
//!
//! ```text
//! {"deem_trace":1,"clock":"real","api":"kev","url":"http://127.0.0.1:8009/v1","model":"kev-latest"}
//! {"t":0,"ev":"ask","record":1,"question":1,"q":"is it an error?","after":[]}
//! {"t":0,"ev":"wave","wave":1,"requests":[1]}
//! {"t":12,"ev":"request","request":1,"sender":0,"questions":[1],"bytes":96}
//! ```
//!
//! A request's event has its body as sent, and its answered event the
//! reply as received: the trace is the run's requests and responses too.
//!
//! `t` is microseconds on the run's clock. Views of the file are made
//! afterwards, from the file alone: `deem trace FILE` (text), `--visual`
//! (a timeline), and `compare` (several traces' answers, API by API).

mod compare;
pub mod stats;
mod views;

pub use compare::compare;
pub use stats::Counts;
pub use views::{report, visual};

use crate::clock::Clock;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// What happened. Questions, requests and waves are numbered from 1 in the
/// order they were made; records in input order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "lowercase")]
pub(crate) enum Event {
    /// a record started
    Record {
        record: usize,
    },
    /// `after`: the answers the record waited for since its last question
    Ask {
        record: usize,
        question: u64,
        q: String,
        after: Vec<u64>,
    },
    /// the record asked what it had asked before, and got that promise
    Reuse {
        record: usize,
        question: u64,
    },
    Wait {
        record: usize,
        question: u64,
    },
    Resume {
        record: usize,
        question: u64,
    },
    End {
        record: usize,
        outcome: String,
    },
    /// a value to judge was cut to `DEEM_MAX_RECORD_BYTES`
    Cut {
        record: usize,
    },
    /// a record's round goes, the `wave`-th it sent: wave k is every
    /// record's k-th round, so a wave has an event for each record in it
    Wave {
        wave: u64,
        questions: usize,
        requests: Vec<u64>,
    },
    /// a request carries one value; `bytes`: its body (absent in older
    /// traces, which also count the values it judged). `body`: the request
    /// as sent, its questions `q0`, `q1`, … in the order of `questions`;
    /// `pictures`: the file each picture ID in it was read from.
    Request {
        request: u64,
        sender: usize,
        questions: Vec<u64>,
        #[serde(default)]
        bytes: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Raw>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pictures: BTreeMap<String, String>,
    },
    /// `tokens` over all attempts; `scores`, the answers that are numbers;
    /// `response`, the last attempt's reply as received, when it was one
    /// (when the request failed, `result` says why)
    Answered {
        request: u64,
        result: String,
        attempts: u64,
        tokens: u64,
        scores: Vec<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response: Option<Raw>,
    },
    Stop {
        why: String,
    },
}

/// JSON as it was sent or received, written into the log as JSON, its keys
/// and numbers as they were; only whitespace between tokens is dropped, so
/// it stays on one line. (Read back, only views see it, through a `Value`:
/// a raw value can't be read inside a tagged event.)
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Raw(String);

impl Raw {
    pub fn new(json: &str) -> Self {
        let (mut out, mut string, mut escaped) = (String::with_capacity(json.len()), false, false);
        for c in json.chars() {
            match c {
                _ if escaped => escaped = false,
                '\\' if string => escaped = true,
                '"' => string = !string,
                c if !string && c.is_ascii_whitespace() => continue,
                _ => {}
            }
            out.push(c);
        }
        Self(out)
    }
}

/// Text that isn't JSON (never seen: deem parsed it, or wrote it) goes as a
/// string, so the line is still an event.
impl Serialize for Raw {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match serde_json::from_str::<&RawValue>(&self.0) {
            Ok(raw) => raw.serialize(s),
            Err(_) => self.0.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Raw {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde_json::Value::deserialize(d).map(|v| Raw(v.to_string()))
    }
}

/// A line of the log: an event and when it happened.
#[derive(Serialize, Deserialize)]
struct Line {
    t: u64,
    #[serde(flatten)]
    ev: Event,
}

/// Where every part of the run reports what happened. It keeps the totals
/// `--stats` prints, and with `--trace` writes each event to the file.
pub(crate) struct Trace {
    clock: Arc<Clock>,
    totals: Mutex<Counts>,
    out: Option<Mutex<Out>>,
}

struct Out {
    path: String,
    file: File,
    /// the first write that failed; nothing is written after it
    error: Option<io::Error>,
}

impl Trace {
    /// Totals only, no file.
    pub fn off(clock: Arc<Clock>) -> Self {
        Self { clock, totals: Mutex::default(), out: None }
    }

    /// `about`: what the first line says the run sent to, after the format.
    pub fn create(path: &str, clock: Arc<Clock>, about: &[(&str, &str)]) -> io::Result<Self> {
        let mut file = File::create(path)?;
        let kind = if clock.is_virtual() { "virtual" } else { "real" };
        let mut head = json!({"deem_trace": 1, "clock": kind});
        about.iter().for_each(|(k, v)| head[*k] = json!(v));
        writeln!(file, "{head}")?;
        let out = Out { path: path.to_owned(), file, error: None };
        Ok(Self { clock, totals: Mutex::default(), out: Some(Mutex::new(out)) })
    }

    pub fn totals(&self) -> Counts {
        crate::lock(&self.totals).clone()
    }

    /// Why the file is incomplete, if a write failed.
    pub fn error(&self) -> Option<String> {
        let out = crate::lock(self.out.as_ref()?);
        out.error.as_ref().map(|e| format!("--trace {}: {e}", out.path))
    }

    /// Each event is written whole, straight to the file, so a killed run
    /// loses at most the event being written.
    fn emit_at(&self, t: u64, e: Event) {
        crate::lock(&self.totals).add(t, &e);
        let Some(out) = &self.out else { return };
        let line = serde_json::to_string(&Line { t, ev: e }).unwrap_or_default() + "\n";
        let mut out = crate::lock(out);
        if out.error.is_none()
            && let Err(e) = out.file.write_all(line.as_bytes())
        {
            out.error = Some(e);
        }
    }

    fn emit(&self, e: Event) {
        self.emit_at(self.clock.now(), e);
    }

    pub fn record(&self, record: usize) {
        self.emit(Event::Record { record });
    }

    /// Record `record` asks `question` (its text `q`), after waiting for
    /// the answers in `after` since its last question.
    pub fn ask(&self, record: usize, question: u64, q: &str, after: Vec<u64>) {
        self.emit(Event::Ask { record, question, q: q.to_owned(), after });
    }

    pub fn reuse(&self, record: usize, question: u64) {
        self.emit(Event::Reuse { record, question });
    }

    /// The record is about to block on `question`. On real time that's a
    /// wait; on virtual time only [`Trace::resume`] can tell.
    pub fn wait(&self, record: usize, question: u64) {
        if !self.clock.is_virtual() {
            self.emit(Event::Wait { record, question });
        }
    }

    /// The record has the answer to `question`, which it looked for at
    /// `since`, and had to block for it if `blocked`. On virtual time it
    /// waited if the answer came later than `since`.
    pub fn resume(&self, record: usize, question: u64, since: u64, blocked: bool) {
        if self.out.is_none() {
            return;
        }
        let now = self.clock.now();
        if !self.clock.is_virtual() {
            if blocked {
                self.emit_at(now, Event::Resume { record, question });
            }
        } else if now > since {
            self.emit_at(since, Event::Wait { record, question });
            self.emit_at(now, Event::Resume { record, question });
        }
    }

    pub fn end(&self, record: usize, outcome: &str) {
        self.emit(Event::End { record, outcome: outcome.to_owned() });
    }

    pub fn cut(&self, record: usize) {
        self.emit(Event::Cut { record });
    }

    pub fn wave(&self, wave: u64, at: u64, questions: usize, requests: Vec<u64>) {
        self.emit_at(at, Event::Wave { wave, questions, requests });
    }

    /// Request `request` goes. `sent` gives its body and the file of each
    /// picture ID in it, if it is sent, built only when there is a file to
    /// write them to.
    pub fn request(
        &self,
        request: u64,
        sender: usize,
        questions: &[u64],
        bytes: usize,
        sent: impl FnOnce() -> Option<(String, BTreeMap<String, String>)>,
    ) {
        let questions = questions.to_vec();
        let (body, pictures) = match self.out.as_ref().and_then(|_| sent()) {
            Some((body, pictures)) => (Some(Raw::new(&body)), pictures),
            None => (None, BTreeMap::new()),
        };
        self.emit(Event::Request { request, sender, questions, bytes, body, pictures });
    }

    /// `response` is the reply as received, if there was one.
    pub fn answered(
        &self,
        request: u64,
        (result, attempts, tokens): (&str, u64, u64),
        scores: Vec<f64>,
        response: Option<&str>,
    ) {
        let response = response.filter(|_| self.out.is_some()).map(Raw::new);
        self.emit(Event::Answered { request, result: result.to_owned(), attempts, tokens, scores, response });
    }

    pub fn stop(&self, why: &str) {
        self.emit(Event::Stop { why: why.to_owned() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip() {
        let all = [
            Event::Record { record: 1 },
            Event::Ask { record: 1, question: 2, q: "q".into(), after: vec![1] },
            Event::Reuse { record: 1, question: 2 },
            Event::Wait { record: 1, question: 2 },
            Event::Resume { record: 1, question: 2 },
            Event::End { record: 1, outcome: "done".into() },
            Event::Cut { record: 1 },
            Event::Wave { wave: 1, questions: 2, requests: vec![1, 2] },
            Event::Request {
                request: 1,
                sender: 3,
                questions: vec![2],
                bytes: 90,
                // read back through a `Value`, whose keys are sorted
                body: Some(Raw::new(r#"{"questions":{"q0":{"type":"noul"}},"state":"x"}"#)),
                pictures: [("6c1e".to_owned(), "cam/1.jpg".to_owned())].into(),
            },
            Event::Answered {
                request: 1,
                result: "ok".into(),
                attempts: 2,
                tokens: 9,
                scores: vec![0.5],
                response: Some(Raw::new(r#"{"answers":{"q0":{"noul":0.5,"type":"noul"}}}"#)),
            },
            Event::Stop { why: "why".into() },
        ];
        for ev in all {
            let line = serde_json::to_string(&Line { t: 7, ev: ev.clone() }).unwrap();
            let back: Line = serde_json::from_str(&line).unwrap();
            assert_eq!((back.t, back.ev), (7, ev), "{line}");
        }
    }

    // A body or reply goes into the log as the JSON it was: keys in its
    // order, numbers as written, not as a string; on one line, whatever
    // whitespace it had between tokens, and only there.
    #[test]
    fn json_is_written_as_it_was() {
        let reply = "{\"answers\": {\"q0\":\n  {\"b\":1, \"a\":1.50,\n \"s\":\"x \\\" y\"}}}\n";
        let ev = Event::Answered {
            request: 1,
            result: "ok".into(),
            attempts: 1,
            tokens: 0,
            scores: vec![],
            response: Some(Raw::new(reply)),
        };
        let line = serde_json::to_string(&Line { t: 0, ev }).unwrap();
        let want = r#""response":{"answers":{"q0":{"b":1,"a":1.50,"s":"x \" y"}}}}"#;
        assert!(line.ends_with(want), "{line}");
        let ev = Event::Stop { why: String::new() };
        let mut line: serde_json::Value = serde_json::to_value(Line { t: 0, ev }).unwrap();
        line["x"] = serde_json::to_value(Raw::new("not-json")).unwrap();
        assert_eq!(line["x"], "not-json", "not JSON: a string, still a line");
    }
}
