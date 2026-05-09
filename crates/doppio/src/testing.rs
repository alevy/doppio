//! Fluent test-fixture builders for [`elaboration::Journal`], [`elaboration::Transaction`],
//! and [`elaboration::Posting`].
//!
//! Constructing elaboration types by hand in tests requires wrestling with proto3 quirks:
//! `amount: Some(Amount { by_commodity: BTreeMap::from([(c, decimal_to_proto(d))]) })`,
//! `state: TransactionState::Cleared as i32`, epoch-days date encoding, etc. This module
//! hides all of that behind a minimal fluent API so test authors can stay focused on the
//! scenario rather than the wire format.
//!
//! # Feature flag
//!
//! This module is gated behind the `testing` cargo feature. Add it to your
//! `dev-dependencies`:
//!
//! ```toml
//! [dev-dependencies]
//! doppio = { version = "...", features = ["testing"] }
//! ```
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "testing")]
//! # fn example() {
//! use doppio::testing::{journal, txn, posting};
//! use chrono::NaiveDate;
//!
//! let j = journal()
//!     .with_txn(txn(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
//!         .with_posting(posting("Expenses:Food").with_amount(50, "$"))
//!         .with_posting(posting("Assets:Checking").with_amount(-50, "$")))
//!     .build();
//! assert_eq!(j.transactions.len(), 1);
//! assert_eq!(j.transactions[0].postings.len(), 2);
//! # }
//! ```

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::elaboration;

// ---
// Date conversion helper
// ---

fn naive_date_to_epoch_days(date: NaiveDate) -> i32 {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is a valid date");
    (date - epoch).num_days() as i32
}

// ---
// JournalBuilder
// ---

/// Fluent builder for [`elaboration::Journal`].
///
/// Create with [`journal()`] and add transactions with [`JournalBuilder::with_txn`].
///
/// # Example
///
/// ```
/// # #[cfg(feature = "testing")]
/// # fn example() {
/// use doppio::testing::{journal, txn, posting};
/// use chrono::NaiveDate;
///
/// let j = journal()
///     .with_txn(txn(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
///         .with_posting(posting("Expenses:Food").with_amount(50, "$"))
///         .with_posting(posting("Assets:Checking").with_amount(-50, "$")))
///     .build();
/// # }
/// ```
#[derive(Debug, Default)]
pub struct JournalBuilder {
    transactions: Vec<TransactionBuilder>,
}

impl JournalBuilder {
    /// Add a transaction to the journal being built.
    #[must_use]
    pub fn with_txn(mut self, txn: TransactionBuilder) -> Self {
        self.transactions.push(txn);
        self
    }

    /// Consume the builder and produce an [`elaboration::Journal`].
    #[must_use]
    pub fn build(self) -> elaboration::Journal {
        elaboration::Journal {
            transactions: self.transactions.into_iter().map(|t| t.build()).collect(),
            ..Default::default()
        }
    }
}

/// Create a new [`JournalBuilder`].
#[must_use]
pub fn journal() -> JournalBuilder {
    JournalBuilder::default()
}

// ---
// TransactionBuilder
// ---

/// Fluent builder for [`elaboration::Transaction`].
///
/// Create with [`txn()`] and chain methods to configure the transaction.
#[derive(Debug)]
pub struct TransactionBuilder {
    date: i32,
    description: String,
    state: i32,
    postings: Vec<PostingBuilder>,
}

impl TransactionBuilder {
    fn new(date: NaiveDate, description: &str) -> Self {
        Self {
            date: naive_date_to_epoch_days(date),
            description: description.to_owned(),
            state: 0,
            postings: Vec::new(),
        }
    }

    /// Add a posting to this transaction.
    #[must_use]
    pub fn with_posting(mut self, posting: PostingBuilder) -> Self {
        self.postings.push(posting);
        self
    }

    /// Set the transaction's cleared/pending/uncleared state.
    ///
    /// Accepts the [`elaboration::TransactionState`] enum and converts to the
    /// proto `i32` wire value internally.
    #[must_use]
    pub fn with_state(mut self, state: elaboration::TransactionState) -> Self {
        self.state = state as i32;
        self
    }

    /// Consume the builder and produce an [`elaboration::Transaction`].
    #[must_use]
    pub fn build(self) -> elaboration::Transaction {
        elaboration::Transaction {
            date: self.date,
            description: self.description,
            state: self.state,
            postings: self.postings.into_iter().map(|p| p.build()).collect(),
            ..Default::default()
        }
    }
}

/// Create a new [`TransactionBuilder`] with the given date and description (payee).
///
/// `date` is a [`chrono::NaiveDate`]; epoch-days encoding is handled internally.
#[must_use]
pub fn txn(date: NaiveDate, description: &str) -> TransactionBuilder {
    TransactionBuilder::new(date, description)
}

// ---
// PostingBuilder
// ---

