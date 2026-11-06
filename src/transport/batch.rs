//! Questions, rounds, and waiting records: the memo, the window, sending,
//! and answers coming back. One record's side is `asker.rs`; a promise and
//! the rounds waiting on it, `promise.rs`.
//!
//! Each record runs on a worker, again from the start each time it has new
//! answers. `ask` gives a run the answer, or queues the question
//! and returns at once. When the run stops, the questions it queued go out
//! together, as the record's next *round*: a request for each value judged.
//! The record waits until the whole round is answered, then runs again.
//! Wave k is every record's k-th round, so a run's waves and requests follow
//! from its program, its input and its answers, never from timing. Answers
//! resolve per request, so a record whose round is back goes on while
//! others wait.

use crate::clock::Clock;
use crate::trace::Trace;
use crate::transport::api::{Answer, ApiError, Failure, Kind, Response, Settings, Spec};
use crate::transport::picture;
use crate::transport::promise::Promise;
use crate::transport::request;
use crate::transport::sched::{Job, Pool};
use serde_json::value::RawValue;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, MutexGuard};

pub type RecId = usize;

/// Requests in a row with no usable answer (failed after their retries,
/// refused, broken, or every answer invalid) before the run aborts: a cause
/// that fails every request the same way would otherwise be sent for every
/// record of the input (TRANSPORT.md, rule 6).
const FUTILE: u32 = 10;

/// Requests at least this large measure bytes per token; in smaller ones the
/// request's fixed cost in tokens would skew it.
const MEASURED: usize = 16 << 10;

/// Records started but not yet printed, as a multiple of the window. A
/// record that has finished waits, with its output in memory, until the
/// records before it print; this bounds how many wait behind a slow one. At
/// deem's rate, 4 windows is about 10 s of work: room for ordinary jitter,
/// while an outage makes deem wait instead of growing.
const UNPRINTED: usize = 4;

/// Answer each demand, let go of it, then tell the rounds whose last answer
/// it was: a record that runs on finds none of its questions still held
/// here, so a sweep can drop them (TRANSPORT.md, rule 3).
fn settle(answered: impl IntoIterator<Item = (Demand, Result<Answer, Failure>)>) {
    let mut wake = Vec::new();
    for (d, a) in answered {
        wake.extend(d.promise.set(a));
    }
    wake.iter().for_each(|w| w.answered());
}

pub struct Limits {
    pub model: String,
    /// records in flight
    pub window: usize,
    /// questions per run; `None` is unlimited
    pub max_questions: Option<u64>,
    /// memo entries past which answered ones no record holds are dropped
    pub memo: usize,
    /// the run's pictures, which requests show by reference
    pub pictures: Arc<picture::Pictures>,
    /// where the settings an error names are
    pub settings: Settings,
}

#[derive(Clone)]
pub struct Batcher(Arc<Shared>);

struct Shared {
    st: std::sync::Mutex<State>,
    cv: Condvar,
    pool: Pool,
    limits: Limits,
    clock: Arc<Clock>,
    trace: Arc<Trace>,
}

struct Demand {
    key: u128,
    state: Box<RawValue>,
    spec: Spec,
    promise: Arc<Promise>,
}

impl Demand {
    /// Whether a record can still read the answer. A promise is held by the
    /// memo, this demand, and the records that asked it, which hold it until
    /// they end; so two holders means no record. Records only let go, and a
    /// memo hit (the only way back) takes the batcher's lock, so a question
    /// found unread stays unread.
    fn unread(&self) -> bool {
        Arc::strong_count(&self.promise) <= 2
    }
}

