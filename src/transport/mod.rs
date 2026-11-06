//! Talking to the API: the request types and the client (`api`), building
//! requests from questions (`request`), the senders (`sched`), the batcher
//! that sends a record's round (`batch`), a record's side of it (`asker`),
//! answers and the rounds waiting on them (`promise`), and pictures
//! (`picture`).

pub mod api;
pub mod asker;
pub mod batch;
pub mod picture;
mod promise;
pub mod request;
pub mod sched;
