//! Application services and the ports they depend on.
//!
//! This crate orchestrates domain rules against storage. It defines *ports*
//! (traits) and never depends on a concrete database, so the transfer logic can
//! be tested against an in-memory implementation.
//!
//! Populated in the next step of the build sequence.

#![forbid(unsafe_code)]

pub use kifaru_domain as domain;
