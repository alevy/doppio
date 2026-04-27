//! Abstract Syntax Tree (AST) for the Ledger file format.
//!
//! This module defines the data types produced by the parser. The AST is a
//! faithful, low-level representation of the source text: dates may lack a
//! year, amounts are unevaluated [`ValueExpr`] trees, and directives are kept
//! in their raw form. Subsequent pipeline stages ([`crate::resolution`] and
//! [`crate::elaboration`]) refine these into higher-level structures.

use std::{collections::BTreeMap, fmt::Display};

use chrono::{Datelike, NaiveDate};
use pest::Parser as PestParser;
use rust_decimal::Decimal;

use crate::parser::{self, LedgerParser};

/// The root of a parsed ledger file.
///
/// Contains all top-level entries in source order.
#[derive(Debug)]
pub struct Journal {
    pub entries: Vec<Entry>,
}

/// A single top-level entry in the journal.
#[derive(Debug)]
pub enum Entry {
    /// A double-entry accounting transaction (date, description, postings).
    Transaction(Transaction),
    /// A configuration directive (`commodity`, `account`, `alias`, …).
    Directive(Directive),
    /// A `P` price directive recording the market price of a commodity.
    HistoricalPrice(HistoricalPrice),
    /// A standalone balance assertion directive.
    ///
    /// Example: `2024-01-15 = Assets:Checking  $1000.00`
    Assertion(AssertionDirective),
    /// A comment line starting with `;`, `#`, `*`, `%`, or `|`.
    Comment(String),
}

/// A standalone balance assertion directive outside of any transaction.
///
/// These directives assert that an account's balance equals a given amount
/// at a specific date. The assertion is not enforced by the resolution stage;
/// enforcement is handled by the elaboration stage (see issue #37).
///
/// Example: `2024-01-15 == Assets:Checking  $1000.00`
#[derive(Debug, Clone)]
pub struct AssertionDirective {
    /// The date at which the balance assertion applies.
    pub date: Date,
    /// The account whose balance is being asserted.
    pub account: String,
    /// The expected balance expressed as a value expression.
    pub amount: ValueExpr,
    /// `true` if `==` (strict equality), `false` if `=` (weak/approximate).
    pub strict: bool,
}

/// A `P` price directive: the market price of one unit of `commodity` in
/// terms of the `price` expression at the given `date`.
///
/// Example: `P 2024-01-15 14:30:00 AAPL $182.50`
#[derive(Debug, Clone)]
pub struct HistoricalPrice {
    /// The date on which this price was recorded.
    pub date: Date,
    /// Optional wall-clock time of the price quote (`HH:MM` or `HH:MM:SS`).
    pub time: Option<String>,
    /// The commodity whose price is being recorded (e.g. `"AAPL"`, `"BTC"`).
    pub commodity: String,
    /// The price of one unit of `commodity` as a value expression.
    pub price: ValueExpr,
}

/// A configuration directive parsed from the ledger source.
#[derive(Debug)]
pub enum Directive {
    /// A `commodity` block declaring properties of a commodity symbol.
    Commodity {
        /// The canonical commodity name (e.g. `"USD"`, `"BTC"`, `"$"`).
        name: String,
        /// Free-form notes attached to the commodity block header.
        notes: Vec<String>,
        /// Structured sub-items (`alias`, `format`, `nomarket`, `default`, …).
        items: Vec<CommodityItem>,
    },
    /// An `account` block declaring properties of an account.
    Account {
        /// The canonical account name (e.g. `"Assets:Bank:Checking"`).
        name: String,
        /// Free-form notes attached to the account block header.
        notes: Vec<String>,
        /// Structured sub-items (`alias`, `note`, …).
        items: Vec<AccountItem>,
    },
    /// A directive whose keyword was not recognised.
    Unknown(String),
    /// A top-level `alias <from> = <to>` account shorthand.
    Alias {
        /// The short name to be used in postings.
        alias: String,
        /// The full account name it expands to.
        account: String,
    },
    /// A `define name = expr` named value alias.
    ///
    /// After this directive, any occurrence of `name` in a value expression
    /// is substituted with the stored `expr` during elaboration.
    Define {
        /// The alias name (a plain identifier).
        name: String,
        /// The value expression the name expands to.
        expr: ValueExpr,
    },
}

