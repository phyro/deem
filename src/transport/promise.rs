//! A question's answer that will arrive, and the rounds that wait for
//! answers: a round is told once the last of its answers is set, on
//! whatever thread sets it.

use crate::transport::api::{Answer, Failure};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// An answer that will arrive, or why none will.
pub struct Promise {
    pub(super) answer: OnceLock<Result<Answer, Failure>>,
    /// the question's number in this run, for `--trace`
    pub(super) id: u64,
    /// when the answer arrived
    pub(super) at: AtomicU64,
    /// the rounds waiting for it
    waiters: Mutex<Vec<Arc<Round>>>,
}

impl Promise {
    pub(super) fn new(id: u64) -> Self {
        Self { answer: OnceLock::new(), id, at: AtomicU64::new(0), waiters: Mutex::default() }
    }

    pub fn is_done(&self) -> bool {
        self.answer.get().is_some()
    }

    /// The answer, once; returns the rounds waiting for it, to be told.
    pub(super) fn set(&self, r: Result<Answer, Failure>) -> Vec<Arc<Round>> {
        let mut waiters = crate::lock(&self.waiters);
        let _ = self.answer.set(r);
        std::mem::take(&mut *waiters)
    }
}

/// A record's round, waiting for its answers: `then` is called once the
/// last of them arrives, on whatever thread brings it.
pub(super) struct Round {
    left: AtomicUsize,
    then: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Round {
    /// Call `then` once each of `promises` is answered: now, if they are.
    pub(super) fn wait(promises: impl Iterator<Item = Arc<Promise>>, then: Box<dyn FnOnce() + Send>) {
        // one for this call, so `then` can't run before every promise is in
        let round = Arc::new(Round { left: AtomicUsize::new(1), then: Mutex::new(Some(then)) });
        for p in promises {
            let mut waiters = crate::lock(&p.waiters);
            if !p.is_done() {
                round.left.fetch_add(1, Ordering::SeqCst);
                waiters.push(round.clone());
            }
        }
        round.answered();
    }

    pub(super) fn answered(&self) {
        if self.left.fetch_sub(1, Ordering::SeqCst) == 1
            && let Some(then) = crate::lock(&self.then).take()
        {
            then();
        }
    }
}