struct State {
    live: usize,
    /// records started and not yet printed: `live`, plus those that have
    /// finished and wait for the records before them to print
    unprinted: usize,
    /// asked and not yet sent
    pending: Vec<Demand>,
    memo: HashMap<u128, Arc<Promise>>,
    asked: u64,
    /// requests sent and not yet back, and sends being built
    in_flight: usize,
    requests: u64,
    stop: Option<String>,
    /// when a record last ended, freeing a slot in the window
    freed: u64,
    /// requests in a row with no usable answer
    futile: u32,
    /// body bytes and input tokens of the large requests answered so far
    measured: (u64, u64),
    /// the memo size at which it is next swept
    sweep_at: usize,
}

impl Batcher {
    pub fn new(pool: Pool, limits: Limits, clock: Arc<Clock>, trace: Arc<Trace>) -> Self {
        let st = State {
            live: 0,
            unprinted: 0,
            pending: Vec::new(),
            memo: HashMap::new(),
            asked: 0,
            in_flight: 0,
            requests: 0,
            stop: None,
            freed: 0,
            futile: 0,
            measured: (0, 0),
            sweep_at: limits.memo,
        };
        Self(Arc::new(Shared { st: st.into(), cv: Condvar::new(), pool, limits, clock, trace }))
    }

    /// The model questions are asked of, part of each one's memo key.
    pub fn model(&self) -> &str {
        &self.0.limits.model
    }

    pub fn trace(&self) -> &Trace {
        &self.0.trace
    }

    pub fn clock(&self) -> &Clock {
        &self.0.clock
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        crate::lock(&self.0.st)
    }

    /// A record is about to start. Waits for a slot in the window, and for
    /// the records waiting to print to drop below their limit; returns when
    /// the record starts: now, or when a slot freed if it had to wait.
    /// `None`: the run is stopping and the record should not start.
    pub fn begin(&self) -> Option<u64> {
        let mut st = self.lock();
        let mut start = self.0.clock.now();
        while self.full(&st) && st.stop.is_none() {
            st = crate::wait(&self.0.cv, st);
            start = start.max(st.freed);
        }
        if st.stop.is_some() {
            return None;
        }
        st.live += 1;
        st.unprinted += 1;
        Some(start)
    }

    /// A record has ended. A question no record can read any more (its
    /// records have all ended without looking at it) is dropped, and
    /// forgotten, so a later repeat is asked afresh.
    pub fn end(&self) {
        let mut st = self.lock();
        st.live -= 1;
        st.freed = st.freed.max(self.0.clock.now());
        let pending = std::mem::take(&mut st.pending);
        let (unread, pending): (Vec<Demand>, Vec<Demand>) = pending.into_iter().partition(Demand::unread);
        st.pending = pending;
        for d in unread {
            st.memo.remove(&d.key);
        }
        self.0.cv.notify_all();
    }

    /// The main thread is done with a record's output: printed, or the
    /// record never ran. Frees its place among the records waiting to print.
    pub fn printed(&self) {
        let mut st = self.lock();
        st.unprinted -= 1;
        self.0.cv.notify_all();
    }

    /// Whether a record can't start now: the window is full, or too many
    /// records wait to print.
    fn full(&self, st: &State) -> bool {
        let window = self.0.limits.window;
        st.live >= window || st.unprinted >= window * UNPRINTED
    }

    /// Queue a question for record `rec`, `key` its memo key, asked after
    /// the answers in `after` (for `--trace`). Never blocks. `None`: the run
    /// is stopping.
    pub(super) fn ask(
        &self,
        rec: RecId,
        key: u128,
        state: Box<RawValue>,
        spec: Spec,
        after: Vec<u64>,
    ) -> Option<Arc<Promise>> {
        let mut st = self.lock();
        // A repeat is free, so it is answered even past the cap. After a
        // stop that drained the queue, its promise is already `Stopped`.
        if let Some(p) = st.memo.get(&key) {
            self.0.trace.reuse(rec, p.id);
            return Some(p.clone());
        }
        if st.stop.is_some() {
            return None;
        }
        if self.0.limits.max_questions.is_some_and(|cap| st.asked >= cap) {
            let cap = self.0.limits.max_questions.unwrap_or_default();
            let why = format!("stopped at the question limit ({cap}); raise it with --max-questions N, 0 for no limit");
            // The questions within the cap still go, when their records
            // wait for them; no record starts once halted.
            self.halt(&mut st, why);
            return None;
        }
        st.asked += 1;
        let promise = Arc::new(Promise::new(st.asked));
        self.0.trace.ask(rec, promise.id, &spec.q, after);
        st.memo.insert(key, promise.clone());
        self.sweep(&mut st);
        st.pending.push(Demand { key, state, spec, promise: promise.clone() });
        Some(promise)
    }

