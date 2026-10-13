//! HTTP transport, background worker, and observability.
//!
//! Owns the `kifaru` binary (`serve` / `worker` / `reconcile`). Contains no
//! business rules — it validates input, maps domain errors to status codes, and
//! delegates to `kifaru-application`.
//!
//! Populated in the next step of the build sequence.

#![forbid(unsafe_code)]

pub use kifaru_domain as domain;
