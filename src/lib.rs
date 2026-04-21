//! ledger-rs — a compiler and query library for the Ledger plain-text
//! accounting format.
//!
//! # Pipeline
//!
//! Source text is processed through four stages:
//!
//! ```text
//! source text
//!   → [parser]      ast::Journal        (PEG grammar + Pratt expressions)
//!   → [resolution]  resolution::HIR     (dates, aliases, metadata)
//!   → [elaboration] elaboration::Journal (evaluation, balancing)
//!   → serialisation                     (postcard + XZ → .bki)
//! ```
//!
//! The top-level entry point is [`compile`], which runs all three in-memory
//! stages and returns the elaborated [`Journal`]. For CLI usage see the
//! `ledger` binary in `src/main.rs`.
//!
//! # Modules
//!
//! - [`ast`] — abstract syntax tree produced by the parser.
//! - [`parser`] — pest-based parser and `include` directive handling.
//! - [`resolution`] — alias resolution, date normalisation, metadata
//!   extraction.
//! - [`elaboration`] — expression evaluation, transaction balancing, and the
//!   final serialisable [`Journal`] type.
//!
//! # Writing ledger text
//!
//! Use [`write_ledger`] to serialise a sequence of [`resolution::Transaction`]
//! values back to canonical Ledger source text:
//!
//! ```rust
//! # use ledger::resolution::{Transaction, Posting};
//! # use chrono::NaiveDate;
//! let txns = vec![
//!     Transaction::new(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
//!         .with_posting(Posting::new("Expenses:Food").with_amount((
//!             rust_decimal::Decimal::from(50u32), "$",
//!         )))
//!         .with_posting(Posting::new("Assets:Checking")),
//! ];
//! let mut out = Vec::new();
//! ledger::write_ledger(txns, &mut out).unwrap();
//! ```

pub mod ast;
pub mod elaboration;
pub mod parser;
pub mod resolution;

pub use elaboration::Journal;

/// Write a sequence of [`resolution::Transaction`] values to `writer` in
/// canonical Ledger source text format.
///
/// Each transaction is formatted using its [`std::fmt::Display`] impl and
/// separated from the next by a blank line. The output is suitable for
/// appending to or creating a `.ledger` source file and round-trips correctly
/// through the parser: `write_ledger(txns)` → parse → resolve should yield
/// semantically equivalent transactions.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if any write to `writer` fails.
///
/// # Example
///
/// ```rust
/// # use ledger::resolution::{Transaction, Posting};
/// # use chrono::NaiveDate;
/// let txns = vec![
///     Transaction::new(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
///         .with_posting(Posting::new("Expenses:Food").with_amount((
///             rust_decimal::Decimal::from(50u32), "$",
///         )))
///         .with_posting(Posting::new("Assets:Checking")),
/// ];
/// let mut out = Vec::new();
/// ledger::write_ledger(txns, &mut out).unwrap();
/// let text = String::from_utf8(out).unwrap();
/// assert!(text.starts_with("2024-01-15 Groceries"));
/// ```
pub fn write_ledger<W>(
    entries: impl IntoIterator<Item = resolution::Transaction>,
    writer: &mut W,
) -> std::io::Result<()>
where
    W: std::io::Write,
{
    let mut first = true;
    for txn in entries {
        if !first {
            writeln!(writer)?;
        }
        first = false;
        write!(writer, "{txn}")?;
    }
    Ok(())
}

/// Load and concatenate all files matching a glob pattern.
///
/// This is the default file-opener used by the CLI when processing `include`
/// directives. Multiple files matched by a single glob (e.g.
/// `include accounts/*.ledger`) are concatenated in the order that
/// [`glob::glob`] returns them (lexicographic on most platforms).
///
/// Panics if the pattern is invalid or a matched path cannot be read.
pub fn file_opener(pattern: &str) -> String {
    use std::io::Read as _;

    let mut buf = String::new();
    for path in glob::glob(pattern).unwrap() {
        std::fs::File::open(path.unwrap())
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();
    }
    buf
}

/// Compile Ledger source text into a fully elaborated [`Journal`].
///
/// Runs the three in-memory pipeline stages in sequence:
///
/// 1. [`parser::Parser::parse`] — tokenise `input` into an [`ast::Journal`].
/// 2. [`resolution::HIR::try_from`] — resolve dates, aliases, and metadata.
/// 3. [`elaboration::Journal::try_from`] — evaluate amounts and balance
///    transactions.
///
/// The `parser` argument supplies the file-opener for `include` directives and
/// the base path for relative path resolution. For single-file inputs without
/// includes, use [`parser::parse_ledger`] instead.
///
/// # Errors
///
/// Returns a boxed error from the first failing stage (parse error, resolution
/// error, or elaboration error).
pub fn compile<F>(
    input: &String,
    mut parser: parser::Parser<F>,
) -> Result<elaboration::Journal, Box<dyn std::error::Error>>
where
    F: Fn(&str) -> String,
{
    let output = parser.parse(input)?;
    let hir: resolution::HIR = output.try_into()?;
    Ok(hir.try_into()?)
}