    /// Past its size, the memo drops the answers no record holds. A pending
    /// question's entry stays: it is how a record that asks it again finds
    /// it, and how `Demand::unread` recognises one nobody can read. What is
    /// held is bounded by the window, so a sweep halves the memo at least
    /// once it is twice that; until then sweeps grow apart (TRANSPORT.md,
    /// rule 3).
    fn sweep(&self, st: &mut State) {
        if st.memo.len() <= st.sweep_at {
            return;
        }
        st.memo.retain(|_, p| !(p.is_done() && Arc::strong_count(p) == 1));
        st.sweep_at = self.0.limits.memo.max(2 * st.memo.len());
    }

    /// Send the pending questions among `ids` (a record's own, and repeats
    /// of others' it holds) as round `round`, in the order they were asked.
    /// Whether any went.
    pub(super) fn send_held(&self, ids: &[u64], round: u64) -> bool {
        let mut st = self.lock();
        let (mine, rest): (Vec<Demand>, Vec<Demand>) =
            std::mem::take(&mut st.pending).into_iter().partition(|d| ids.contains(&d.promise.id));
        st.pending = rest;
        if mine.is_empty() {
            return false;
        }
        // Held until the requests are counted, so `close` can't miss them.
        st.in_flight += 1;
        drop(st);
        self.send(mine, round, self.0.clock.now());
        self.lock().in_flight -= 1;
        self.0.cv.notify_all();
        true
    }

    /// Fail everything pending, wake everyone, and read no more records.
    /// Questions already sent are still answered. The first reason wins.
    /// (The question cap only halts: what is queued within it is sent.)
    pub fn shutdown(&self, why: String) {
        self.stop(&mut self.lock(), why);
    }

    /// A note, once, when failing requests have held the run up.
    pub fn notice(&self) -> Option<String> {
        self.0.pool.notice()
    }

    pub fn stopped(&self) -> Option<String> {
        self.lock().stop.clone()
    }

    /// Wait until every question sent has been answered.
    pub fn close(&self) {
        let mut st = self.lock();
        while st.in_flight > 0 {
            st = crate::wait(&self.0.cv, st);
        }
    }

    /// Refuse new records and questions, and wake everyone. The first
    /// reason wins. What is queued is still sent.
    fn halt(&self, st: &mut State, why: String) {
        if st.stop.is_none() {
            self.0.trace.stop(&why);
        }
        st.stop.get_or_insert(why);
        self.0.cv.notify_all();
    }

    /// Halt, fail what is queued, and send nothing more: for when sending
    /// can't help (a rejected key, an API that's down or refuses everything,
    /// a closed output).
    fn stop(&self, st: &mut State, why: String) {
        self.halt(st, why);
        self.0.pool.cancel();
        settle(st.pending.drain(..).map(|d| (d, Err(Failure::Stopped))).collect::<Vec<_>>());
    }

    /// The bytes per token measured so far, which estimate a request's
    /// tokens for the token rate.
    fn bytes_per_token(&self) -> f64 {
        let (bytes, tokens) = self.lock().measured;
        if tokens == 0 { request::BYTES_PER_TOKEN } else { bytes as f64 / tokens as f64 }
    }