/// A single key/value item inside a `commodity` directive block.
#[derive(Clone, Debug)]
pub enum CommodityItem {
    /// An alternative name for this commodity (`alias <name>`).
    Alias(String),
    /// A display format string, e.g. `"1,000.00 USD"`.
    Format(String),
    /// Marks the commodity as having no market price data (`nomarket`).
    NoMarket,
    /// Marks this commodity as the default when no commodity is specified.
    Default,
    /// A free-form note describing the commodity.
    Note(String),
    /// An unrecognised sub-directive with an optional value.
    Unknown(String, Option<String>),
}

/// A single key/value item inside an `account` directive block.
#[derive(Clone, Debug)]
pub enum AccountItem {
    /// An alternative name for this account (`alias <name>`).
    Alias(String),
    /// A free-form note describing the account.
    Note(String),
    /// A fatal assertion that every posting to this account must satisfy.
    ///
    /// Elaboration halts with [`crate::elaboration::ElaborationError::AccountAssertionFailed`]
    /// if the expression evaluates to `false` for any posting.
    Assert(BoolExpr),
    /// A non-fatal check that every posting to this account should satisfy.
    ///
    /// If the expression evaluates to `false`, a warning is printed to stderr
    /// but elaboration continues.
    Check(BoolExpr),
    /// An unrecognised sub-directive with an optional value.
    Unknown(String, Option<String>),
}

/// A boolean expression used in `assert` / `check` account sub-directives.
///
/// The grammar supports a simple left-to-right structure:
/// `lhs [cmp_op rhs] [bool_op continuation]`.
/// Full precedence parsing of boolean operators is a TODO(#74-followup).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoolExpr {
    /// Left-hand side value expression (e.g. `commodity`, `amount`).
    pub lhs: ValueExpr,
    /// Optional comparison: operator and right-hand side.
    pub cmp: Option<(CmpOp, ValueExpr)>,
    /// Optional logical continuation chained to the right.
    pub chain: Option<(BoolOp, Box<BoolExpr>)>,
}

/// Comparison operator used in [`BoolExpr`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `=~` — LHS string matches the regex RHS.
    RegexMatch,
    /// `!~` — LHS string does not match the regex RHS.
    RegexNotMatch,
}

impl std::fmt::Display for CmpOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CmpOp::Eq => write!(f, "=="),
            CmpOp::Ne => write!(f, "!="),
            CmpOp::Lt => write!(f, "<"),
            CmpOp::Le => write!(f, "<="),
            CmpOp::Gt => write!(f, ">"),
            CmpOp::Ge => write!(f, ">="),
            CmpOp::RegexMatch => write!(f, "=~"),
            CmpOp::RegexNotMatch => write!(f, "!~"),
        }
    }
}

/// Boolean chaining operator used in [`BoolExpr`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoolOp {
    And,
    Or,
}

impl std::fmt::Display for BoolOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoolOp::And => write!(f, "and"),
            BoolOp::Or => write!(f, "or"),
        }
    }
}

impl std::fmt::Display for BoolExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.lhs)?;
        if let Some((op, rhs)) = &self.cmp {
            write!(f, " {op} {rhs}")?;
        }
        if let Some((op, cont)) = &self.chain {
            write!(f, " {op} {cont}")?;
        }
        Ok(())
    }
}

/// A parsed date, with an optional year.
///
/// Ledger's grammar always requires a four-digit year, but this struct
/// keeps it `Option<i32>` so the parser can represent partially-specified
/// dates uniformly. The [`crate::resolution`] stage rejects dates where
/// no year can be determined.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Default, Debug)]
pub struct Date {
    /// The calendar year, e.g. `2024`. `None` if not present in the source.
    pub year: Option<i32>,
    /// Month of the year (1–12).
    pub month: u32,
    /// Day of the month (1–31).
    pub date: u32,
}

impl From<NaiveDate> for Date {
    fn from(value: NaiveDate) -> Self {
        Self {
            year: Some(value.year()),
            month: value.month0() + 1,
            date: value.day0() + 1,
        }
    }
}

/// A double-entry accounting transaction as it appears in the source.
///
/// Amounts in the postings are unevaluated [`ValueExpr`] trees; the
/// [`crate::elaboration`] stage evaluates them and ensures the transaction
/// balances.
///
/// This is the raw parse-tree representation. For programmatic transaction
/// construction, use [`crate::resolution::Transaction`] and its builder methods instead.
#[derive(Clone, Default, Debug)]
pub struct Transaction {
    /// The primary (effective) date.
    pub date: Date,
    /// An optional secondary date (`date=secondary_date`), used for the
    /// "date the transaction was actually processed" semantics.
    pub secondary_date: Option<Date>,
    /// Cleared (`*`), pending (`!`), or uncleared (no marker).
    pub state: TransactionState,
    /// An optional reference code in parentheses, e.g. `(INV-42)`.
    pub code: Option<String>,
    /// The payee / description text following the header date and state.
    pub description: String,
    /// Lines starting with `;` in the transaction header (before postings).
    pub notes: Vec<String>,
    /// The individual account postings that make up this transaction.
    pub postings: Vec<Posting>,
}

