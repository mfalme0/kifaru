//! Transfers and their state machine.

use crate::account::AccountId;
use crate::error::TransferError;
use crate::money::{Currency, Money};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Stable identifier for a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(Uuid);

impl TransferId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for TransferId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle state of a transfer.
///
/// ```text
///   Pending ──complete──► Completed   (terminal)
///      │
///      └──fail──────────► Failed      (terminal)
/// ```
///
/// A transfer is created `Pending` and reaches a terminal state exactly once.
/// `Pending` is never observable *outside* the committing transaction: a
/// transfer that fails validation is recorded `Failed`, and one that succeeds is
/// recorded `Completed`, both within the same database transaction. Every
/// transfer that created a balance movement is `Completed` (INV-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TransferStatus {
    Pending,
    Completed,
    Failed,
}

impl TransferStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, TransferStatus::Completed | TransferStatus::Failed)
    }

    const fn code(self) -> &'static str {
        match self {
            TransferStatus::Pending => "pending",
            TransferStatus::Completed => "completed",
            TransferStatus::Failed => "failed",
        }
    }
}

impl fmt::Display for TransferStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Why a transfer failed, carried so the failure is diagnosable after the fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureReason(String);

impl FailureReason {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A transfer between two accounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub id: TransferId,
    pub source: AccountId,
    pub destination: AccountId,
    pub amount: Money,
    pub status: TransferStatus,
    pub failure_reason: Option<FailureReason>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Transfer {
    /// Create a transfer in the `Pending` state.
    ///
    /// Rejects a transfer to the same account: it would post a debit and a credit
    /// against one account, which the ledger treats as malformed
    /// (see [`crate::error::LedgerError::SameAccount`]).
    pub fn new(
        source: AccountId,
        destination: AccountId,
        amount: Money,
        created_at: DateTime<Utc>,
    ) -> Result<Self, TransferError> {
        if source == destination {
            return Err(TransferError::SelfTransfer);
        }
        if !amount.is_positive() {
            return Err(TransferError::Money(
                crate::error::MoneyError::NotPositive {
                    minor_units: amount.minor_units(),
                },
            ));
        }
        Ok(Self {
            id: TransferId::new(),
            source,
            destination,
            amount,
            status: TransferStatus::Pending,
            failure_reason: None,
            created_at,
            updated_at: created_at,
        })
    }

    /// Rehydrate from persisted state.
    #[allow(clippy::too_many_arguments)]
    pub const fn from_parts(
        id: TransferId,
        source: AccountId,
        destination: AccountId,
        amount: Money,
        status: TransferStatus,
        failure_reason: Option<FailureReason>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            source,
            destination,
            amount,
            status,
            failure_reason,
            created_at,
            updated_at,
        }
    }

    pub const fn currency(&self) -> Currency {
        self.amount.currency()
    }

    /// Move to `Completed`. Only legal from `Pending`.
    pub fn complete(&mut self, at: DateTime<Utc>) -> Result<(), TransferError> {
        self.transition(TransferStatus::Completed, None, at)
    }

    /// Move to `Failed`. Only legal from `Pending`.
    pub fn fail(&mut self, reason: FailureReason, at: DateTime<Utc>) -> Result<(), TransferError> {
        self.transition(TransferStatus::Failed, Some(reason), at)
    }

    fn transition(
        &mut self,
        to: TransferStatus,
        reason: Option<FailureReason>,
        at: DateTime<Utc>,
    ) -> Result<(), TransferError> {
        let from = self.status;
        // Both terminal states are reachable only from Pending. Going from a
        // terminal state anywhere is refused, so a transfer cannot be completed
        // twice or flipped from Completed to Failed.
        if !matches!(from, TransferStatus::Pending) || !to.is_terminal() {
            return Err(TransferError::IllegalStateTransition { from, to });
        }
        self.status = to;
        self.failure_reason = reason;
        self.updated_at = at;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const KES: Currency = Currency::Kes;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
    }

    fn transfer() -> Transfer {
        Transfer::new(
            AccountId::new(),
            AccountId::new(),
            Money::positive(1_000, KES).unwrap(),
            now(),
        )
        .unwrap()
    }

    #[test]
    fn new_transfers_are_pending() {
        let t = transfer();
        assert_eq!(t.status, TransferStatus::Pending);
        assert!(t.failure_reason.is_none());
        assert!(!t.status.is_terminal());
    }

    #[test]
    fn self_transfer_is_rejected() {
        let a = AccountId::new();
        let r = Transfer::new(a, a, Money::positive(10, KES).unwrap(), now());
        assert_eq!(r, Err(TransferError::SelfTransfer));
    }

    #[test]
    fn zero_amount_is_rejected() {
        let r = Transfer::new(
            AccountId::new(),
            AccountId::new(),
            Money::from_minor_units(0, KES).unwrap(),
            now(),
        );
        assert!(matches!(r, Err(TransferError::Money(_))));
    }

    #[test]
    fn pending_can_complete() {
        let mut t = transfer();
        t.complete(now()).unwrap();
        assert_eq!(t.status, TransferStatus::Completed);
        assert!(t.failure_reason.is_none());
    }

    #[test]
    fn pending_can_fail_with_a_reason() {
        let mut t = transfer();
        t.fail(FailureReason::new("insufficient funds"), now())
            .unwrap();
        assert_eq!(t.status, TransferStatus::Failed);
        assert_eq!(t.failure_reason.unwrap().as_str(), "insufficient funds");
    }

    #[test]
    fn completed_transfer_cannot_be_completed_again() {
        let mut t = transfer();
        t.complete(now()).unwrap();
        let r = t.complete(now());
        assert_eq!(
            r,
            Err(TransferError::IllegalStateTransition {
                from: TransferStatus::Completed,
                to: TransferStatus::Completed
            })
        );
    }

    #[test]
    fn completed_transfer_cannot_be_flipped_to_failed() {
        let mut t = transfer();
        t.complete(now()).unwrap();
        let r = t.fail(FailureReason::new("late failure"), now());
        assert!(matches!(
            r,
            Err(TransferError::IllegalStateTransition { .. })
        ));
        // The reason is not applied.
        assert!(t.failure_reason.is_none());
    }

    #[test]
    fn failed_transfer_is_terminal() {
        let mut t = transfer();
        t.fail(FailureReason::new("nope"), now()).unwrap();
        assert!(t.fail(FailureReason::new("again"), now()).is_err());
        assert!(t.complete(now()).is_err());
    }
}
