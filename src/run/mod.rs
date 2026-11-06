//! The run: a reader thread hands each record to a pool of workers, at most
//! a window of them at once, and the main thread writes their outputs in
//! input order. A record that finishes before the ones ahead of it keeps its
//! output until they print, so the reader also waits while four windows of
//! records are started and not yet printed (`batch.rs`, `UNPRINTED`).

pub mod input;
mod options;
pub mod output;
mod record;

pub use options::RunOptions;

use crate::clock::Clock;
use crate::config::Config;
use crate::engine::{Input, Outcome, Program};
use crate::run::input::{Input as Source, Raw, Reader};
use crate::run::output::{Chunk, NOT_JSON, Output};
use crate::run::record::{Records, Shared};
use crate::trace::Trace;
use crate::trace::stats::Counts;
use crate::transport::api::Judge;
use crate::transport::batch::{Batcher, Limits, RecId};
use crate::transport::picture::{Picture, Pictures};
use crate::transport::sched::{Pool, Rate, Retry};
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel, sync_channel};
use std::time::Duration;

/// A run of a record stops once it has this many unanswered questions, so a
/// program can't queue questions without bound (`repeat(ask(q))`). An
/// `input: "all"` program may have as many as a full window of records.
const MAX_UNANSWERED: usize = 64;
/// Record failures shown one by one; the rest are counted.
const SHOWN: usize = 5;
/// Output chunks a record may have written ahead of the one being printed.
const AHEAD: usize = 16;

pub struct Report {
    pub code: i32,
    /// what the run cost
    pub stats: Counts,
}

enum Item {
    Record(RecId, Receiver<Msg>),
    Unreadable(String),
}

pub(crate) enum Msg {
    Out(Chunk),
    /// how the record ended, and the record as read, for `--failed`
    End(Outcome, Option<String>),
}

