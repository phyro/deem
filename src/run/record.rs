//! Records on a pool of workers. A record's run never waits: it ends, and
//! the record waits as data (its input, its journal, its answers) until its
//! round is answered, then runs again on the same worker. So a few workers
//! run every record in flight, each record pinned to one, since a jq value
//! can't change threads.
//!
//! A worker never blocks on a record's output except the record being
//! printed. Any other record whose output is full pauses: its run stops,
//! and it runs again once its turn to print comes.

use super::Msg;
use super::input::{Raw, Unvalued};
use super::output::{Chunk, NOT_JSON, is_json};
use crate::engine::{Ask, Emitted, J, Journal, NOTHING, Outcome, Program, Step};
use crate::transport::api::{Asked, Failure, Spec};
use crate::transport::asker::Asker;
use crate::transport::batch::{Batcher, RecId};
use crate::transport::picture::{self, Picture, Pictures};
use serde_json::value::RawValue;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel};

/// Workers at most: a record's jq work is small next to waiting for its
/// answers.
const WORKERS: usize = 8;
const STACK: usize = 4 << 20;

/// What every record shares.
pub(super) struct Shared {
    pub program: Arc<Program>,
    /// failed answers that records ended without failing on
    pub caught: Arc<AtomicU64>,
    pub batcher: Batcher,
    pub pictures: Arc<Pictures>,
    pub raw: bool,
    pub max_bytes: usize,
    /// keep each record as read, for `--failed`
    pub keep: bool,
    pub max_unanswered: usize,
}

/// The pool.
pub(super) struct Records {
    workers: Vec<Sender<Event>>,
    /// the record being printed
    printing: Arc<AtomicUsize>,
}

enum Event {
    /// a record, its output, and when it starts on the run's clock
    Start(RecId, Raw, SyncSender<Msg>, u64),
    /// the record's round is answered
    Answered(RecId),
    /// the record's turn to print has come
    Head(RecId),
    /// no record prints any more
    Close,
}

impl Records {
    pub fn new(shared: Shared) -> Result<Arc<Self>, String> {
        let n = std::thread::available_parallelism().map_or(1, |n| n.get()).min(WORKERS);
        let (shared, printing) = (Arc::new(shared), Arc::new(AtomicUsize::new(0)));
        let mut pool = Self { workers: Vec::new(), printing };
        for _ in 0..n {
            let (tx, rx) = channel();
            let w = Worker { shared: shared.clone(), printing: pool.printing.clone(), tx: tx.clone() };
            if let Err(e) = std::thread::Builder::new().stack_size(STACK).spawn(move || w.work(rx)) {
                // the ones started have no records: closed, they end
                pool.close();
                return Err(format!("cannot start a worker thread: {e}"));
            }
            pool.workers.push(tx);
        }
        Ok(Arc::new(pool))
    }

    fn send(&self, seq: RecId, e: Event) {
        let _ = self.workers[seq % self.workers.len()].send(e);
    }

    pub fn start(&self, seq: RecId, raw: Raw, out: SyncSender<Msg>, start: u64) {
        self.send(seq, Event::Start(seq, raw, out, start));
    }

    /// Record `seq` is the one being printed.
    pub fn head(&self, seq: RecId) {
        self.printing.store(seq, Ordering::SeqCst);
        self.send(seq, Event::Head(seq));
    }

    /// No record prints any more: a worker ends what waits to print, and
    /// stops once it has no records.
    pub fn close(&self) {
        self.workers.iter().for_each(|w| _ = w.send(Event::Close));
    }
}

struct Worker {
    shared: Arc<Shared>,
    printing: Arc<AtomicUsize>,
    /// its own events, for the rounds of its records
    tx: Sender<Event>,
}

/// A record between events: its output, and where it is.
struct Rec {
    out: SyncSender<Msg>,
    /// what didn't fit in `out`, in order, to send when its turn comes
    held: Vec<Msg>,
    /// the record as read, for `--failed`
    text: Option<String>,
    /// its time on the run's clock
    time: u64,
    state: State,
}