#[cfg(test)]
mod write_ledger_tests {
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Parse a ledger source string and return the resolved transactions.
    fn parse_transactions(source: &str) -> Vec<resolution::Transaction> {
        let mut p = parser::Parser {
            opener: |_: &str| String::new(),
            base_path: std::path::PathBuf::new(),
        };
        let ast_journal = p.parse(&source.to_string()).expect("parse failed");
        let hir: resolution::HIR = ast_journal.try_into().expect("resolution failed");
        hir.entries
            .into_iter()
            .filter_map(|e| {
                if let resolution::Entry::Transaction(txn) = e.data {
                    Some(txn)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn write_empty_iterator_produces_no_output() {
        let mut out: Vec<u8> = Vec::new();
        write_ledger(std::iter::empty::<resolution::Transaction>(), &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn write_single_transaction_basic() {
        let txn = resolution::Transaction::new(date(2024, 1, 15), "Groceries")
            .with_posting(
                resolution::Posting::new("Expenses:Food")
                    .with_amount((Decimal::from(50u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([txn], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert!(text.starts_with("2024-01-15 Groceries"));
        assert!(text.contains("Expenses:Food"));
        assert!(text.contains("Assets:Checking"));
        // No trailing blank line for a single transaction.
        assert!(!text.ends_with("\n\n"));
    }

    #[test]
    fn multiple_transactions_separated_by_blank_line() {
        let txns = vec![
            resolution::Transaction::new(date(2024, 1, 1), "First"),
            resolution::Transaction::new(date(2024, 1, 2), "Second"),
        ];

        let mut out: Vec<u8> = Vec::new();
        write_ledger(txns, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        // There should be exactly one blank line separating the two transactions.
        assert!(text.contains("\n\n"), "expected blank-line separator between transactions");
        // But no trailing double newline after the last one.
        assert!(!text.ends_with("\n\n"));
    }

    #[test]
    fn round_trip_preserves_date_and_description() {
        let original = resolution::Transaction::new(date(2024, 3, 15), "Salary payment")
            .with_state(ast::TransactionState::Cleared)
            .with_posting(
                resolution::Posting::new("Income:Salary")
                    .with_amount((Decimal::from(5000u32), "USD")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([original], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        let parsed = parse_transactions(&text);
        assert_eq!(parsed.len(), 1);
        let roundtripped = &parsed[0];

        assert_eq!(roundtripped.date, date(2024, 3, 15));
        assert_eq!(roundtripped.description, "Salary payment");
        assert!(matches!(roundtripped.state, ast::TransactionState::Cleared));
        assert_eq!(roundtripped.postings.len(), 2);
        assert_eq!(roundtripped.postings[0].account, "Income:Salary");
        assert_eq!(roundtripped.postings[1].account, "Assets:Checking");
    }

    #[test]
    fn round_trip_preserves_metadata_and_tags() {
        // Use two comments so they are emitted as indented `; comment` note
        // lines (rather than inlined on the header), which round-trip cleanly
        // through the parser/resolver pipeline.
        let original = resolution::Transaction::new(date(2024, 6, 1), "Grant revenue")
            .with_tag("income")
            .with_comment("Q2 payment")
            .with_comment("approved")
            .with_metadata("program", "Grant:UW:HARVEST")
            .with_metadata("ref", "INV-001")
            .with_posting(
                resolution::Posting::new("Income:Grants")
                    .with_amount((Decimal::from(10_000u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([original], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        let parsed = parse_transactions(&text);
        assert_eq!(parsed.len(), 1);
        let rt = &parsed[0];

        assert!(rt.tags.contains(&"income".to_string()), "tag 'income' missing from {rt:?}");
        assert!(
            rt.comments.contains(&"Q2 payment".to_string()),
            "comment 'Q2 payment' missing from {rt:?}",
        );
        assert!(
            rt.comments.contains(&"approved".to_string()),
            "comment 'approved' missing from {rt:?}",
        );
        assert_eq!(rt.metadata.get("program").map(String::as_str), Some("Grant:UW:HARVEST"));
        assert_eq!(rt.metadata.get("ref").map(String::as_str), Some("INV-001"));
    }

    #[test]
    fn round_trip_multiple_transactions() {
        let txns = vec![
            resolution::Transaction::new(date(2024, 1, 10), "Food")
                .with_posting(
                    resolution::Posting::new("Expenses:Food")
                        .with_amount((Decimal::from(30u32), "$")),
                )
                .with_posting(resolution::Posting::new("Assets:Checking")),
            resolution::Transaction::new(date(2024, 1, 20), "Rent")
                .with_state(ast::TransactionState::Cleared)
                .with_posting(
                    resolution::Posting::new("Expenses:Rent")
                        .with_amount((Decimal::from(1200u32), "$")),
                )
                .with_posting(resolution::Posting::new("Assets:Checking")),
        ];

        let mut out: Vec<u8> = Vec::new();
        write_ledger(txns, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        let parsed = parse_transactions(&text);
        assert_eq!(parsed.len(), 2);

        assert_eq!(parsed[0].description, "Food");
        assert_eq!(parsed[0].date, date(2024, 1, 10));

        assert_eq!(parsed[1].description, "Rent");
        assert_eq!(parsed[1].date, date(2024, 1, 20));
        assert!(matches!(parsed[1].state, ast::TransactionState::Cleared));
    }

    #[test]
    fn round_trip_posting_with_metadata() {
        let original = resolution::Transaction::new(date(2024, 4, 1), "Payroll")
            .with_posting(
                resolution::Posting::new("Expenses:Salary")
                    .with_amount((Decimal::from(3000u32), "$"))
                    .with_metadata("employee", "alice")
                    .with_tag("payroll"),
            )
            .with_posting(resolution::Posting::new("Assets:Bank"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([original], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        let parsed = parse_transactions(&text);
        assert_eq!(parsed.len(), 1);
        let posting = &parsed[0].postings[0];

        assert_eq!(posting.account, "Expenses:Salary");
        assert_eq!(posting.metadata.get("employee").map(String::as_str), Some("alice"));
        assert!(posting.tags.contains(&"payroll".to_string()));
    }
}
