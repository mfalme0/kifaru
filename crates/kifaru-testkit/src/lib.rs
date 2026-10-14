//! Test fixtures, deterministic seeding, and invariant assertions.
//!
//! Kept as a normal crate (not `#[cfg(test)]`) so integration tests under
//! `tests/` can use it. **Nothing here may be depended on by production
//! crates** — it talks directly to the database and can write state a real
//! transfer never would (see `harness::Harness::seed_opening_balance`).

#![forbid(unsafe_code)]

pub mod harness;

pub use kifaru_domain as domain;
