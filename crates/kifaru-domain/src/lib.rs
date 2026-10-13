//! KIFARU domain model.
//!
//! Pure business rules: money, accounts, transfers, and the double-entry ledger.
//! This crate has **no database, no HTTP, and no async runtime** as a dependency,
//! so every rule here is testable in isolation and cannot be bypassed by a
//! caller reaching for a query builder.
//!
//! The design intent is that financial invariants are *structural* rather than
//! remembered:
//!
//! - `Money` cannot be negative, so INV-1 is hard to violate by accident.
//! - `LedgerTransaction` can only be built as a balanced debit/credit pair, so
//!   INV-2 and INV-3 hold by construction.
//! - `LedgerEntry` has no mutating API, so INV-7 is enforced by absence.
//!
//! Invariants that require the database to check (INV-4 balance agreement, INV-5
//! atomicity, INV-6 idempotency, INV-8 storage fidelity) are enforced in
//! `kifaru-storage` and asserted by the test suite. See `docs/invariants.md`.

pub mod account;
pub mod error;
pub mod ledger;
pub mod money;
pub mod transfer;

pub use account::{Account, AccountId, AccountStatus};
pub use error::{AccountError, LedgerError, MoneyError, TransferError};
pub use ledger::{EntryDirection, LedgerEntry, LedgerTransaction, derived_balance_minor_units};
pub use money::{Currency, Money};
pub use transfer::{FailureReason, Transfer, TransferId, TransferStatus};
