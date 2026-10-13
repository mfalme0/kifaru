//! Errors raised by domain rules.
//!
//! These are *rule violations*, not transport concerns. The HTTP layer maps them
//! to status codes in `kifaru-api`; the domain has no knowledge of HTTP.

use crate::money::Currency;
use thiserror::Error;

/// A monetary value was constructed or combined in a way the domain forbids.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MoneyError {
    /// A negative monetary value is not representable: KIFARU's default overdraft
    /// policy prohibits negative balances (INV-1).
    #[error("monetary value must not be negative, got {minor_units} minor units")]
    Negative { minor_units: i64 },

    /// A transfer leg must carry a strictly positive amount.
    #[error("monetary value must be strictly positive, got {minor_units} minor units")]
    NotPositive { minor_units: i64 },

    /// Two monetary values of different currencies cannot be combined.
    #[error("currency mismatch: expected {expected}, found {found}")]
    CurrencyMismatch { expected: Currency, found: Currency },

    /// The arithmetic result does not fit in `i64` minor units.
    #[error("monetary arithmetic overflowed i64 minor units")]
    Overflow,

    /// Subtraction would produce a negative value.
    #[error("subtraction would produce a negative balance")]
    WouldGoNegative,
}

/// A transfer attempt violated a domain rule.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransferError {
    #[error("source and destination accounts must differ")]
    SelfTransfer,

    #[error("source account is not active (status: {0})")]
    SourceNotActive(crate::account::AccountStatus),

    #[error("destination account is not active (status: {0})")]
    DestinationNotActive(crate::account::AccountStatus),

    #[error(
        "account currency {account_currency} does not match transfer currency {transfer_currency}"
    )]
    AccountCurrencyMismatch {
        account_currency: Currency,
        transfer_currency: Currency,
    },

    #[error("insufficient funds: balance {balance} minor units, required {required} minor units")]
    InsufficientFunds { balance: i64, required: i64 },

    #[error("illegal transfer state transition from {from} to {to}")]
    IllegalStateTransition {
        from: crate::transfer::TransferStatus,
        to: crate::transfer::TransferStatus,
    },

    #[error(transparent)]
    Money(#[from] MoneyError),
}

/// A ledger consistency rule was violated.
///
/// Reaching any of these in a committed state means a financial invariant has
/// already been broken; they exist to fail loudly rather than silently.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LedgerError {
    /// INV-2: a transfer must have exactly one debit and one credit.
    #[error("ledger transaction for {transfer_id} has {entries} entries, expected exactly 2")]
    WrongEntryCount {
        transfer_id: uuid::Uuid,
        entries: usize,
    },

    /// INV-2: both legs must carry the same magnitude.
    #[error("ledger legs are unequal: debit {debit} minor units vs credit {credit} minor units")]
    UnbalancedMagnitude { debit: i64, credit: i64 },

    /// INV-3: a ledger transaction must net to zero.
    #[error("ledger transaction does not balance: net {net} minor units")]
    DoesNotBalance { net: i64 },

    #[error("ledger entries reference different transfers")]
    TransferMismatch,

    #[error("ledger entries reference the same account")]
    SameAccount,

    #[error("ledger entries use different currencies")]
    CurrencyMismatch,

    /// INV-7: committed entries are append-only.
    #[error("committed ledger entries are immutable (INV-7)")]
    Immutable,
}

/// An account state machine rule was violated.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AccountError {
    #[error("illegal account state transition from {from} to {to}")]
    IllegalStateTransition {
        from: crate::account::AccountStatus,
        to: crate::account::AccountStatus,
    },
}
