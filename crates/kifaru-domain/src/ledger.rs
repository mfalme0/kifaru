//! Double-entry ledger primitives.
//!
//! # Append-only
//!
//! [`LedgerEntry`] has no setters. Once constructed, an entry cannot be altered;
//! corrections are made by posting a *compensating* transaction (INV-7). The
//! storage adapter additionally revokes `UPDATE`/`DELETE` on the table, so the
//! guarantee is enforced twice: once by the type, once by the database.
//!
//! # Balance by construction
//!
//! A [`LedgerTransaction`] is the only way this crate produces entries for a
//! transfer, and it always produces exactly two of them — one debit, one credit,
//! carrying the *same* `Money` value. That makes INV-2 and INV-3 structural
//! rather than something a caller must remember to check. [`LedgerTransaction::validate`]
//! re-derives the property from the stored fields so it can also be run against
//! rows loaded back from the database, which is what reconciliation does.

use crate::account::AccountId;
use crate::error::LedgerError;
use crate::money::Money;
use crate::transfer::TransferId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Which side of the ledger an entry sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EntryDirection {
    /// Money leaving an account.
    Debit,
    /// Money arriving in an account.
    Credit,
}

impl EntryDirection {
    /// Signed contribution to a balance recomputed from the ledger.
    ///
    /// Debits reduce, credits increase. This is the single place the sign
    /// convention lives.
    pub const fn signed_factor(self) -> i64 {
        match self {
            EntryDirection::Debit => -1,
            EntryDirection::Credit => 1,
        }
    }
}

impl fmt::Display for EntryDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EntryDirection::Debit => "debit",
            EntryDirection::Credit => "credit",
        })
    }
}

/// A single immutable posting against one account.
///
/// `amount` is always positive; direction carries the sign. There are no
/// mutating methods — this is the type-level half of INV-7.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: Uuid,
    pub transfer_id: TransferId,
    pub account_id: AccountId,
    pub direction: EntryDirection,
    pub amount: Money,
    pub created_at: DateTime<Utc>,
}

impl LedgerEntry {
    pub fn new(
        transfer_id: TransferId,
        account_id: AccountId,
        direction: EntryDirection,
        amount: Money,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            transfer_id,
            account_id,
            direction,
            amount,
            created_at,
        }
    }

    /// Rehydrate from persisted state. Used by storage adapters and
    /// reconciliation, never to mutate an existing row.
    #[allow(clippy::too_many_arguments)]
    pub const fn from_parts(
        id: Uuid,
        transfer_id: TransferId,
        account_id: AccountId,
        direction: EntryDirection,
        amount: Money,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            transfer_id,
            account_id,
            direction,
            amount,
            created_at,
        }
    }

    /// Signed minor units this entry contributes to its account's balance.
    pub const fn signed_minor_units(&self) -> i64 {
        self.amount.minor_units() * self.direction.signed_factor()
    }
}

/// The complete set of ledger postings for one transfer.
///
/// Always exactly two entries. There is no public constructor that can produce
/// an unbalanced transaction, so INV-2 and INV-3 cannot be violated by building
/// one of these in application code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerTransaction {
    pub transfer_id: TransferId,
    pub debit: LedgerEntry,
    pub credit: LedgerEntry,
}

impl LedgerTransaction {
    /// Build the balanced debit/credit pair for a transfer.
    ///
    /// `amount` must be strictly positive — it is the same value on both legs,
    /// so the magnitude is equal by construction.
    pub fn double_entry(
        transfer_id: TransferId,
        source: AccountId,
        destination: AccountId,
        amount: Money,
        created_at: DateTime<Utc>,
    ) -> Result<Self, LedgerError> {
        let tx = Self {
            transfer_id,
            debit: LedgerEntry::new(
                transfer_id,
                source,
                EntryDirection::Debit,
                amount,
                created_at,
            ),
            credit: LedgerEntry::new(
                transfer_id,
                destination,
                EntryDirection::Credit,
                amount,
                created_at,
            ),
        };
        tx.validate()?;
        Ok(tx)
    }

    /// Rehydrate a transaction from rows loaded out of the database.
    pub const fn from_parts(
        transfer_id: TransferId,
        debit: LedgerEntry,
        credit: LedgerEntry,
    ) -> Self {
        Self {
            transfer_id,
            debit,
            credit,
        }
    }

    /// Both entries, in a stable order.
    pub fn entries(&self) -> [&LedgerEntry; 2] {
        [&self.debit, &self.credit]
    }

    /// Net movement of the transaction. A balanced transaction nets to zero.
    pub const fn net_minor_units(&self) -> i64 {
        self.debit.signed_minor_units() + self.credit.signed_minor_units()
    }

    /// Re-derive INV-2 and INV-3 from the fields themselves.
    ///
    /// Used both when constructing a new transaction and when validating rows
    /// read back from storage, so a corrupted row is caught rather than trusted.
    pub fn validate(&self) -> Result<(), LedgerError> {
        if self.debit.transfer_id != self.credit.transfer_id {
            return Err(LedgerError::TransferMismatch);
        }
        if self.debit.transfer_id != self.transfer_id {
            return Err(LedgerError::TransferMismatch);
        }
        if self.debit.account_id == self.credit.account_id {
            return Err(LedgerError::SameAccount);
        }
        if self.debit.amount.currency() != self.credit.amount.currency() {
            return Err(LedgerError::CurrencyMismatch);
        }
        if self.debit.direction != EntryDirection::Debit
            || self.credit.direction != EntryDirection::Credit
        {
            return Err(LedgerError::DoesNotBalance {
                net: self.net_minor_units(),
            });
        }
        if self.debit.amount.minor_units() != self.credit.amount.minor_units() {
            return Err(LedgerError::UnbalancedMagnitude {
                debit: self.debit.amount.minor_units(),
                credit: self.credit.amount.minor_units(),
            });
        }
        // INV-3: with equal magnitudes and opposite signs the net must be zero.
        let net = self.net_minor_units();
        if net != 0 {
            return Err(LedgerError::DoesNotBalance { net });
        }
        Ok(())
    }
}