impl Display for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(year) = self.date.year {
            write!(f, "{year:04}-")?;
        }
        write!(f, "{:02}-{:02}", self.date.month, self.date.date)?;

        if let Some(ref date) = self.secondary_date {
            write!(f, "=")?;
            if let Some(year) = date.year {
                write!(f, "{year:04}-")?;
            }
            write!(f, "{:02}-{:02}", date.month, date.date)?;
        }

        match self.state {
            TransactionState::Uncleared => {}
            TransactionState::Pending => write!(f, " !")?,
            TransactionState::Cleared => write!(f, " *")?,
        }

        if let Some(ref code) = self.code {
            write!(f, " ({code})")?;
        }

        writeln!(f, " {}", self.description)?;

        for note in self.notes.iter() {
            writeln!(f, "    ; {note}")?;
        }

        for posting in self.postings.iter() {
            posting.fmt(f)?;
        }

        Ok(())
    }
}

/// A single posting (debit or credit line) within a transaction.
///
/// This is the raw parse-tree representation. For programmatic posting
/// construction, use [`crate::resolution::Posting`] and its builder methods instead.
#[derive(Clone, Default, Debug)]
pub struct Posting {
    /// The account name, e.g. `"Expenses:Food"`.
    pub account: String,
    /// The amount, or `None` if this is a "null posting" whose value should
    /// be inferred by the elaboration stage as the negation of all others.
    pub amount: Option<AmountDetails>,
    /// Per-posting cleared/pending state (overrides the transaction state).
    pub state: TransactionState,
    /// Lines starting with `;` indented beneath the posting line.
    pub notes: Vec<String>,
}

impl Posting {
    /// Creates a new posting for `account` with no amount and no notes.
    pub fn new<S: Into<String>>(account: S) -> Self {
        Self {
            account: account.into(),
            amount: None,
            state: TransactionState::Uncleared,
            notes: vec![],
        }
    }

    /// Appends a note to this posting (builder pattern).
    pub fn with_note<S: Into<String>>(mut self, note: S) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Sets the amount for this posting (builder pattern).
    pub fn with_amount<A: Into<AmountDetails>>(mut self, amount: A) -> Self {
        self.amount = Some(amount.into());
        self
    }
}

impl Display for Posting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "    ")?;
        match self.state {
            TransactionState::Uncleared => {}
            TransactionState::Pending => write!(f, "! ")?,
            TransactionState::Cleared => write!(f, "* ")?,
        }

        write!(f, "{}", self.account)?;

        if let Some(ref amount) = self.amount {
            write!(f, "  {amount}")?;
        }
        writeln!(f)?;

        for note in self.notes.iter() {
            writeln!(f, "    ; {note}")?;
        }
        Ok(())
    }
}

/// The amount field of a posting, in one of two forms.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum AmountDetails {
    /// A standard amount expression, with optional lot pricing and/or a
    /// balance assertion.
    ///
    /// Example: `10 AAPL @ $150 = $1500`
    Amount {
        /// The value expression (may be arithmetic, e.g. `(100 + 50) USD`).
        value: ValueExpr,
        /// Cost basis: `@ price_per_unit` or `@@ total_cost`.
        lot_pricing: Option<LotPricing>,
        /// If present, the account balance after this posting must equal this
        /// value (`= expected_balance`).
        balance_assertion: Option<ValueExpr>,
    },
    /// A balance assignment: `= target_balance`, with no explicit debit/credit
    /// value. The posting amount is computed as `target - current_balance`.
    BalanceAssignment(ValueExpr),
}

impl<I: Into<ValueExpr>> From<I> for AmountDetails {
    fn from(value: I) -> Self {
        AmountDetails::Amount {
            value: value.into(),
            lot_pricing: None,
            balance_assertion: None,
        }
    }
}