/// Where a record is. Its data is in the states that have it, and every
/// event takes the state and leaves the next one.
enum State {
    /// read, and not run yet
    Ready(Live),
    /// one of its runs is in progress
    Running,
    /// its round is out: it runs again when the round is answered
    Waiting(Live),
    /// its output is full: it runs again when its turn to print comes
    Paused(Live),
    /// it has ended; what it holds goes when its turn comes
    Ended,
}

struct Live {
    input: J,
    journal: Journal,
    asker: Asker,
    /// the record's pictures, held while it runs
    _pictures: Vec<Arc<Picture>>,
}

impl Rec {
    fn take(&mut self) -> State {
        std::mem::replace(&mut self.state, State::Running)
    }
}

impl Worker {
    fn work(self, rx: Receiver<Event>) {
        let mut recs: HashMap<RecId, Rec> = HashMap::new();
        let mut closed = false;
        while let Ok(e) = rx.recv() {
            let (seq, then): (RecId, fn(&Self, RecId, &mut Rec)) = match e {
                Event::Start(seq, raw, out, start) => {
                    recs.insert(seq, self.start(seq, raw, out, start));
                    (seq, Self::go)
                }
                Event::Answered(seq) => (seq, Self::answered),
                Event::Head(seq) => (seq, Self::head),
                Event::Close => {
                    closed = true;
                    recs.retain(|&seq, rec| !self.settle(seq, rec, closed));
                    if recs.is_empty() {
                        return;
                    }
                    continue;
                }
            };
            let Some(rec) = recs.get_mut(&seq) else { continue };
            if let Err(p) = catch_unwind(AssertUnwindSafe(|| then(&self, seq, rec))) {
                let s = p.downcast_ref::<&str>().map(|s| s.to_string());
                let why = s.or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                self.finish(seq, rec, Outcome::Bug(why));
            }
            if self.settle(seq, rec, closed) {
                recs.remove(&seq);
            }
            if closed && recs.is_empty() {
                return;
            }
        }
    }

    /// Whether the worker is done with the record: it has ended, and sent
    /// all it held. Once no record prints (`closed`), a record waiting to
    /// print never will: one paused ends, and what any holds is dropped.
    fn settle(&self, seq: RecId, rec: &mut Rec, closed: bool) -> bool {
        if closed {
            if let State::Paused(_) = rec.state {
                self.finish(seq, rec, Outcome::Stopped);
            }
            rec.held.clear();
        }
        matches!(rec.state, State::Ended) && rec.held.is_empty()
    }

    /// Record `seq`, read, starts at `start` on the run's clock.
    fn start(&self, seq: RecId, raw: Raw, out: SyncSender<Msg>, start: u64) -> Rec {
        let s = &self.shared;
        s.batcher.clock().start_at(start);
        s.batcher.trace().record(seq);
        let text = s.keep.then(|| raw.text());
        let mut rec = Rec { out, held: Vec::new(), text, time: start, state: State::Running };
        let mut pictures = Vec::new();
        match raw.value(&s.pictures, &mut pictures) {
            Err(Unvalued::Bug(e)) => self.end(seq, &mut rec, None, Outcome::Bug(e)),
            Err(Unvalued::Picture(e)) => self.end(seq, &mut rec, None, Outcome::Failed(e)),
            Ok(input) => {
                let asker = Asker::new(&s.batcher, seq);
                rec.state = State::Ready(Live { input, journal: Journal::default(), asker, _pictures: pictures });
            }
        }
        rec
    }

    /// A record read runs for the first time.
    fn go(&self, seq: RecId, rec: &mut Rec) {
        match rec.take() {
            State::Ready(live) => self.run(seq, rec, live),
            other => rec.state = other,
        }
    }

    /// A waiting record's round is answered: it runs again.
    fn answered(&self, seq: RecId, rec: &mut Rec) {
        match rec.take() {
            State::Waiting(live) => {
                self.shared.batcher.clock().start_at(rec.time);
                let going = live.asker.resume();
                rec.time = self.shared.batcher.clock().now();
                match going {
                    true => self.run(seq, rec, live),
                    false => self.end(seq, rec, Some(live), Outcome::Stopped),
                }
            }
            other => rec.state = other,
        }
    }

