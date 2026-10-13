//! Accounts and their state machine.

use crate::error::{AccountError, TransferError};
use crate::money::{Currency, Money};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Stable identifier for an account.
///
/// Backed by UUIDv7 so identifiers sort by creation time, which keeps index
/// locality reasonable and makes `ORDER BY id` a meaningful lock order for
/// deadlock avoidance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(Uuid);

impl AccountId {
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

impl Default for AccountId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle state of an account.
///
/// Permitted transitions (enforced by [`Account::transition_to`]):
///
/// ```text
///            suspend              reactivate
///   Active  ────────►  Suspended  ──────────►  Active
///      │                   │
///      │ close             │ close
///      ▼                   ▼
///   Closed       ◄─────────┘          (terminal — no transitions out)
/// ```
///
/// `Closed` is terminal. Money may not move out of, or into, a non-`Active`
/// account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AccountStatus {
    Active,
    Suspended,
    Closed,
}

impl AccountStatus {
    /// Whether transfers may debit or credit this account.
    pub const fn is_active(self) -> bool {
        matches!(self, AccountStatus::Active)
    }

    /// Whether this status can ever change again.
    pub const fn is_terminal(self) -> bool {
        matches!(self, AccountStatus::Closed)
    }

    const fn code(self) -> &'static str {
        match self {
            AccountStatus::Active => "active",
            AccountStatus::Suspended => "suspended",
            AccountStatus::Closed => "closed",
        }
    }
}