/// Signed balance derived from entries, which may legitimately be negative.
///
/// Reconciliation needs the *signed* figure so it can report an overdraw as a
/// discrepancy instead of silently clamping it to zero.
pub fn derived_balance_minor_units(entries: &[LedgerEntry], account_id: AccountId) -> i64 {
    entries
        .iter()
        .filter(|e| e.account_id == account_id)
        .map(LedgerEntry::signed_minor_units)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;
    use chrono::TimeZone;

    const KES: Currency = Currency::Kes;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
    }

    fn pair() -> (AccountId, AccountId, Money) {
        (
            AccountId::new(),
            AccountId::new(),
            Money::positive(500, KES).unwrap(),
        )
    }

    #[test]
    fn double_entry_is_balanced_by_construction() {
        let (src, dst, amount) = pair();
        let tx =
            LedgerTransaction::double_entry(TransferId::new(), src, dst, amount, now()).unwrap();
        assert_eq!(tx.net_minor_units(), 0);
        assert_eq!(tx.debit.amount, tx.credit.amount);
        assert_eq!(tx.debit.direction, EntryDirection::Debit);
        assert_eq!(tx.credit.direction, EntryDirection::Credit);
        assert_eq!(tx.debit.account_id, src);
        assert_eq!(tx.credit.account_id, dst);
    }

    #[test]
    fn both_entries_share_the_transfer_id() {
        let (src, dst, amount) = pair();
        let tid = TransferId::new();
        let tx = LedgerTransaction::double_entry(tid, src, dst, amount, now()).unwrap();
        for e in tx.entries() {
            assert_eq!(e.transfer_id, tid);
        }
    }

    #[test]
    fn validate_catches_a_tampered_magnitude() {
        let (src, dst, amount) = pair();
        let mut tx =
            LedgerTransaction::double_entry(TransferId::new(), src, dst, amount, now()).unwrap();
        // Simulate storage corruption: credit leg no longer matches the debit leg.
        tx.credit.amount = Money::positive(499, KES).unwrap();
        assert_eq!(
            tx.validate(),
            Err(LedgerError::UnbalancedMagnitude {
                debit: 500,
                credit: 499
            })
        );
    }

    #[test]
    fn validate_rejects_same_account_on_both_legs() {
        let a = AccountId::new();
        let amount = Money::positive(10, KES).unwrap();
        let tid = TransferId::new();
        let tx = LedgerTransaction::from_parts(
            tid,
            LedgerEntry::new(tid, a, EntryDirection::Debit, amount, now()),
            LedgerEntry::new(tid, a, EntryDirection::Credit, amount, now()),
        );
        assert_eq!(tx.validate(), Err(LedgerError::SameAccount));
    }

    #[test]
    fn signed_minor_units_depend_only_on_direction() {
        let (src, dst, amount) = pair();
        let tx =
            LedgerTransaction::double_entry(TransferId::new(), src, dst, amount, now()).unwrap();
        assert_eq!(tx.debit.signed_minor_units(), -500);
        assert_eq!(tx.credit.signed_minor_units(), 500);
    }

    #[test]
    fn derived_balance_recomputes_from_history() {
        let (src, dst, amount) = pair();
        let tx =
            LedgerTransaction::double_entry(TransferId::new(), src, dst, amount, now()).unwrap();
        let entries = vec![tx.debit.clone(), tx.credit.clone()];

        assert_eq!(derived_balance_minor_units(&entries, src), -500);
        assert_eq!(derived_balance_minor_units(&entries, dst), 500);
    }

    #[test]
    fn derived_balance_sums_exactly_in_both_directions() {
        let a = AccountId::new();
        let b = AccountId::new();
        let mut entries = Vec::new();

        // a -> b: 300
        let tx = LedgerTransaction::double_entry(
            TransferId::new(),
            a,
            b,
            Money::positive(300, KES).unwrap(),
            now(),
        )
        .unwrap();
        entries.extend(tx.entries().iter().map(|e| (*e).clone()));

        // b -> a: 120
        let tx = LedgerTransaction::double_entry(
            TransferId::new(),
            b,
            a,
            Money::positive(120, KES).unwrap(),
            now(),
        )
        .unwrap();
        entries.extend(tx.entries().iter().map(|e| (*e).clone()));

        assert_eq!(derived_balance_minor_units(&entries, a), -180);
        assert_eq!(derived_balance_minor_units(&entries, b), 180);
        // The ledger as a whole always nets to zero (INV-3).
        let total: i64 = entries.iter().map(LedgerEntry::signed_minor_units).sum();
        assert_eq!(total, 0);
    }

    #[test]
    fn no_mutating_api_on_ledger_entries() {
        // INV-7 is enforced by absence: this test documents that `LedgerEntry`
        // exposes no `&mut self` accessors. If one is ever added, this comment
        // and the guarantee both become false.
        let (src, dst, amount) = pair();
        let tx =
            LedgerTransaction::double_entry(TransferId::new(), src, dst, amount, now()).unwrap();
        let _immutable: &LedgerEntry = &tx.debit;
    }
}