/// Fluent builder for [`elaboration::Posting`].
///
/// Create with [`posting()`] and chain methods to set the amount and kind.
#[derive(Debug)]
pub struct PostingBuilder {
    account: String,
    amount: Option<elaboration::Amount>,
    kind: i32,
    lot: Option<elaboration::Lot>,
}

impl PostingBuilder {
    fn new(account: &str) -> Self {
        Self {
            account: account.to_owned(),
            amount: None,
            kind: 0,
            lot: None,
        }
    }

    /// Set this posting's amount.
    ///
    /// `value` is anything that converts to [`rust_decimal::Decimal`] (e.g. `i32`, `i64`,
    /// or a `Decimal` directly). `commodity` is the commodity symbol (e.g. `"$"`, `"USD"`).
    ///
    /// Replaces any previously set amount on this builder.
    #[must_use]
    pub fn with_amount<V: Into<Decimal>>(mut self, value: V, commodity: &str) -> Self {
        self.amount = Some(single_commodity_amount(value.into(), commodity));
        self
    }

    /// Set this posting's amount from a [`rust_decimal::Decimal`] directly.
    ///
    /// Equivalent to [`PostingBuilder::with_amount`] but accepts a `Decimal` without
    /// requiring a conversion impl. Useful when the caller already has a `Decimal` in hand.
    #[must_use]
    pub fn with_amount_decimal(mut self, value: Decimal, commodity: &str) -> Self {
        self.amount = Some(single_commodity_amount(value, commodity));
        self
    }

    /// Set the posting kind (REAL, VIRTUAL_BALANCED, VIRTUAL_UNBALANCED).
    ///
    /// Defaults to `PostingKind::Unspecified` (treated as REAL by all consumers).
    #[must_use]
    pub fn with_kind(mut self, kind: elaboration::PostingKind) -> Self {
        self.kind = kind as i32;
        self
    }

    /// Attach a lot annotation to this posting.
    ///
    /// Use the [`lot()`] builder to construct the [`LotBuilder`] argument.
    #[must_use]
    pub fn with_lot(mut self, lot: LotBuilder) -> Self {
        self.lot = Some(lot.build());
        self
    }

    /// Consume the builder and produce an [`elaboration::Posting`].
    #[must_use]
    pub fn build(self) -> elaboration::Posting {
        elaboration::Posting {
            account: self.account,
            amount: self.amount,
            kind: self.kind,
            lot: self.lot,
            ..Default::default()
        }
    }
}

/// Create a new [`PostingBuilder`] for the given account name.
#[must_use]
pub fn posting(account: &str) -> PostingBuilder {
    PostingBuilder::new(account)
}

// ---
// LotBuilder
// ---

/// Fluent builder for [`elaboration::Lot`].
///
/// Create with [`lot()`] and chain methods to set cost, date, and note.
#[derive(Debug, Default)]
pub struct LotBuilder {
    cost: Option<elaboration::Amount>,
    date: Option<i32>,
    note: Option<String>,
}

impl LotBuilder {
    /// Set the per-unit cost basis for this lot.
    #[must_use]
    pub fn with_cost<V: Into<Decimal>>(mut self, value: V, commodity: &str) -> Self {
        self.cost = Some(single_commodity_amount(value.into(), commodity));
        self
    }

    /// Set the lot's acquisition date.
    #[must_use]
    pub fn with_date(mut self, date: NaiveDate) -> Self {
        self.date = Some(naive_date_to_epoch_days(date));
        self
    }

    /// Set the lot's free-form note.
    #[must_use]
    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.to_owned());
        self
    }

    /// Consume the builder and produce an [`elaboration::Lot`].
    #[must_use]
    pub fn build(self) -> elaboration::Lot {
        elaboration::Lot {
            cost: self.cost,
            date: self.date,
            note: self.note,
        }
    }
}

/// Create a new [`LotBuilder`].
#[must_use]
pub fn lot() -> LotBuilder {
    LotBuilder::default()
}

// ---
// Internal helpers
// ---

fn single_commodity_amount(value: Decimal, commodity: &str) -> elaboration::Amount {
    elaboration::Amount {
        by_commodity: BTreeMap::from([(commodity.to_owned(), crate::decimal_to_proto(value))]),
    }
}