    /// Send `demands` as one record's round `round`, a request for each value,
    /// going out at `at`. The caller holds a count in `in_flight` until this
    /// returns.
    fn send(&self, demands: Vec<Demand>, round: u64, at: u64) {
        let questions = demands.len();
        let states: Vec<_> = demands.iter().map(|d| (&*d.state, &d.spec)).collect();
        let per_token = self.bytes_per_token();
        let reqs = request::requests(&self.0.limits.model, &states);
        let first = {
            let mut st = self.lock();
            let first = st.requests + 1;
            st.requests += reqs.len() as u64;
            st.in_flight += reqs.len();
            first
        };
        let mut demands: Vec<Option<Demand>> = demands.into_iter().map(Some).collect();
        let mut refused = Vec::new();
        let jobs: Vec<Job> = reqs
            .into_iter()
            .zip(first..)
            .filter_map(|(request::Built { mut req, order }, n)| {
                let ds: Vec<Demand> = order.iter().filter_map(|&i| demands[i].take()).collect();
                let size = req.body().len();
                // a request with pictures doesn't measure bytes per token
                let mut bytes = size;
                let mut tokens = request::tokens(&req, per_token);
                match picture::resolve(&req, &self.0.limits.pictures, per_token) {
                    Ok((shown, _)) if shown.is_empty() => {}
                    Ok((shown, with)) => {
                        (req.pictures, tokens, bytes) = (shown, with, 0);
                    }
                    Err(e) => {
                        refused.push((n, ds, e));
                        return None;
                    }
                }
                let ids = ds.iter().map(|d| d.promise.id).collect();
                let b = self.clone();
                Some(Job {
                    req,
                    bytes: size,
                    tokens,
                    ids,
                    n,
                    at,
                    done: Box::new(move |r, attempts, tokens| b.resolve(n, ds, r, (attempts, tokens, bytes))),
                })
            })
            .collect();
        self.0.trace.wave(round, at, questions, jobs.iter().map(|j| j.n).collect());
        for job in jobs {
            self.0.pool.submit(job);
        }
        // too large to send: failed as if the API had refused them
        for (n, ds, e) in refused {
            self.resolve(n, ds, Err(e), (1, 0, 0));
        }
    }

