//! deem: grep by meaning. A jq program runs on every record and asks a model
//! typed questions through `judge`; a record's independent questions go out
//! together.
//!
//! The library, as the `deem` binary uses it:
//!
//! - [`programs`] finds a program by name, path or jq text, and holds the
//!   built-in ones and the question forms;
//! - [`run`] runs one on an input, against a [`api::Judge`], and returns a
//!   [`Report`];
//! - [`trace`] writes `--trace` and draws `deem trace`;
//! - [`engine`] is jq, run again until it has every answer, for running a
//!   program on one record yourself.
//!
//! Inside, the layers depend one way: `programs` → `engine`; `run` →
//! `engine`, `transport`, `config`, `trace`; `config` → `transport` →
//! `trace` → `clock`; `engine` uses only `transport::api`'s types
//! (`tests/layers.rs` checks it).

mod clock;
mod config;
pub mod engine;
pub mod programs;
mod run;
pub mod trace;
mod transport;

pub use config::{Config, NO_KEY, apis};
pub use run::input::{Input, open_input};
pub use run::output::Chunk;
pub use run::{Report, RunOptions, run};
pub use transport::api;

/// Hooks for tests: virtual time, and how requests are built.
#[doc(hidden)]
pub mod testing {
    pub use crate::clock::elapse;
    pub use crate::transport::request::{BYTES_PER_TOKEN, requests, tokens};
}

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// Locks are never poisoned in a way that matters: record panics are
/// caught before they can hold one.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn wait<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    cv.wait(g).unwrap_or_else(PoisonError::into_inner)
}
