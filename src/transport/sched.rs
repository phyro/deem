//! The sender pool: worker threads that send requests under the API's two
//! ceilings (requests a second, tokens a second), retry what can be retried,
//! and slow down when the API asks. Once the pool is cancelled, nothing more
//! is sent: not a queued request, and not a retry (TRANSPORT.md, rules 1, 5
//! and 8).

use crate::clock::{self, Clock};
use crate::trace::Trace;
use crate::transport::api::{ApiError, Judge, Kind, Request, Response};
use crate::transport::picture;
use std::hash::{BuildHasher, RandomState};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Senders at most, whatever the rate: each is a thread and a connection.
pub const MAX_WORKERS: usize = 256;
const _: () = assert!(crate::transport::api::CONNECTIONS >= MAX_WORKERS, "a pooled connection per sender");

/// The ceilings: requests and input tokens a second, at most. The pool runs
/// slower while the API asks it to.
#[derive(Clone, Copy)]
pub struct Rate {
    pub requests: f64,
    pub tokens: f64,
}
/// The longest backoff, and the longest `Retry-After` deem waits.
const MAX_BACKOFF: Duration = Duration::from_secs(5);
const MAX_WAIT: Duration = Duration::from_secs(60);

/// How the pool retries.
#[derive(Clone, Copy)]
pub struct Retry {
    /// the first retry's delay; each retry doubles it, up to 5 s
    pub backoff: Duration,
    /// how long "slow down" answers are retried while no request succeeds
    pub busy_for: Duration,
    /// how long failing attempts may hold the run up before it says why
    pub notice_after: Duration,
}

pub struct Job {
    pub req: Request,
    /// its body's length, for `--trace`
    pub bytes: usize,
    pub tokens: u64,
    /// the questions it carries, and its number in the run, for `--trace`
    pub ids: Vec<u64>,
    pub n: u64,
    /// when its wave went, for `--trace`
    pub at: u64,
    pub done: Done,
}

/// Called once, on a worker thread, with a request's final result, the
/// attempts it took, and the tokens they cost. No attempts: the pool was
/// cancelled before the request was sent.
pub type Done = Box<dyn FnOnce(Result<Response, ApiError>, u64, u64) + Send>;

pub struct Pool {
    tx: Mutex<Sender<Job>>,
    shared: Arc<Shared>,
}

/// What the workers share.
struct Shared {
    cancelled: AtomicBool,
    rps: Bucket,
    tps: Bucket,
    health: Mutex<Health>,
    /// the one note about a hold-up, until the run takes it
    notice: Mutex<Option<String>>,
    noticed: AtomicBool,
    retry: Retry,
}

struct Health {
    /// when a request last succeeded (or the pool started)
    ok: Instant,
    /// since when attempts have failed without a success in between
    failing: Option<Instant>,
}

struct Worker {
    judge: Arc<dyn Judge>,
    rx: Arc<Mutex<Receiver<Job>>>,
    shared: Arc<Shared>,
    /// this worker's number, for `--trace`
    n: usize,
    clock: Arc<Clock>,
    trace: Arc<Trace>,
}

impl Pool {
    pub fn new(
        judge: Arc<dyn Judge>,
        workers: usize,
        rate: Rate,
        retry: Retry,
        clock: Arc<Clock>,
        trace: Arc<Trace>,
    ) -> Self {
        let (tx, rx) = channel();
        let rx = Arc::new(Mutex::new(rx));
        let shared = Arc::new(Shared {
            cancelled: AtomicBool::new(false),
            rps: Bucket::new(rate.requests),
            tps: Bucket::new(rate.tokens),
            health: Mutex::new(Health { ok: Instant::now(), failing: None }),
            notice: Mutex::new(None),
            noticed: AtomicBool::new(false),
            retry,
        });
        for n in 0..workers {
            let w = Worker {
                judge: judge.clone(),
                rx: rx.clone(),
                shared: shared.clone(),
                n,
                clock: clock.clone(),
                trace: trace.clone(),
            };
            std::thread::spawn(move || w.run());
        }
        Self { tx: Mutex::new(tx), shared }
    }

    /// Send nothing more: queued requests end unsent, and requests being
    /// retried end with their last error. A request already on the wire
    /// still gets its answer.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::SeqCst)
    }

    /// The note, once, when failing attempts have held the run up.
    pub fn notice(&self) -> Option<String> {
        crate::lock(&self.shared.notice).take()
    }

    pub fn submit(&self, job: Job) {
        // The workers outlive the pool's users, so the send can't fail.
        let _ = crate::lock(&self.tx).send(job);
    }
}