    /// The record's turn to print: what it holds goes, and a paused record
    /// runs again.
    fn head(&self, seq: RecId, rec: &mut Rec) {
        for m in std::mem::take(&mut rec.held) {
            if rec.out.send(m).is_err() {
                return self.finish(seq, rec, Outcome::Stopped);
            }
        }
        match rec.take() {
            State::Paused(live) => self.run(seq, rec, live),
            other => rec.state = other,
        }
    }

    /// One run of the record; it then ends, waits for its round, or pauses.
    fn run(&self, seq: RecId, rec: &mut Rec, mut live: Live) {
        let s = &self.shared;
        s.batcher.clock().start_at(rec.time);
        let ask = RecordAsk { rec: s, id: seq, asker: &live.asker };
        let (out, held, printing) = (&rec.out, &mut rec.held, &self.printing);
        let step = s.program.step(&live.input, &mut live.journal, &ask, &mut |vals| {
            let mut c = Chunk::default();
            vals.iter().for_each(|v| c.push(v, s.raw));
            // a picture is printed as its path, never its reference
            s.pictures.show(&mut c.bytes);
            deliver(seq, out, held, printing, Msg::Out(c))
        });
        rec.time = s.batcher.clock().now();
        match step {
            Step::Done(outcome) => self.end(seq, rec, Some(live), outcome),
            Step::Paused => rec.state = State::Paused(live),
            Step::Rerun(looked) => {
                let tx = self.tx.clone();
                let then = Box::new(move || _ = tx.send(Event::Answered(seq)));
                match live.asker.send(looked, then) {
                    0 => self.end(seq, rec, Some(live), Outcome::Bug(NOTHING.into())),
                    _ => rec.state = State::Waiting(live),
                }
            }
        }
    }

    /// End the record from wherever it is, unless it has ended.
    fn finish(&self, seq: RecId, rec: &mut Rec, outcome: Outcome) {
        match std::mem::replace(&mut rec.state, State::Ended) {
            State::Ended => {}
            State::Running => self.end(seq, rec, None, outcome),
            State::Ready(live) | State::Waiting(live) | State::Paused(live) => self.end(seq, rec, Some(live), outcome),
        }
    }

    /// The record has ended: say how, and free its place in the window. Only
    /// a record leaving a state that isn't `Ended` gets here, so once.
    fn end(&self, seq: RecId, rec: &mut Rec, live: Option<Live>, outcome: Outcome) {
        let s = &self.shared;
        // the program caught the failed answers its last run was given
        if let Some(live) = &live
            && outcome == Outcome::Done
        {
            s.caught.fetch_add(live.journal.failed() as u64, Ordering::SeqCst);
        }
        // the record lets go of its questions before the batcher hears it
        // has ended
        drop(live);
        rec.state = State::Ended;
        s.batcher.trace().end(seq, ended(&outcome));
        deliver(seq, &rec.out, &mut rec.held, &self.printing, Msg::End(outcome, rec.text.take()));
        s.batcher.end();
    }
}

fn ended(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Done => "done",
        Outcome::Stopped => "stopped",
        Outcome::Failed(e) | Outcome::Bug(e) => e,
    }
}

/// Send `m` on record `seq`'s output. Only the record being printed may
/// wait for room; any other holds what doesn't fit and pauses.
fn deliver(seq: RecId, out: &SyncSender<Msg>, held: &mut Vec<Msg>, printing: &AtomicUsize, m: Msg) -> Emitted {
    if !held.is_empty() {
        held.push(m);
        return Emitted::Pause;
    }
    match out.try_send(m) {
        Ok(()) => Emitted::More,
        Err(TrySendError::Disconnected(_)) => Emitted::Closed,
        Err(TrySendError::Full(m)) if printing.load(Ordering::SeqCst) == seq => match out.send(m) {
            Ok(()) => Emitted::More,
            Err(_) => Emitted::Closed,
        },
        Err(TrySendError::Full(m)) => {
            held.push(m);
            Emitted::Pause
        }
    }
}

/// `judge` for one record: the byte limit, then the batcher, with at most
/// `max_unanswered` questions outstanding.
struct RecordAsk<'a> {
    rec: &'a Shared,
    id: RecId,
    asker: &'a Asker,
}