    /// Request `n` is back, after `attempts` that cost `tokens`, with a body
    /// of `bytes`: check its answers, fill its promises, and wake the records
    /// waiting on them.
    ///
    /// A failure every request would share aborts the run at once; any other that keeps coming aborts it
    /// after `FUTILE` requests in a row (TRANSPORT.md, rule 6). No attempts:
    /// the run stopped before the request was sent.
    fn resolve(
        &self,
        n: u64,
        ds: Vec<Demand>,
        r: Result<Response, ApiError>,
        (attempts, tokens, bytes): (u64, u64, usize),
    ) {
        let cancelled = self.0.pool.cancelled();
        if attempts == 0 {
            settle(ds.into_iter().map(|d| (d, Err(Failure::Stopped))));
            self.lock().in_flight -= 1;
            self.0.cv.notify_all();
            return;
        }
        // the reply as received, for `--trace`, even one broken as a whole
        let raw = r.as_ref().ok().and_then(|r| r.raw.clone());
        // A reply without an answer to every question is broken as a whole.
        let r = r.and_then(|resp| match (0..ds.len()).map(request::qname).find(|q| !resp.answers.contains_key(q)) {
            Some(q) => Err(ApiError::Decode(format!("the response had no answer for {q}"))),
            None => Ok(resp),
        });
        let kind = r.as_ref().err().map(ApiError::kind);
        let describe = |e: &ApiError| e.describe(&self.0.limits.settings);
        let result = r.as_ref().map_or_else(describe, |_| "ok".into());
        let checked: Result<Vec<Result<Answer, String>>, &ApiError> = r.as_ref().map(|resp| {
            (ds.iter().enumerate()).map(|(i, d)| resp.answers[&request::qname(i)].clone().check(&d.spec)).collect()
        });
        let useful = checked.as_ref().is_ok_and(|a| a.iter().any(Result::is_ok));
        let setup = kind.is_some_and(Kind::setup);
        let futile = {
            let mut st = self.lock();
            if let Some(t) = r.as_ref().ok().and_then(|r| r.input_tokens).filter(|_| bytes >= MEASURED) {
                st.measured = (st.measured.0 + bytes as u64, st.measured.1 + t);
            }
            st.futile = if useful { 0 } else { st.futile + 1 };
            st.futile >= FUTILE
        };
        // An abort says why once: the answers it fails are `Stopped`.
        let stops = cancelled || setup || futile;
        let fail = |e: String| if stops { Failure::Stopped } else { Failure::Failed(e) };
        let answers: Vec<Result<Answer, Failure>> = match &checked {
            Ok(a) => a.iter().map(|a| a.clone().map_err(fail)).collect(),
            Err(e) => ds.iter().map(|_| Err(fail(describe(e)))).collect(),
        };
        let scores = answers.iter().filter_map(|a| a.as_ref().ok()?.value.as_f64()).collect();
        self.0.trace.answered(n, (&result, attempts, tokens), scores, raw.as_deref());
        let at = self.0.clock.now();
        ds.iter().for_each(|d| d.promise.at.store(at, Ordering::SeqCst));
        settle(ds.into_iter().zip(answers));
        let mut st = self.lock();
        st.in_flight -= 1;
        match &checked {
            Err(e) if setup => self.stop(&mut st, describe(e)),
            _ if futile => {
                let last = match &checked {
                    Err(e) => describe(e),
                    Ok(a) => a.iter().find_map(|a| a.clone().err()).unwrap_or_default(),
                };
                let why = match kind {
                    Some(Kind::Unavailable) => {
                        format!("the API is unavailable: {FUTILE} requests in a row failed after their retries")
                    }
                    Some(Kind::SlowDown) => {
                        format!("the API kept asking deem to slow down: {FUTILE} requests in a row gave up waiting")
                    }
                    _ => format!("no usable answer to {FUTILE} requests in a row, so sending more can't help"),
                };
                self.stop(&mut st, format!("{why} (the last: {last})"))
            }
            _ => self.0.cv.notify_all(),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::transport::api::{Asked, Judge, Request};
    use crate::transport::asker::Asker;
    use crate::transport::sched::{Rate, Retry};
    use std::time::Duration;

    const RATE: Rate = Rate { requests: 80.0, tokens: 100_000.0 };
    use serde_json::json;

    /// Answers every question 0.5.
    pub(crate) struct Half;

    impl Judge for Half {
        fn judge(&self, r: &Request) -> Result<Response, ApiError> {
            let half = || Answer { value: json!(0.5), confidence: None, probabilities: None };
            let answers = r.questions.keys().map(|k| (k.clone(), half())).collect();
            Ok(Response { answers, input_tokens: None, raw: None })
        }
    }

    pub(crate) fn batcher(cap: u64) -> Batcher {
        batcher_with(Some(cap), 8, 100_000, Arc::new(Half))
    }

    pub(crate) fn batcher_with(
        max_questions: Option<u64>,
        window: usize,
        memo: usize,
        judge: Arc<dyn Judge>,
    ) -> Batcher {
        let clock = Arc::new(Clock::new(true));
        let trace = Arc::new(Trace::off(clock.clone()));
        let retry =
            Retry { backoff: Duration::from_millis(1), busy_for: Duration::from_secs(60), notice_after: Duration::MAX };
        let pool = Pool::new(judge, 1, RATE, retry, clock.clone(), trace.clone());
        let limits = Limits {
            model: "m".into(),
            window,
            max_questions,
            memo,
            pictures: Default::default(),
            settings: Settings::Env,
        };
        Batcher::new(pool, limits, clock, trace)
    }

    pub(crate) fn raw(j: &str) -> Box<RawValue> {
        RawValue::from_string(j.to_owned()).unwrap()
    }

    pub(crate) fn q(text: &str) -> Spec {
        Spec { q: text.into(), ..Spec::default() }
    }

    /// Records the requests it is sent, as (state, its questions).
    pub(crate) struct Seen(pub(crate) std::sync::Mutex<Vec<(String, usize)>>);
    impl Judge for Seen {
        fn judge(&self, r: &Request) -> Result<Response, ApiError> {
            self.0.lock().unwrap().push((r.state.get().to_owned(), r.questions.len()));
            Half.judge(r)
        }
    }

    /// What a worker does with a record between its runs: send its round,
    /// wait for the answers, and resume.
    pub(crate) fn wait(r: &Asker) -> Option<usize> {
        let (tx, rx) = std::sync::mpsc::channel();
        let n = r.send(None, Box::new(move || _ = tx.send(())));
        if n > 0 {
            rx.recv_timeout(Duration::from_secs(10)).expect("the round is answered");
        }
        r.resume().then_some(n)
    }

    /// The question a run got no answer to.
    pub(crate) fn later(r: Result<(Asked, bool), Failure>) -> u64 {
        match r {
            Ok((Asked::Later(id), _)) => id,
            r => panic!("not queued: {r:?}"),
        }
    }

    pub(crate) fn half() -> Result<(Asked, bool), Failure> {
        Ok((Asked::Answer(Ok(Answer { value: json!(0.5), confidence: None, probabilities: None })), false))
    }

    // The cap only refuses what is past it: the question within it is still
    // sent, and a repeat is free even after the cap.
    #[test]
    fn the_cap_sends_what_fits() {
        let b = batcher(1);
        assert!(b.begin().is_some() && b.begin().is_some());
        let (one, two) = (Asker::new(&b, 0), Asker::new(&b, 1));
        let p = later(one.ask(raw(r#""a""#), q("q"), 64));
        assert_eq!(two.ask(raw(r#""b""#), q("q"), 64), Err(Failure::Stopped), "past the cap");
        assert!(b.begin().is_none(), "no record starts once halted");
        assert_eq!(later(two.ask(raw(r#""a""#), q("q"), 64)), p, "a repeat is free, even past the cap");
        assert_eq!(wait(&one), Some(1));
        assert_eq!(one.ask(raw(r#""a""#), q("q"), 64), half());
        assert!(b.stopped().is_some_and(|why| why.starts_with("stopped at the question limit (1)")));
        drop((one, two));
        b.end();
        b.end();
        b.close();
    }

    // A question whose records all ended without waiting for it is dropped,
    // and forgotten: a later repeat is asked afresh (were it remembered, the
    // repeat would wait for an answer never sent).
    #[test]
    fn an_unread_question_is_not_sent() {
        let b = batcher(10);
        assert!(b.begin().is_some());
        let one = Asker::new(&b, 0);
        later(one.ask(raw(r#""a""#), q("q"), 64));
        drop(one);
        b.end();
        b.close();
        assert_eq!(b.lock().requests, 0, "nobody could read it");
        assert!(b.begin().is_some());
        let two = Asker::new(&b, 1);
        assert_eq!(later(two.ask(raw(r#""a""#), q("q"), 64)), 2, "a new question, not the dropped one");
        assert_eq!(wait(&two), Some(1));
        assert_eq!(b.lock().requests, 1);
        drop(two);
        b.end();
        b.close();
    }

    // A question one record asked and another repeats goes with whichever
    // waits first; the first, ending without waiting, doesn't drop it.
    #[test]
    fn a_repeat_is_sent_by_whoever_waits() {
        let b = batcher(10);
        assert!(b.begin().is_some() && b.begin().is_some());
        let (one, two) = (Asker::new(&b, 0), Asker::new(&b, 1));
        let p = later(one.ask(raw(r#""a""#), q("q"), 64));
        assert_eq!(later(two.ask(raw(r#""a""#), q("q"), 64)), p);
        drop(one);
        b.end();
        assert_eq!(wait(&two), Some(1), "still asked, as two holds it");
        assert_eq!(two.ask(raw(r#""a""#), q("q"), 64), half());
        assert_eq!(b.lock().requests, 1);
        drop(two);
        b.end();
        b.close();
    }

    // A stop that sending can't help (here a closed output) fails what is
    // queued instead.
    #[test]
    fn a_shutdown_fails_what_is_queued() {
        let b = batcher(10);
        assert!(b.begin().is_some());
        let one = Asker::new(&b, 0);
        later(one.ask(raw(r#""a""#), q("q"), 64));
        b.shutdown("the output closed".into());
        assert_eq!(wait(&one), None);
        drop(one);
        b.end();
        b.close();
        assert_eq!(b.lock().requests, 0);
    }

    // TRANSPORT.md, rule 3: past its size, the memo drops the answered
    // entries no record holds, and keeps the rest: a held answer is still
    // found, and a pending question still recognised. Sweeps grow apart
    // while most entries are held, so the memo stays within twice the larger
    // of its size and what is held.
    #[test]
    fn the_memo_keeps_what_is_held() {
        let b = batcher_with(None, 8, 100, Arc::new(Half));
        let ask = |r: &Asker, i: usize| r.ask(raw(r#""a""#), q(&format!("q{i}")), 100_000);
        assert!(b.begin().is_some());
        let one = Asker::new(&b, 0);
        (0..50).for_each(|i| _ = later(ask(&one, i)));
        assert_eq!(wait(&one), Some(50));
        for round in 1..=10 {
            let two = Asker::new(&b, round);
            (0..60).for_each(|i| _ = later(ask(&two, 1000 * round + i)));
            assert_eq!(wait(&two), Some(60));
            drop(two);
            let memo = b.lock().memo.len();
            assert!(memo <= 2 * (50 + 60) + 1, "round {round}: {memo} entries");
        }
        let requests = b.lock().requests;
        let three = Asker::new(&b, 99);
        for i in 0..50 {
            assert_eq!(ask(&three, i).map(|r| r.0), half().map(|r| r.0), "a held answer is still found");
        }
        let pending = later(ask(&three, 99_999));
        let four = Asker::new(&b, 100);
        assert_eq!(later(ask(&four, 99_999)), pending, "and a pending question");
        assert_eq!(wait(&three), Some(1));
        assert_eq!(b.lock().requests, requests + 1, "only the new question was sent");
        drop((one, three, four));
        b.end();
        b.close();
    }

    /// `begin` on another thread, and a way to see what it returned.
    fn begin_elsewhere(b: &Batcher) -> std::sync::mpsc::Receiver<Option<u64>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let b = b.clone();
        std::thread::spawn(move || tx.send(b.begin()));
        rx
    }

    // Records that finished but haven't printed hold their output, so at
    // most four windows of them may wait; a record's printing frees a place,
    // and a stop frees the reader.
    #[test]
    fn waiting_to_print_is_bounded() {
        let b = batcher_with(None, 2, 100_000, Arc::new(Half));
        for _ in 0..2 * UNPRINTED {
            assert!(b.begin().is_some());
            b.end();
        }
        let (short, long) = (Duration::from_millis(200), Duration::from_secs(5));
        let next = begin_elsewhere(&b);
        assert!(next.recv_timeout(short).is_err(), "the ninth waits: none has printed");
        b.printed();
        assert!(next.recv_timeout(long).is_ok_and(|s| s.is_some()), "one printed: the ninth starts");
        b.end();
        let next = begin_elsewhere(&b);
        assert!(next.recv_timeout(short).is_err());
        b.shutdown("the output closed".into());
        assert_eq!(next.recv_timeout(long), Ok(None), "a stop frees the reader");
    }
}