impl Worker {
    fn run(self) {
        loop {
            let job = crate::lock(&self.rx).recv();
            let Ok(job) = job else { return };
            self.clock.start_at(job.at);
            let (r, attempts, tokens) = self.send(&job);
            (job.done)(r, attempts, tokens);
        }
    }

    /// The result, the attempts it took, and the tokens they cost: the API's
    /// count where it gives one, else the estimate. Each attempt first waits
    /// its turn under the ceilings, and checks for a cancel before and after.
    fn send(&self, job: &Job) -> (Result<Response, ApiError>, u64, u64) {
        let (mut attempts, mut spent, mut last) = (0, 0, None);
        // as sent: pictures read and given their IDs, once, after the first turn
        let mut sent: Option<Request> = None;
        loop {
            let ready = self.turn(job.tokens);
            if !ready {
                let e = last.unwrap_or_else(|| ApiError::Transport("not sent: the run stopped".into()));
                return (Err(e), attempts, spent);
            }
            let prepared = match attempts {
                0 if !job.req.pictures.is_empty() => picture::prepare(&job.req).map(|r| sent = Some(r)),
                _ => Ok(()),
            };
            if attempts == 0 {
                // as sent, pictures by their IDs; none if its pictures
                // can't be read, as then it isn't sent
                let req = sent.as_ref().unwrap_or(&job.req);
                let pictures = || req.assets.iter().map(|(id, a)| (id.clone(), a.path.clone())).collect();
                let body = || prepared.is_ok().then(|| (req.body(), pictures()));
                self.trace.request(job.n, self.n, &job.ids, job.bytes, body);
            }
            attempts += 1;
            if let Err(e) = prepared {
                return (Err(e), attempts, spent);
            }
            let req = sent.as_ref().unwrap_or(&job.req);
            // A panicking judge must not strand the records waiting on it.
            let r = catch_unwind(AssertUnwindSafe(|| self.judge.judge(req)))
                .unwrap_or_else(|_| Err(ApiError::Decode("the client panicked".into())));
            spent += r.as_ref().ok().and_then(|r| r.input_tokens).unwrap_or(job.tokens);
            let e = match r {
                Ok(resp) => {
                    self.succeeded();
                    return (Ok(resp), attempts, spent);
                }
                Err(e) => e,
            };
            let kind = e.kind();
            let held_up = self.failed();
            let busy = kind == Kind::SlowDown;
            if busy {
                self.shared.rps.slow_down();
                if let Some(d) = e.retry_after() {
                    self.shared.rps.hold(d.min(MAX_WAIT));
                }
            }
            let hopeless = busy && crate::lock(&self.shared.health).ok.elapsed() >= self.shared.retry.busy_for;
            if attempts >= u64::from(kind.attempts()) || hopeless {
                return (Err(e), attempts, spent);
            }
            if held_up >= self.shared.retry.notice_after && !self.shared.noticed.swap(true, Ordering::SeqCst) {
                let note = match busy {
                    true => format!("the API asks deem to slow down ({e}); retrying more slowly"),
                    false => format!("the API is failing ({e}); retrying"),
                };
                *crate::lock(&self.shared.notice) = Some(note);
            }
            // a wait the API asked for is kept by the ceilings' hold instead
            if e.retry_after().is_none() {
                let d = self.backoff(attempts);
                self.pause(d);
                clock::elapse(d.as_micros() as u64);
            }
            last = Some(e);
        }
    }

    /// The wait before retry `attempt`: the first delay, doubled each time,
    /// at most 5 s, and up to a quarter less at random, as the official SDKs
    /// do, so senders that failed together don't retry together.
    fn backoff(&self, attempt: u64) -> Duration {
        let d = (self.shared.retry.backoff * 2u32.saturating_pow(attempt.saturating_sub(1) as u32)).min(MAX_BACKOFF);
        // on virtual time a run is the same every time, so no chance
        if self.clock.is_virtual() {
            return d;
        }
        let random = RandomState::new().hash_one(attempt) as f64 / u64::MAX as f64;
        d.mul_f64(1.0 - 0.25 * random)
    }

    /// Wait for a turn under both ceilings; false if the pool was cancelled.
    fn turn(&self, tokens: u64) -> bool {
        !self.cancelled()
            && self.pause(self.shared.rps.take(1.0))
            && self.pause(self.shared.tps.take(tokens as f64))
            && !self.cancelled()
    }

