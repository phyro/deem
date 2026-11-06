//! The run's time: real, or virtual for tests.
//!
//! Why deem needs its own clock: what deem does depends on time, and real
//! time makes that untestable. When a wave goes out depends on a quiet
//! period while requests are in flight, a retry waits out a backoff, and
//! `--trace` stamps every event. On real time a test can check that things
//! happened, but not when: timestamps change from run to run, and decisions
//! made on timeouts change with the machine's load. The questions worth
//! testing are about time, though: did the record wait for one round trip
//! or two, did the fast request's records go on without the slow one.
//!
//! On virtual time only the API takes time: the fake API says how much with
//! [`elapse`], and deem's own work takes none. A run then has the same
//! timestamps and durations every time, and a test can state them: a
//! record that waits on one 100 µs request ends at 100.
//!
//! Virtual time is kept per thread, because deem's threads really do run
//! at once: a single counter would depend on how they were scheduled.
//! Instead a thread's time moves on when something reaches it from later
//! on: a request starts when its round went, and a record resumes when its
//! round came back. A record carries its own time between runs, and the
//! worker that runs it starts from that.
//!
//! It is its own module, not part of the trace, because the batcher and the
//! senders carry its times too: when a round went, when an answer came back.
//! Nothing deem decides waits on a timer of its own: a round goes when its
//! record waits, so what is sent doesn't depend on time at all.

use std::cell::Cell;
use std::time::Instant;

thread_local! {
    /// This thread's virtual time, in microseconds.
    static VIRTUAL: Cell<u64> = const { Cell::new(0) };
}

/// On virtual time, this thread spends `us`: a request's latency, a retry's
/// backoff.
pub fn elapse(us: u64) {
    VIRTUAL.with(|v| v.set(v.get() + us));
}

pub struct Clock {
    /// `None`: virtual time
    start: Option<Instant>,
}

impl Clock {
    pub fn new(virtual_time: bool) -> Self {
        Self { start: (!virtual_time).then(Instant::now) }
    }

    pub fn is_virtual(&self) -> bool {
        self.start.is_none()
    }

    /// Microseconds since the run started.
    pub fn now(&self) -> u64 {
        match self.start {
            Some(s) => s.elapsed().as_micros() as u64,
            None => VIRTUAL.get(),
        }
    }

    /// Something that happened at `at` reaches this thread: on virtual
    /// time, it can't go on before then.
    pub fn reach(&self, at: u64) {
        if self.is_virtual() {
            VIRTUAL.with(|v| v.set(v.get().max(at)));
        }
    }

    /// This thread starts on something that happened at `at`, whatever it
    /// did before: a sender, on a request. Returns now.
    pub fn start_at(&self, at: u64) -> u64 {
        if self.is_virtual() {
            VIRTUAL.set(at);
        }
        self.now()
    }
}
