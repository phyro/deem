//! One record's side of the batcher. A record is in a run, asking, or
//! waiting for the round its run sent: `ask` and `send` belong to a run,
//! `resume` to a round that is out, and each moves the record on. The
//! record's answers are the engine's (its journal); a run asks here only
//! what the record hasn't had answered.

use crate::transport::api::{Asked, Failure, Spec};
use crate::transport::batch::{Batcher, RecId};
use crate::transport::promise::{Promise, Round};
use crate::transport::request;
use serde_json::value::RawValue;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// One record's side of the batcher.
pub struct Asker {
    rec: RecId,
    batcher: Batcher,
    state: RefCell<State>,
    notes: RefCell<Notes>,
    /// how many rounds the record has sent
    pub(super) rounds: Cell<u64>,
}

/// Where the record is: in a run, or waiting for the round its last run
/// sent.
enum State {
    Running(Run),
    Sent(Sent),
}

/// What the record's current run has asked.
#[derive(Default)]
struct Run {
    /// the last round's answers, held until this run has had them
    answered: HashMap<u128, Arc<Promise>>,
    /// the questions this run asked that had no answer when it asked them:
    /// unknown to it however soon they arrive, so what a run asks depends
    /// on its answers, not on their timing
    round: HashMap<u128, Arc<Promise>>,
    /// every question this run asked, to tell a repeat
    asked: HashSet<u128>,
}

/// A record's round, sent: its questions, the one the run stopped on, when,
/// and whether it had to wait at all.
struct Sent {
    round: HashMap<u128, Arc<Promise>>,
    on: u64,
    since: u64,
    blocked: bool,
}

/// For `--trace` only: what the record waited for since it last asked a
/// new question, whether it has asked one since it last waited, and the
/// questions it has asked twice in a run (each traced once, not in every
/// run).
#[derive(Default)]
struct Notes {
    waited: Vec<u64>,
    asked_since: bool,
    repeated: HashSet<u128>,
}

impl Asker {
    pub fn new(batcher: &Batcher, rec: RecId) -> Self {
        Self {
            rec,
            batcher: batcher.clone(),
            state: RefCell::new(State::Running(Run::default())),
            notes: RefCell::default(),
            rounds: Cell::new(0),
        }
    }

    /// Ask a question in the current run. Unanswered, it joins the record's
    /// next round, unless that already has `max` questions. Also whether the
    /// record hadn't asked it before.
    pub fn ask(&self, state: Box<RawValue>, spec: Spec, max: usize) -> Result<(Asked, bool), Failure> {
        let mut st = self.state.borrow_mut();
        let State::Running(run) = &mut *st else { panic!("a question asked while the record's round is out") };
        let key = request::key(self.batcher.model(), &state, &spec);
        let again = !run.asked.insert(key);
        if let Some(p) = run.round.get(&key) {
            self.repeat(again, key, p);
            return Ok((Asked::Later(p.id), false));
        }
        if let Some(p) = run.answered.get(&key) {
            self.repeat(again, key, p);
            return Ok((asked(p)?, false));
        }
        if run.round.len() >= max {
            return Ok((Asked::Full, false));
        }
        let after = {
            let mut n = self.notes.borrow_mut();
            n.asked_since = true;
            n.waited.clone()
        };
        let p = self.batcher.ask(self.rec, key, state, spec, after).ok_or(Failure::Stopped)?;
        let a = asked(&p)?;
        if let Asked::Later(_) = a {
            run.round.insert(key, p);
        }
        Ok((a, true))
    }

    /// A question asked again in one run is traced once.
    fn repeat(&self, again: bool, key: u128, p: &Promise) {
        if again && self.notes.borrow_mut().repeated.insert(key) {
            self.batcher.trace().reuse(self.rec, p.id);
        }
    }

    /// The current run has stopped: send the questions it asked that
    /// haven't gone (the record's own, and repeats of others' it holds) as
    /// the record's next round. `then` is called once each of them is
    /// answered, on whatever thread brings the last; then the record
    /// [`resume`](Self::resume)s. `looked`: the question the run stopped on,
    /// if any. How many questions it waits for; with none, nothing is sent,
    /// and `then` is never called.
    pub fn send(&self, looked: Option<u64>, then: Box<dyn FnOnce() + Send>) -> usize {
        let mut st = self.state.borrow_mut();
        let State::Running(run) = &mut *st else { panic!("a round sent while one is out") };
        let round = std::mem::take(&mut run.round);
        let Some(first) = round.values().map(|p| p.id).min() else { return 0 };
        let b = &self.batcher;
        let on = looked.unwrap_or(first);
        {
            let mut n = self.notes.borrow_mut();
            if std::mem::take(&mut n.asked_since) {
                n.waited.clear();
            }
            if !n.waited.contains(&on) {
                n.waited.push(on);
            }
        }
        let since = b.clock().now();
        let blocked = round.values().any(|p| !p.is_done());
        if blocked {
            b.trace().wait(self.rec, on);
            let ids: Vec<u64> = round.values().map(|p| p.id).collect();
            let r = self.rounds.get() + 1;
            if b.send_held(&ids, r) {
                self.rounds.set(r);
            }
        }
        let n = round.len();
        // the run, and the last round's answers it held, are done with
        *st = State::Sent(Sent { round: round.clone(), on, since, blocked });
        drop(st);
        Round::wait(round.into_values(), then);
        n
    }

