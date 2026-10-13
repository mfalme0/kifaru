//! Exact monetary values.
//!
//! # Representation
//!
//! A [`Money`] is a signed `i64` count of **minor units** (cents for KES) tagged
//! with a [`Currency`]. There is no floating point anywhere in this type, and
//! there is no rounding step in the transfer path — both ledger legs carry the
//! identical `i64`, so there is nothing to round.
//!
//! # Why `i64` and not a decimal type
//!
//! Exact by construction, maps 1:1 to PostgreSQL `BIGINT`, and every operation
//! is a single checked integer op. `rust_decimal` was considered and rejected:
//! 96-bit, more expensive per operation, and it buys multi-currency scale
//! generality that a single-configured-currency MVP does not need. The cost of
//! this choice is recorded in the ADR: adding a currency with a different
//! minor-unit scale requires a scale-aware `Money`.
//!
//! # Non-negativity
//!
//! `Money` cannot represent a negative value. This is deliberate: it makes
//! INV-1 ("no account balance is negative under the default overdraft policy")
//! difficult to violate accidentally, because the type system rejects it before
//! any database interaction happens. Subtraction that would go negative returns
//! [`MoneyError::WouldGoNegative`] rather than silently wrapping.

use crate::error::MoneyError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The currencies KIFARU can represent.
///
/// Exactly one currency is *configured* for a running instance. The type is an
/// enum rather than a `String` so that adding a currency is a deliberate,
/// compiler-checked change with an explicit minor-unit scale, instead of a
/// loose string arriving at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Currency {
    /// Kenyan Shilling. Minor unit: cent, scale 2 (100 cents = 1 shilling).
    Kes,
}

impl Currency {
    /// Decimal places in this currency's minor unit.
    ///
    /// This is the *scale*: `KES 1.00` is `100` minor units.
    pub const fn minor_unit_scale(self) -> u32 {
        match self {
            Currency::Kes => 2,
        }
    }

    /// ISO 4217 alphabetic code.
    pub const fn code(self) -> &'static str {
        match self {
            Currency::Kes => "KES",
        }
    }

    /// The currency a running instance is configured to use.
    ///
    /// Phase 1 supports exactly one configured currency. When configuration
    /// becomes multi-valued this function is replaced by a real config lookup.
    pub const fn configured() -> Self {
        Currency::Kes
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// An exact non-negative monetary value.
///
/// Construction refuses negative values. All arithmetic is checked: overflow and
/// negative results are returned as errors, never wrapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Money {
    minor_units: i64,
    currency: Currency,
}

impl Money {
    /// Zero in the given currency.
    pub const fn zero(currency: Currency) -> Self {
        Self {
            minor_units: 0,
            currency,
        }
    }

    /// Zero in the configured currency.
    pub const fn zero_configured() -> Self {
        Self::zero(Currency::configured())
    }

    /// Construct from minor units.
    ///
    /// Negative minor units are rejected: a `Money` is never negative (INV-1).
    pub const fn from_minor_units(
        minor_units: i64,
        currency: Currency,
    ) -> Result<Self, MoneyError> {
        if minor_units < 0 {
            return Err(MoneyError::Negative { minor_units });
        }
        Ok(Self {
            minor_units,
            currency,
        })
    }

    /// Construct a strictly positive amount, as required for a transfer leg.
    pub const fn positive(minor_units: i64, currency: Currency) -> Result<Self, MoneyError> {
        if minor_units <= 0 {
            return Err(MoneyError::NotPositive { minor_units });
        }
        Ok(Self {
            minor_units,
            currency,
        })
    }

    /// The raw minor-unit count. Always `>= 0`.
    pub const fn minor_units(self) -> i64 {
        self.minor_units
    }

    pub const fn currency(self) -> Currency {
        self.currency
    }

    pub const fn is_zero(self) -> bool {
        self.minor_units == 0
    }

    pub const fn is_positive(self) -> bool {
        self.minor_units > 0
    }