/// The whole pipeline, with its ends open so tests can drive it.
/// `program` is what to call the program in messages, its source, and its
/// arguments.
pub fn run(
    (name, program, args): (&str, &str, &[String]),
    opts: &RunOptions,
    cfg: &Config,
    judge: Arc<dyn Judge>,
    input: impl Into<Source>,
    out: impl Write,
    err: &mut dyn Write,
) -> Report {
    let input: Source = input.into();
    let fail = |err: &mut dyn Write, e: String| {
        let _ = writeln!(err, "deem: {e}");
        Report { code: 2, stats: Counts::default() }
    };
    let pictures = Arc::new(Pictures::new());
    let text: Result<Vec<_>, String> =
        opts.rawfile.iter().map(|(n, f)| Ok((n.clone(), input::text_file(f)?))).collect();
    let mut bound = match text {
        Ok(b) => b,
        Err(e) => return fail(err, format!("--rawfile: {e}")),
    };
    // `--img` pictures, held for the run
    let mut held: Vec<Arc<Picture>> = Vec::new();
    for (name, path) in &opts.img {
        match pictures.add_as(path, Some(name)) {
            Ok(p) => {
                bound.push((name.clone(), p.reference.clone()));
                held.push(p);
            }
            Err(e) => return fail(err, format!("--img {name}: {e}")),
        }
    }
    let program = match Program::compile_with(program, args, &opts.named, &bound) {
        Ok(p) => Arc::new(p),
        Err(e) => return fail(err, e.strip_prefix("the program").map_or(e.clone(), |r| format!("{name}{r}"))),
    };
    let all = program.input() == Input::All;
    if all && opts.slurp {
        return fail(err, "-s can't be used with an input \"all\" program: it already reads every record".into());
    }
    let reader = match input {
        Source::Text(src) => Reader::new(src, opts.split.as_deref(), opts.slurp, opts.json_in),
        pictures => Reader::pictures(pictures, opts.split.as_deref(), opts.slurp, opts.json_in),
    };
    let reader = match reader {
        Ok(r) => r,
        Err(e) => return fail(err, e),
    };
    // Tools print lines, also for JSON input.
    let json = reader.json();
    let raw = !json || opts.raw_out || program.is_tool();
    let max_questions = opts.max_questions.map_or(cfg.max_questions, |n| (n > 0).then_some(n));
    let limits = Limits {
        model: cfg.model.clone(),
        window: cfg.window,
        max_questions,
        memo: cfg.memo,
        pictures: pictures.clone(),
        settings: cfg.settings(),
    };
    let clock = Arc::new(Clock::new(cfg.virtual_time));
    let trace = match &opts.trace {
        None => Trace::off(clock.clone()),
        Some(path) => match Trace::create(path, clock.clone(), &about(cfg)) {
            Ok(t) => t,
            Err(e) => return fail(err, format!("--trace: {e}")),
        },
    };
    let trace = Arc::new(trace);
    let retry = Retry { backoff: cfg.backoff, busy_for: cfg.busy_for, notice_after: cfg.notice_after };
    let rate = Rate { requests: cfg.requests_per_second as f64, tokens: cfg.tokens_per_second as f64 };
    let pool = Pool::new(judge, cfg.workers, rate, retry, clock.clone(), trace.clone());
    let batcher = Batcher::new(pool, limits, clock.clone(), trace.clone());
    let mut unfinished = match opts.failed.as_deref().map(std::fs::File::create).transpose() {
        Ok(f) => f.map(io::BufWriter::new),
        Err(e) => return fail(err, format!("--failed: {e}")),
    };
    // one record per line, or NUL-separated when records may hold newlines
    let sep = if opts.split.is_some() { "\0" } else { "\n" };
    let keep = unfinished.is_some();
    let max_unanswered = if all { MAX_UNANSWERED * cfg.window } else { MAX_UNANSWERED };
    let caught = Arc::new(AtomicU64::new(0));
    let shared = Shared {
        program,
        caught: caught.clone(),
        batcher: batcher.clone(),
        pictures: pictures.clone(),
        raw,
        max_bytes: cfg.max_record_bytes,
        keep,
        max_unanswered,
    };
    let pool = match Records::new(shared) {
        Ok(p) => p,
        Err(e) => return fail(err, e),
    };
    let records = match all {
        true => read(reader.all(), batcher.clone(), pool.clone()),
        false => read(reader, batcher.clone(), pool.clone()),
    };

    let mut out = Output::new(out);
    let (mut failed, mut records_failed) = (false, 0);
    // a program read a field of a text record: likely -J was forgotten
    let mut indexed_text = false;
    let mut unwritten = None;
    let written = write(
        &records,
        &batcher,
        &|seq| pool.head(seq),
        &mut out,
        &mut |e, what| {
            failed |= what != Note::Aside;
            if what == Note::Record {
                indexed_text |= !json && e.contains(": cannot index \"");
                records_failed += 1;
                if records_failed > SHOWN {
                    return;
                }
            }
            let _ = writeln!(err, "deem: {e}");
        },
        &mut |text| {
            if let Some(f) = &mut unfinished
                && let Err(e) = f.write_all(text.as_bytes()).and_then(|_| f.write_all(sep.as_bytes()))
            {
                unwritten.get_or_insert(e);
            }
        },
    );
    if let Some(e) = unfinished.as_mut().and_then(|f| f.flush().err()).or(unwritten) {
        let _ = writeln!(err, "deem: --failed: {e}");
        failed = true;
    }
    if let Err(e) = &written {
        batcher.shutdown(e.to_string());
    }
    // no record prints from here on
    pool.close();
    let stopped = match written {
        Ok(n) => n,
        Err(e) => {
            if e.kind() == io::ErrorKind::BrokenPipe {
                return Report { code: 141, stats: trace.totals() };
            }
            let _ = writeln!(err, "deem: writing the output: {e}");
            return Report { code: 2, stats: trace.totals() };
        }
    };
    batcher.close();
    let _ = out.flush();

    let stats = trace.totals();
    if opts.stats {
        let _ = err.write_all(stats.report(clock.now(), max_questions).as_bytes());
    }
    let cuts = stats.cuts;
    if cuts > 0 {
        let max = cfg.max_record_bytes;
        let _ = writeln!(
            err,
            "deem: cut {cuts} judged value(s) to DEEM_MAX_RECORD_BYTES ({max}); the model saw only the start of each"
        );
    }
    // records that failed: the first few were shown as they came
    if records_failed > SHOWN {
        let _ = writeln!(err, "deem: … and {} more", records_failed - SHOWN);
    }
    let stop = batcher.stopped();
    if let Some(why) = &stop {
        let _ = writeln!(err, "deem: {why}");
        if opts.failed.is_some() {
            // `--failed` has what was read and didn't finish; the rest is unread
            let n = stats.records;
            let _ = writeln!(err, "deem: stopped after reading {n} records; any after them weren't read");
        }
    }
    if let Some(e) = trace.error() {
        let _ = writeln!(err, "deem: {e}");
    }
    // with one record, its error above says it all; a stop with no record
    // failed (a wrong setting) is said by its one message
    if records_failed > 0 && stats.records > 1 {
        let n = stats.records;
        let what = match stopped {
            0 => format!("{records_failed} of {n} records failed"),
            s => format!("{records_failed} of {n} records failed and {s} didn't finish"),
        };
        let _ = match &opts.failed {
            None => writeln!(err, "deem: {what}; --failed FILE keeps them to run again"),
            Some(f) => writeln!(err, "deem: {what}; they are in {f}"),
        };
    }
    match caught.load(Ordering::SeqCst) {
        0 => {}
        1 => {
            let _ = writeln!(err, "deem: 1 failed answer was caught by a try or ? in the program");
        }
        n => {
            let _ = writeln!(err, "deem: {n} failed answers were caught by a try or ? in the program");
        }
    }
    if indexed_text {
        let _ = writeln!(err, "deem: the input was read as text; pass -J to read JSON records");
    }
    let code = match () {
        _ if failed || stop.is_some() => 2,
        _ if out.count == 0 || (opts.exit_status && out.last_falsy) => 1,
        _ => 0,
    };
    Report { code, stats }
}