impl Ask for RecordAsk<'_> {
    fn ask(&self, state: &J, spec: Spec) -> Result<Asked, Failure> {
        let (state, cut) = self.fit(state)?;
        let state = self.pictures(state, &spec)?;
        let (asked, new) = self.asker.ask(state, spec, self.rec.max_unanswered)?;
        // counted once, however many runs ask it
        if cut && new {
            self.rec.batcher.trace().cut(self.id);
        }
        Ok(asked)
    }
}

impl RecordAsk<'_> {
    /// Where pictures may be: each a whole string, in the value or what
    /// describes an answer (an option, a level, an example), never in the
    /// question's text. A value that is one picture
    /// alone goes as `{"image": …}`, a state the API's docs show.
    fn pictures(&self, state: Box<RawValue>, spec: &Spec) -> Result<Box<RawValue>, Failure> {
        let prefix = self.rec.pictures.prefix();
        let fail = |e: &str| Err(Failure::Failed(format!("judge: {e}")));
        let whole = |json: &str| -> Result<usize, Failure> {
            let found = picture::scan(json, &prefix);
            if let Some(Err(text)) = found.iter().find(|f| f.is_err()) {
                let text = serde_json::to_string(text).unwrap_or_default();
                return Err(Failure::Failed(format!(
                    "judge: a picture must be a whole string: put it in the value and name it in backticks (in {})",
                    self.rec.pictures.shown(&text)
                )));
            }
            Ok(found.len())
        };
        if [&spec.q, &spec.yes, &spec.no].iter().any(|t| t.contains(&prefix)) {
            return fail("a picture can't be in the question's text: put it in the value and name it in backticks");
        }
        let described =
            serde_json::to_string(&(&spec.about, &spec.levels, &spec.like, &spec.unlike)).unwrap_or_default();
        let shown = whole(state.get())? + whole(&described)?;
        if spec.boxed && shown == 0 {
            return fail("a box is found in a picture, and the value has none");
        }
        match picture::scan(state.get(), &prefix).first() {
            Some(Ok(f)) if f.start == 0 && f.end == state.get().len() => {
                Ok(RawValue::from_string(format!("{{\"image\":{}}}", state.get())).expect("an object is JSON"))
            }
            _ => Ok(state),
        }
    }

    /// What the model sees: exactly what `tojson` prints for `state`, or an
    /// error; and whether it was cut. `DEEM_MAX_RECORD_BYTES` applies to what
    /// is judged, not to what is read. A long string is cut; anything else
    /// has no obvious place to cut, so it is an error that says what to do.
    fn fit(&self, state: &J) -> Result<(Box<RawValue>, bool), Failure> {
        let fail = |e: String| Failure::Failed(format!("judge: the value to judge {e}"));
        if !is_json(state) {
            return Err(fail(format!("isn't JSON ({NOT_JSON}); see \"Differences from jq\"")));
        }
        let max = self.rec.max_bytes;
        let cut = match state {
            J::TStr(s) if s.len() > max => {
                // at a char boundary: not before a UTF-8 continuation byte
                let n = (0..=max).rev().find(|&n| s.get(n).is_none_or(|b| b & 0xC0 != 0x80)).unwrap_or(0);
                Some(J::utf8_str(s.slice(..n)))
            }
            _ => None,
        };
        let mut bytes = Vec::new();
        let _ = jaq_json::write::write(&mut bytes, &Default::default(), 0, cut.as_ref().unwrap_or(state));
        let text = String::from_utf8(bytes).map_err(|_| fail("has a string that isn't valid UTF-8".into()))?;
        let raw = RawValue::from_string(text).map_err(|e| fail(format!("isn't JSON: {e}")))?;
        match raw.get().len() {
            n if n > max && !matches!(state, J::TStr(_)) => Err(fail(format!(
                "is {n} bytes, over DEEM_MAX_RECORD_BYTES ({max}). \
                 Judge a smaller part, e.g. `.body | .[0:4000] | ask(…; .)`, or raise the limit"
            ))),
            _ => Ok((raw, cut.is_some())),
        }
    }
}