    /// Add two values of the same currency, failing on overflow.
    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        self.ensure_same_currency(other.currency)?;
        let sum = self
            .minor_units
            .checked_add(other.minor_units)
            .ok_or(MoneyError::Overflow)?;
        // Both operands are non-negative, so the sum cannot be negative.
        Ok(Self {
            minor_units: sum,
            currency: self.currency,
        })
    }

    /// Subtract `other` from `self`, failing if the result would be negative.
    ///
    /// A negative result is reported as [`MoneyError::WouldGoNegative`] rather
    /// than as an overflow, so callers can distinguish "you cannot afford this"
    /// from "this number does not fit".
    pub fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
        self.ensure_same_currency(other.currency)?;
        let diff = self
            .minor_units
            .checked_sub(other.minor_units)
            .ok_or(MoneyError::Overflow)?;
        if diff < 0 {
            return Err(MoneyError::WouldGoNegative);
        }
        Ok(Self {
            minor_units: diff,
            currency: self.currency,
        })
    }

    /// Fail unless both values share a currency.
    pub const fn ensure_same_currency(self, other: Currency) -> Result<(), MoneyError> {
        if !matches!((self.currency, other), (Currency::Kes, Currency::Kes)) {
            return Err(MoneyError::CurrencyMismatch {
                expected: self.currency,
                found: other,
            });
        }
        Ok(())
    }
}

impl fmt::Display for Money {
    /// Renders in major units using the currency's declared scale.
    ///
    /// This is a *presentation* concern. It is never used in the transfer path
    /// and never fed back into arithmetic.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scale = self.currency.minor_unit_scale();
        if scale == 0 {
            return write!(f, "{} {}", self.minor_units, self.currency);
        }
        let divisor = 10i64.pow(scale);
        let major = self.minor_units / divisor;
        let minor = self.minor_units % divisor;
        write!(
            f,
            "{}.{:0width$} {}",
            major,
            minor,
            self.currency,
            width = scale as usize
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KES: Currency = Currency::Kes;

    #[test]
    fn negative_minor_units_are_rejected() {
        assert_eq!(
            Money::from_minor_units(-1, KES),
            Err(MoneyError::Negative { minor_units: -1 })
        );
    }

    #[test]
    fn zero_is_a_valid_money_but_not_a_valid_transfer_amount() {
        assert!(Money::from_minor_units(0, KES).is_ok());
        assert_eq!(
            Money::positive(0, KES),
            Err(MoneyError::NotPositive { minor_units: 0 })
        );
        assert_eq!(
            Money::positive(-5, KES),
            Err(MoneyError::NotPositive { minor_units: -5 })
        );
    }

    #[test]
    fn addition_is_exact_and_does_not_overflow() {
        let a = Money::from_minor_units(150, KES).unwrap();
        let b = Money::from_minor_units(275, KES).unwrap();
        assert_eq!(a.checked_add(b).unwrap().minor_units(), 425);

        let max = Money::from_minor_units(i64::MAX, KES).unwrap();
        let one = Money::from_minor_units(1, KES).unwrap();
        assert_eq!(max.checked_add(one), Err(MoneyError::Overflow));
    }

    #[test]
    fn subtraction_that_would_go_negative_is_rejected_not_wrapped() {
        let balance = Money::from_minor_units(100, KES).unwrap();
        let amount = Money::from_minor_units(250, KES).unwrap();
        assert_eq!(
            balance.checked_sub(amount),
            Err(MoneyError::WouldGoNegative)
        );
    }

    #[test]
    fn subtraction_to_exactly_zero_is_allowed() {
        let balance = Money::from_minor_units(100, KES).unwrap();
        assert!(balance.checked_sub(balance).unwrap().is_zero());
    }

    #[test]
    fn currency_mismatch_is_detected() {
        // Only one currency exists today, so this asserts the guard's shape
        // rather than exercising a mismatch. Kept so the check is not silently
        // removed when a second currency is added.
        let a = Money::from_minor_units(100, KES).unwrap();
        assert!(a.ensure_same_currency(KES).is_ok());
    }

    #[test]
    fn display_uses_declared_scale() {
        let m = Money::from_minor_units(12345, KES).unwrap();
        assert_eq!(m.to_string(), "123.45 KES");
        let m = Money::from_minor_units(5, KES).unwrap();
        assert_eq!(m.to_string(), "0.05 KES");
    }
}