impl Display for AmountDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AmountDetails::Amount {
                value,
                lot_pricing,
                balance_assertion,
            } => {
                write!(f, "{value}")?;
                if let Some(lot_pricing) = lot_pricing {
                    match lot_pricing {
                        LotPricing::Unit(value_expr) => write!(f, " @ {value_expr}")?,
                        LotPricing::Total(value_expr) => write!(f, " @@ {value_expr}")?,
                    }
                }
                if let Some(balance_assertion) = balance_assertion {
                    write!(f, " = {balance_assertion}")?;
                }
                Ok(())
            }
            AmountDetails::BalanceAssignment(value) => {
                write!(f, "={value}")
            }
        }
    }
}

impl Display for ValueExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueExpr::Amount { value, commodity } => {
                write!(f, "{value}")?;
                if let Some(commodity) = commodity {
                    write!(f, " {commodity}")?;
                }
                Ok(())
            }
            ValueExpr::Str(s) => write!(f, "\"{s}\""),
            ValueExpr::Regex(pattern) => write!(f, "/{pattern}/"),
            ValueExpr::Unary { op, expr } => {
                let op = match op {
                    Op::Add => "+",
                    Op::Sub => "-",
                    Op::Mul => "*",
                    Op::Div => "/",
                };
                write!(f, "{op}{expr}")
            }
            ValueExpr::Binary { lhs, rhs, op } => {
                let op = match op {
                    Op::Add => "+",
                    Op::Sub => "-",
                    Op::Mul => "*",
                    Op::Div => "/",
                };
                write!(f, "{lhs} {op} {rhs}")
            }
            ValueExpr::Function { name, args } => {
                write!(f, "{name}(")?;
                let mut args = args.iter();
                if let Some(a) = args.next() {
                    write!(f, "{a}")?;
                }
                for a in args {
                    write!(f, ", {a}")?;
                }
                write!(f, ")")
            }
            ValueExpr::Commodity(c) => write!(f, "{c}"),
            ValueExpr::Typed { expr, commodity } => write!(f, "{expr} {commodity}"),
            ValueExpr::Access { expr, field } => write!(f, "{expr}.{field}"),
            ValueExpr::Object(_) => todo!(),
        }
    }
}

/// An unevaluated value expression, produced by the parser and consumed by
/// the elaboration stage.
///
/// The variants cover all syntactic forms that can appear in a posting amount:
/// numeric literals, string literals, arithmetic, function calls, and
/// commodity type annotations.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ValueExpr {
    /// A key-value object, used as the return type of the `account()` function.
    /// Fields are accessed with the [`ValueExpr::Access`] form.
    Object(BTreeMap<String, Self>),

    /// A numeric amount with an optional commodity.
    ///
    /// `commodity: None` means the commodity is not yet known; the elaboration
    /// stage fills it in from the context's default commodity.
    Amount {
        value: Decimal,
        commodity: Option<String>,
    },

    /// A prefix unary operator applied to a sub-expression.
    ///
    /// Only `+` and `-` are meaningful on amounts; `*` and `/` produce an
    /// [`EvaluationError`](crate::elaboration::EvaluationError).
    Unary { op: Op, expr: Box<ValueExpr> },

    /// An infix binary operator applied to two sub-expressions.
    Binary {
        lhs: Box<ValueExpr>,
        rhs: Box<ValueExpr>,
        op: Op,
    },

    /// A function call, e.g. `account("Assets:Bank")` or `scrub(100 USD)`.
    Function { name: String, args: Vec<ValueExpr> },

    /// A bare commodity symbol that appears without an adjacent number,
    /// e.g. in `$-123` the `$` is parsed as a `Commodity` and `123` as
    /// `Amount { value: 123, commodity: None }`, with `-` as a `Binary { op: Sub }`
    /// between them. This special form is resolved in the evaluator.
    Commodity(String),

    /// A type annotation wrapping a parenthesised expression: `(expr) USD`.
    /// The elaboration stage verifies that the inner expression's commodity
    /// is compatible with the annotation.
    Typed {
        expr: Box<ValueExpr>,
        commodity: String,
    },

    /// A string literal (double-quoted).
    Str(String),

    /// A regex literal: `/pattern/`. Carries the raw pattern string between
    /// the delimiters (backslash-escapes are preserved as written). The regex
    /// is compiled on first use by the evaluator; it is never compiled here
    /// at parse time, keeping the AST independent of the `regex` crate.
    Regex(String),

    /// Field access on an object expression: `account("Foo").total`.
    Access { expr: Box<ValueExpr>, field: String },
}

impl ValueExpr {
    /// Convenience constructor for an amount with a known commodity.
    pub fn amount(value: Decimal, commodity: String) -> ValueExpr {
        ValueExpr::Amount {
            value,
            commodity: Some(commodity),
        }
    }