    /// The round sent is answered: the record's next run begins, with its
    /// answers, and its time moves on to the last of them. `false`: the run
    /// is stopping.
    pub fn resume(&self) -> bool {
        let mut st = self.state.borrow_mut();
        let State::Sent(Sent { round, on, since, blocked }) =
            std::mem::replace(&mut *st, State::Running(Run::default()))
        else {
            panic!("a record resumed with no round out")
        };
        let b = &self.batcher;
        b.clock().reach(round.values().map(|p| p.at.load(Ordering::SeqCst)).max().unwrap_or_default());
        b.trace().resume(self.rec, on, since, blocked);
        let stopped = round.values().any(|p| matches!(p.answer.get(), Some(Err(Failure::Stopped))));
        *st = State::Running(Run { answered: round, ..Run::default() });
        !stopped
    }
}

/// What a promise gives a run: an answer, a failure, or nothing yet.
fn asked(p: &Promise) -> Result<Asked, Failure> {
    match p.answer.get() {
        None => Ok(Asked::Later(p.id)),
        Some(Ok(a)) => Ok(Asked::Answer(Ok(a.clone()))),
        Some(Err(Failure::Failed(why))) => Ok(Asked::Answer(Err(why.clone()))),
        Some(Err(Failure::Stopped)) => Err(Failure::Stopped),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::batch::tests::{Half, Seen, batcher_with, half, later, q, raw, wait};

    // A record's questions go when it waits, and not before: all its run
    // queued, a request for each value, as its next round. Another record's
    // go with its own wait, whatever this one does meanwhile.
    #[test]
    fn a_round_goes_when_its_record_waits() {
        let seen = Arc::new(Seen(Default::default()));
        let b = batcher_with(None, 8, 100, seen.clone());
        let (one, two) =
            (b.begin().and(Some(Asker::new(&b, 1))).unwrap(), b.begin().and(Some(Asker::new(&b, 2))).unwrap());
        later(one.ask(raw(r#""a""#), q("q1"), 64));
        later(one.ask(raw(r#""a""#), q("q2"), 64));
        later(one.ask(raw(r#""b""#), q("q1"), 64));
        later(two.ask(raw(r#""c""#), q("q1"), 64));
        assert!(seen.0.lock().unwrap().is_empty(), "nothing goes before a record waits");
        assert_eq!(wait(&one), Some(3));
        b.close();
        let mut sent = seen.0.lock().unwrap().clone();
        sent.sort();
        assert_eq!(sent, [(r#""a""#.into(), 2), (r#""b""#.into(), 1)], "one's round: a request a value");
        assert_eq!(one.ask(raw(r#""a""#), q("q2"), 64), half(), "its next run has the answer");
        assert_eq!((one.rounds.get(), two.rounds.get()), (1, 0), "two hasn't waited");
        drop((one, two));
        b.end();
        b.end();
    }

    // A run's round holds at most `max` questions; a question it asked
    // again is no new one. And a question unanswered when the run asked it
    // stays unanswered to that run, however soon its answer comes.
    #[test]
    fn a_run_is_full_at_max() {
        let b = batcher_with(None, 8, 100, Arc::new(Half));
        assert!(b.begin().is_some() && b.begin().is_some());
        let (one, two) = (Asker::new(&b, 1), Asker::new(&b, 2));
        let a = later(one.ask(raw(r#""a""#), q("q"), 2));
        later(one.ask(raw(r#""b""#), q("q"), 2));
        assert_eq!(one.ask(raw(r#""a""#), q("q"), 2).map(|r| r.0), Ok(Asked::Later(a)), "asked again: no new one");
        assert_eq!(one.ask(raw(r#""c""#), q("q"), 2), Ok((Asked::Full, false)));
        assert_eq!(later(two.ask(raw(r#""a""#), q("q"), 2)), a, "two shares one's question");
        assert_eq!(wait(&two), Some(1));
        assert_eq!(one.ask(raw(r#""a""#), q("q"), 2).map(|r| r.0), Ok(Asked::Later(a)), "still unknown to this run");
        assert_eq!(wait(&one), Some(2));
        assert_eq!(later(one.ask(raw(r#""c""#), q("q"), 2)), 3, "the next run asks it");
        drop((one, two));
        b.end();
        b.end();
        b.close();
    }

    // A record is in a run or waiting for its round, never both: a
    // question asked, or a round sent, while a round is out, and a resume
    // with none out, are bugs, said as such (the pool reports them).
    #[test]
    fn a_record_is_in_a_run_or_waiting() {
        let b = batcher_with(None, 8, 100, Arc::new(Half));
        assert!(b.begin().is_some());
        let one = Asker::new(&b, 1);
        let quiet = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| one.resume()));
        assert!(quiet.is_err(), "no round is out");
        later(one.ask(raw(r#""a""#), q("q"), 64));
        let (tx, rx) = std::sync::mpsc::channel();
        assert_eq!(one.send(None, Box::new(move || _ = tx.send(()))), 1);
        let asked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| one.ask(raw(r#""b""#), q("q"), 64)));
        assert!(asked.is_err(), "a question while the round is out");
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(one.resume());
        assert_eq!(one.ask(raw(r#""a""#), q("q"), 64), half(), "the next run has the answer");
        drop(one);
        b.end();
        b.close();
    }
}