/// What a message on stderr is about.
#[derive(Clone, Copy, PartialEq)]
enum Note {
    /// a record that failed
    Record,
    /// something else that fails the run: unreadable input
    Failure,
    /// a warning, or a note on a hold-up; the run goes on
    Aside,
}

/// How often the main thread, while it waits, looks for a note to pass on.
const LOOK: Duration = Duration::from_millis(100);

/// Write each record's outputs in input order, flushing whenever the next
/// output isn't ready yet. `head` is told each record as its turn to print
/// comes. `report` gets messages for stderr, and what each is about;
/// `unfinished` gets each record that didn't finish, as read. Returns how
/// many records a stop left unfinished without an error of their own.
fn write<W: Write>(
    records: &Receiver<Item>,
    batcher: &Batcher,
    head: &dyn Fn(RecId),
    out: &mut Output<W>,
    report: &mut dyn FnMut(String, Note),
    unfinished: &mut dyn FnMut(&str),
) -> io::Result<usize> {
    let (mut warned, mut stopped) = (false, 0);
    let notice = |report: &mut dyn FnMut(String, Note)| {
        if let Some(n) = batcher.notice() {
            report(n, Note::Aside);
        }
    };
    loop {
        let item = match records.try_recv() {
            Ok(item) => item,
            Err(TryRecvError::Disconnected) => return Ok(stopped),
            Err(TryRecvError::Empty) => {
                out.flush()?;
                notice(report);
                match records.recv_timeout(LOOK) {
                    Ok(item) => item,
                    Err(RecvTimeoutError::Disconnected) => return Ok(stopped),
                    // Stopped: the reader may be blocked on input that never
                    // comes, and every record it started has been handed over.
                    Err(RecvTimeoutError::Timeout) if batcher.stopped().is_some() => return Ok(stopped),
                    Err(RecvTimeoutError::Timeout) => continue,
                }
            }
        };
        let (seq, msgs) = match item {
            Item::Record(seq, msgs) => (seq, msgs),
            Item::Unreadable(e) => {
                report(e, Note::Failure);
                continue;
            }
        };
        head(seq);
        loop {
            let msg = match msgs.try_recv() {
                Ok(m) => m,
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {
                    out.flush()?;
                    match msgs.recv_timeout(LOOK) {
                        Ok(m) => m,
                        Err(RecvTimeoutError::Timeout) => {
                            notice(report);
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            };
            match msg {
                Msg::Out(c) => {
                    out.write(&c)?;
                    if c.not_json && !std::mem::replace(&mut warned, true) {
                        out.flush()?;
                        report(
                            format!(
                                "record {seq} printed a value that isn't JSON ({NOT_JSON}); see \"Differences from jq\""
                            ),
                            Note::Aside,
                        );
                    }
                }
                Msg::End(outcome, text) => {
                    match &outcome {
                        Outcome::Done => {}
                        Outcome::Stopped => stopped += 1,
                        Outcome::Failed(e) => report(format!("record {seq}: {e}"), Note::Record),
                        Outcome::Bug(e) => report(format!("internal error in record {seq}: {e}"), Note::Record),
                    }
                    if let (false, Some(text)) = (outcome == Outcome::Done, text) {
                        unfinished(&text);
                    }
                }
            }
        }
        // Every output is written: the record no longer holds memory here.
        batcher.printed();
    }
}

/// The reader thread: hand each record, in order, to the pool once the
/// window has room for it, and the main thread its output channel.
fn read(
    reader: impl Iterator<Item = Result<Raw, String>> + Send + 'static,
    batcher: Batcher,
    pool: Arc<Records>,
) -> Receiver<Item> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut seq = 0;
        for raw in reader {
            let raw = match raw {
                Ok(raw) => raw,
                Err(e) => {
                    let _ = tx.send(Item::Unreadable(format!("reading the input: {e}")));
                    break;
                }
            };
            let Some(start) = batcher.begin() else { break };
            seq += 1;
            let (otx, orx) = sync_channel(AHEAD);
            pool.start(seq, raw, otx, start);
            if tx.send(Item::Record(seq, orx)).is_err() {
                break;
            }
        }
    });
    rx
}

/// What a trace's first line says the run sent to.
fn about(cfg: &Config) -> Vec<(&str, &str)> {
    let api = cfg.api.as_deref().map(|a| ("api", a));
    api.into_iter().chain([("url", cfg.base_url.as_str()), ("model", cfg.model.as_str())]).collect()
}