    /// Parse a Ledger value expression from a string.
    ///
    /// Useful for testing or standalone expression evaluation without
    /// going through the full journal parser.
    ///
    /// # Example
    /// ```
    /// # use doppio::ast::ValueExpr;
    /// let expr = ValueExpr::parse("100 USD").unwrap();
    /// ```
    pub fn parse(input: &str) -> Result<ValueExpr, pest::error::Error<parser::Rule>> {
        let mut pairs = LedgerParser::parse(parser::Rule::value_expr, input)?;
        let pair = pairs.next().unwrap();
        Ok(parser::parse_expr(pair))
    }
}

impl<S: Into<String>> From<(Decimal, S)> for ValueExpr {
    fn from(value: (Decimal, S)) -> Self {
        Self::amount(value.0, value.1.into())
    }
}

/// Arithmetic operator used in [`ValueExpr::Binary`] and [`ValueExpr::Unary`].
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

/// The cost-basis annotation on a posting amount.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum LotPricing {
    /// Per-unit price: `@ price`. The total cost is `amount * price`.
    Unit(ValueExpr),
    /// Total price: `@@ total`. The total cost is taken as-is.
    Total(ValueExpr),
}

/// Cleared/pending state of a transaction or individual posting.
#[derive(Clone, Debug, Default)]
pub enum TransactionState {
    /// No state marker — the transaction has not been reviewed.
    #[default]
    Uncleared,
    /// `!` — the transaction is pending confirmation.
    Pending,
    /// `*` — the transaction has been confirmed/reconciled.
    Cleared,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[test]
    fn test_date_ordering() {
        let d1 = Date {
            year: Some(2024),
            month: 1,
            date: 1,
        };
        let d2 = Date {
            year: Some(2024),
            month: 6,
            date: 15,
        };
        let d3 = Date {
            year: Some(2025),
            month: 1,
            date: 1,
        };
        let d4 = Date {
            year: Some(2024),
            month: 1,
            date: 1,
        };

        assert!(d1 < d2);
        assert!(d2 < d3);
        assert!(d1 < d3);
        assert_eq!(d1, d4);
        assert!(d3 > d1);

        let mut dates = vec![d3.clone(), d1.clone(), d2.clone()];
        dates.sort();
        assert_eq!(dates, vec![d1, d2, d3]);
    }

    #[test]
    fn test_transaction_display_indentation() {
        let mut tx = Transaction::default();
        tx.date = Date {
            year: Some(2024),
            month: 1,
            date: 15,
        };
        tx.description = "Test payee".to_string();
        tx.notes = vec!["a note".to_string()];
        let mut posting = Posting::new("Assets:Bank");
        posting.amount = Some(AmountDetails::Amount {
            value: ValueExpr::Amount {
                value: Decimal::from(100),
                commodity: Some("USD".into()),
            },
            lot_pricing: None,
            balance_assertion: None,
        });
        tx.postings = vec![posting];

        let out = format!("{tx}");
        // Notes use 4-space indent
        assert!(
            out.contains("    ; a note"),
            "note should have 4-space indent, got:\n{out}"
        );
        // Posting line uses 4-space indent
        assert!(
            out.contains("    Assets:Bank"),
            "posting should have 4-space indent, got:\n{out}"
        );
    }

    #[test]
    fn test_value_expr_parse_amount() {
        let expr = ValueExpr::parse("100 USD").unwrap();
        assert_eq!(
            expr,
            ValueExpr::Amount {
                value: "100".parse().unwrap(),
                commodity: Some("USD".into()),
            }
        );
    }

    #[test]
    fn test_value_expr_parse_prefixed_commodity() {
        let expr = ValueExpr::parse("$50").unwrap();
        assert_eq!(
            expr,
            ValueExpr::Amount {
                value: "50".parse().unwrap(),
                commodity: Some("$".into()),
            }
        );
    }

    #[test]
    fn test_value_expr_parse_arithmetic() {
        // Should parse without error and produce a Binary node
        let expr = ValueExpr::parse("10 + 5 USD").unwrap();
        assert!(
            matches!(expr, ValueExpr::Binary { .. } | ValueExpr::Typed { .. }),
            "expected Binary or Typed, got {expr:?}"
        );
    }

    #[test]
    fn test_value_expr_parse_error() {
        assert!(ValueExpr::parse("@@@invalid").is_err());
    }
}
