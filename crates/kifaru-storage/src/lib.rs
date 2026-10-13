//! PostgreSQL adapters.
//!
//! Implements the ports declared in `kifaru-application`. This is the only crate
//! that knows SQL or `sqlx`.
//!
//! Populated in the next step of the build sequence.

#![forbid(unsafe_code)]

pub use kifaru_domain as domain;
