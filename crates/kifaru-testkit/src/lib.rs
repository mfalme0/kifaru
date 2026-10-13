//! Test fixtures, deterministic seeding, and invariant assertions.
//!
//! Kept as a normal crate (not `#[cfg(test)]`) so integration tests under
//! `tests/` can use it. Nothing here may be depended on by production crates.
//!
//! Populated in the next step of the build sequence.

#![forbid(unsafe_code)]

pub use kifaru_domain as domain;