// ---
// Tests
// ---

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    use crate::elaboration;

    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn test_minimal_journal() {
        let j = journal()
            .with_txn(
                txn(date(2024, 1, 15), "Groceries")
                    .with_posting(posting("Expenses:Food").with_amount(50, "$"))
                    .with_posting(posting("Assets:Checking").with_amount(-50, "$")),
            )
            .build();

        assert_eq!(j.transactions.len(), 1);
        let txn = &j.transactions[0];
        assert_eq!(txn.description, "Groceries");
        assert_eq!(txn.postings.len(), 2);

        let food = &txn.postings[0];
        assert_eq!(food.account, "Expenses:Food");
        assert_eq!(food.amount_in("$"), Some(Decimal::from(50)));

        let checking = &txn.postings[1];
        assert_eq!(checking.account, "Assets:Checking");
        assert_eq!(checking.amount_in("$"), Some(Decimal::from(-50)));
    }

    #[test]
    fn test_with_state_cleared() {
        let j = journal()
            .with_txn(
                txn(date(2024, 3, 1), "Salary")
                    .with_state(elaboration::TransactionState::Cleared)
                    .with_posting(
                        posting("Income:Salary").with_amount(Decimal::new(500000, 2), "$"),
                    )
                    .with_posting(posting("Assets:Checking").with_amount(-5000, "$")),
            )
            .build();

        let t = &j.transactions[0];
        assert_eq!(t.state, elaboration::TransactionState::Cleared as i32);
        assert_eq!(t.description, "Salary");
    }

    #[test]
    fn test_with_lot_annotation() {
        let j = journal()
            .with_txn(
                txn(date(2024, 3, 15), "Buy AAPL")
                    .with_posting(
                        posting("Assets:Brokerage")
                            .with_amount(10, "AAPL")
                            .with_lot(
                                lot()
                                    .with_cost(150, "$")
                                    .with_date(date(2024, 3, 15))
                                    .with_note("BUY-2024"),
                            ),
                    )
                    .with_posting(posting("Assets:Cash").with_amount(-1500, "$")),
            )
            .build();

        let t = &j.transactions[0];
        let brokerage = &t.postings[0];
        assert_eq!(brokerage.account, "Assets:Brokerage");
        assert!(brokerage.has_lot(), "posting should carry a lot annotation");
        assert_eq!(brokerage.lot_cost_in("$"), Some(Decimal::from(150)));
        assert_eq!(brokerage.lot_date_naive(), Some(date(2024, 3, 15)));
        assert_eq!(brokerage.lot_note(), Some("BUY-2024"));
    }

    #[test]
    fn test_amount_decimal_conversion() {
        // Verify that decimal_to_proto is correctly applied: a Decimal round-trips through
        // the Amount encoding without losing precision.
        let value = Decimal::new(12345, 2); // 123.45
        let j = journal()
            .with_txn(
                txn(date(2024, 6, 1), "Precision test")
                    .with_posting(posting("Expenses:Test").with_amount_decimal(value, "USD"))
                    .with_posting(posting("Assets:Cash").with_amount_decimal(-value, "USD")),
            )
            .build();

        let p = &j.transactions[0].postings[0];
        assert_eq!(
            p.amount_in("USD"),
            Some(value),
            "decimal round-trip through proto encoding should preserve exact value"
        );
    }

    #[test]
    fn test_posting_with_kind() {
        let j = journal()
            .with_txn(
                txn(date(2024, 1, 1), "Virtual")
                    .with_posting(
                        posting("(Budget:Food)")
                            .with_amount(50, "$")
                            .with_kind(elaboration::PostingKind::VirtualUnbalanced),
                    )
                    .with_posting(posting("Expenses:Food").with_amount(50, "$"))
                    .with_posting(posting("Assets:Checking").with_amount(-50, "$")),
            )
            .build();

        let virtual_p = &j.transactions[0].postings[0];
        assert_eq!(
            virtual_p.posting_kind(),
            elaboration::PostingKind::VirtualUnbalanced
        );
        assert!(!virtual_p.is_real());
    }

    #[test]
    fn test_date_encoding() {
        // 2024-01-15 is 19737 days after 1970-01-01.
        let j = journal()
            .with_txn(
                txn(date(2024, 1, 15), "Check date")
                    .with_posting(posting("Expenses:Test").with_amount(1, "$"))
                    .with_posting(posting("Assets:Cash").with_amount(-1, "$")),
            )
            .build();

        let t = &j.transactions[0];
        assert_eq!(
            t.date_naive(),
            date(2024, 1, 15),
            "epoch-days encoding must round-trip correctly"
        );
    }

    #[test]
    fn test_empty_journal() {
        let j = journal().build();
        assert!(j.transactions.is_empty());
    }

    #[test]
    fn test_multiple_transactions() {
        let j = journal()
            .with_txn(
                txn(date(2024, 1, 1), "First")
                    .with_posting(posting("Expenses:A").with_amount(10, "$"))
                    .with_posting(posting("Assets:Cash").with_amount(-10, "$")),
            )
            .with_txn(
                txn(date(2024, 1, 2), "Second")
                    .with_posting(posting("Expenses:B").with_amount(20, "$"))
                    .with_posting(posting("Assets:Cash").with_amount(-20, "$")),
            )
            .build();

        assert_eq!(j.transactions.len(), 2);
        assert_eq!(j.transactions[0].description, "First");
        assert_eq!(j.transactions[1].description, "Second");
    }
}