    fn succeeded(&self) {
        let mut h = crate::lock(&self.shared.health);
        (h.ok, h.failing) = (Instant::now(), None);
        drop(h);
        self.shared.rps.recover();
    }

    /// An attempt failed; how long attempts have been failing.
    fn failed(&self) -> Duration {
        let mut h = crate::lock(&self.shared.health);
        h.failing.get_or_insert_with(Instant::now).elapsed()
    }

    fn cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::SeqCst)
    }

    /// Sleep for `d`, or until the pool is cancelled; false if it was.
    fn pause(&self, d: Duration) -> bool {
        let end = Instant::now() + d;
        while !self.cancelled() {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(Duration::from_millis(20)));
        }
        false
    }
}

/// A token bucket: at most `max` a second, with a burst of one second's
/// worth. The request bucket also adapts: it halves its rate when the API
/// asks deem to slow down, at most once a second, and grows it back while
/// requests succeed, each second by a twentieth or by one, whichever is
/// more: from half of 80 back to 80 in about 14 s.
struct Bucket {
    max: f64,
    s: Mutex<Level>,
}

struct Level {
    rate: f64,
    tokens: f64,
    at: Instant,
    /// when the rate was last halved
    cut: Option<Instant>,
    /// no turns before this: a `Retry-After`
    hold: Instant,
}

impl Bucket {
    fn new(max: f64) -> Self {
        let now = Instant::now();
        Self { max, s: Mutex::new(Level { rate: max, tokens: max, at: now, cut: None, hold: now }) }
    }

    /// Take `n`; how long to wait before using them.
    fn take(&self, n: f64) -> Duration {
        let mut s = crate::lock(&self.s);
        let now = Instant::now();
        s.tokens = (s.tokens + now.duration_since(s.at).as_secs_f64() * s.rate).min(s.rate) - n;
        s.at = now;
        let deficit = Duration::from_secs_f64((-s.tokens).max(0.0) / s.rate);
        deficit.max(s.hold.saturating_duration_since(now))
    }

    fn slow_down(&self) {
        let mut s = crate::lock(&self.s);
        if s.cut.is_none_or(|c| c.elapsed() >= Duration::from_secs(1)) {
            s.rate = (s.rate / 2.0).max(1.0);
            s.tokens = s.tokens.min(s.rate);
            s.cut = Some(Instant::now());
        }
    }

    fn recover(&self) {
        let mut s = crate::lock(&self.s);
        // a success a turn: `rate` of them a second
        s.rate = (s.rate + (1.0 / s.rate).max(0.05)).min(self.max);
    }

    fn hold(&self, d: Duration) {
        let mut s = crate::lock(&self.s);
        s.hold = s.hold.max(Instant::now() + d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::api::Spec;
    use serde_json::value::RawValue;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::sync_channel;

    /// Always busy; counts the attempts.
    struct Busy(AtomicUsize);

    impl Judge for Busy {
        fn judge(&self, _: &Request) -> Result<Response, ApiError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(ApiError::Status(503, "busy".into()))
        }
    }

    // A cancel stops a request between its retries, and a queued request
    // is never sent.
    #[test]
    fn a_cancel_sends_nothing_more() {
        let clock = Arc::new(Clock::new(true));
        let trace = Arc::new(Trace::off(clock.clone()));
        let busy = Arc::new(Busy(AtomicUsize::new(0)));
        let retry =
            Retry { backoff: Duration::from_secs(60), busy_for: Duration::from_secs(60), notice_after: Duration::MAX };
        let pool = Pool::new(busy.clone(), 1, Rate { requests: 80.0, tokens: 100_000.0 }, retry, clock, trace);
        let (tx, rx) = sync_channel(2);
        let state = RawValue::from_string(r#""a""#.into()).unwrap();
        let spec = Spec { q: "q".into(), ..Spec::default() };
        for n in 1..=2 {
            let req = crate::transport::request::requests("m", &[(&state, &spec)]).remove(0).req;
            let tx = tx.clone();
            let done: Done = Box::new(move |r, attempts, _| {
                let _ = tx.send((n, r.is_err(), attempts));
            });
            pool.submit(Job { req, bytes: 0, tokens: 1, ids: vec![n], n, at: 0, done });
        }
        while busy.0.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        pool.cancel();
        let t = Duration::from_secs(5);
        assert_eq!(rx.recv_timeout(t), Ok((1, true, 1)), "not the minute's backoff, nor a retry");
        assert_eq!(rx.recv_timeout(t), Ok((2, true, 0)), "the queued one, unsent");
        assert_eq!(busy.0.load(Ordering::SeqCst), 1);
    }
}