impl fmt::Display for AccountStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// An account and its authoritative balance.
///
/// `version` is optimistic-concurrency metadata carried in the schema. The
/// transfer path does not rely on it for correctness — row locks do that — but
/// it makes the row's write count observable and gives the reconciliation report
/// something concrete to cite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub status: AccountStatus,
    pub currency: Currency,
    pub balance: Money,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl Account {
    /// Open a new account at zero balance.
    pub fn open(currency: Currency, created_at: DateTime<Utc>) -> Self {
        Self {
            id: AccountId::new(),
            status: AccountStatus::Active,
            currency,
            balance: Money::zero(currency),
            version: 0,
            created_at,
        }
    }

    /// Rehydrate an account from persisted state. Used by storage adapters.
    pub const fn from_parts(
        id: AccountId,
        status: AccountStatus,
        currency: Currency,
        balance: Money,
        version: i64,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            status,
            currency,
            balance,
            version,
            created_at,
        }
    }

    /// Apply a status change if the transition is legal.
    pub fn transition_to(&mut self, to: AccountStatus) -> Result<(), AccountError> {
        let from = self.status;
        let legal = matches!(
            (from, to),
            (AccountStatus::Active, AccountStatus::Suspended)
                | (AccountStatus::Suspended, AccountStatus::Active)
                | (AccountStatus::Active, AccountStatus::Closed)
                | (AccountStatus::Suspended, AccountStatus::Closed)
        ) || from == to;

        if !legal {
            return Err(AccountError::IllegalStateTransition { from, to });
        }
        self.status = to;
        Ok(())
    }

    /// Check that this account can participate as the source of a transfer.
    pub fn ensure_can_debit(&self, amount: Money) -> Result<(), TransferError> {
        if !self.status.is_active() {
            return Err(TransferError::SourceNotActive(self.status));
        }
        if self.currency != amount.currency() {
            return Err(TransferError::AccountCurrencyMismatch {
                account_currency: self.currency,
                transfer_currency: amount.currency(),
            });
        }
        if self.balance.minor_units() < amount.minor_units() {
            return Err(TransferError::InsufficientFunds {
                balance: self.balance.minor_units(),
                required: amount.minor_units(),
            });
        }
        Ok(())
    }

    /// Check that this account can participate as the destination of a transfer.
    pub fn ensure_can_credit(&self, amount: Money) -> Result<(), TransferError> {
        if !self.status.is_active() {
            return Err(TransferError::DestinationNotActive(self.status));
        }
        if self.currency != amount.currency() {
            return Err(TransferError::AccountCurrencyMismatch {
                account_currency: self.currency,
                transfer_currency: amount.currency(),
            });
        }
        Ok(())
    }

    /// Debit this account in memory. Callers must have run `ensure_can_debit`.
    pub fn debit(&mut self, amount: Money) -> Result<(), TransferError> {
        self.ensure_can_debit(amount)?;
        self.balance = self.balance.checked_sub(amount)?;
        self.version += 1;
        Ok(())
    }

    /// Credit this account in memory. Callers must have run `ensure_can_credit`.
    pub fn credit(&mut self, amount: Money) -> Result<(), TransferError> {
        self.ensure_can_credit(amount)?;
        self.balance = self.balance.checked_add(amount)?;
        self.version += 1;
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

    fn funded(minor_units: i64) -> Account {
        let mut a = Account::open(KES, now());
        a.balance = Money::from_minor_units(minor_units, KES).unwrap();
        a
    }

    #[test]
    fn new_accounts_are_active_and_empty() {
        let a = Account::open(KES, now());
        assert_eq!(a.status, AccountStatus::Active);
        assert!(a.balance.is_zero());
        assert_eq!(a.version, 0);
    }

    #[test]
    fn closed_is_terminal() {
        let mut a = Account::open(KES, now());
        a.transition_to(AccountStatus::Closed).unwrap();
        assert!(a.transition_to(AccountStatus::Active).is_err());
        assert!(a.transition_to(AccountStatus::Suspended).is_err());
    }

    #[test]
    fn can_suspend_and_reactivate() {
        let mut a = Account::open(KES, now());
        a.transition_to(AccountStatus::Suspended).unwrap();
        a.transition_to(AccountStatus::Active).unwrap();
        assert_eq!(a.status, AccountStatus::Active);
    }

    #[test]
    fn suspended_accounts_cannot_move_money() {
        let mut src = funded(1_000);
        let dst = funded(0);
        src.transition_to(AccountStatus::Suspended).unwrap();

        let amount = Money::positive(100, KES).unwrap();
        assert!(matches!(
            src.ensure_can_debit(amount),
            Err(TransferError::SourceNotActive(AccountStatus::Suspended))
        ));
        assert!(dst.ensure_can_credit(amount).is_ok());

        let mut suspended_dst = funded(0);
        suspended_dst
            .transition_to(AccountStatus::Suspended)
            .unwrap();
        assert!(matches!(
            suspended_dst.ensure_can_credit(amount),
            Err(TransferError::DestinationNotActive(_))
        ));
    }

    #[test]
    fn debit_cannot_overdraw() {
        let mut a = funded(100);
        let amount = Money::positive(250, KES).unwrap();
        assert!(matches!(
            a.debit(amount),
            Err(TransferError::InsufficientFunds {
                balance: 100,
                required: 250
            })
        ));
        // Balance is untouched after a refused debit.
        assert_eq!(a.balance.minor_units(), 100);
        assert_eq!(a.version, 0);
    }

    #[test]
    fn debit_to_exact_zero_succeeds_and_bumps_version() {
        let mut a = funded(100);
        a.debit(Money::positive(100, KES).unwrap()).unwrap();
        assert!(a.balance.is_zero());
        assert_eq!(a.version, 1);
    }

    #[test]
    fn credit_accumulates_exactly() {
        let mut a = funded(0);
        a.credit(Money::positive(1, KES).unwrap()).unwrap();
        a.credit(Money::positive(2, KES).unwrap()).unwrap();
        assert_eq!(a.balance.minor_units(), 3);
        assert_eq!(a.version, 2);
    }

    #[test]
    fn account_ids_are_unique_and_time_ordered() {
        let a = AccountId::new();
        let b = AccountId::new();
        assert_ne!(a, b);
        assert!(a < b, "UUIDv7 should sort by creation order");
    }
}
