//! doppio — a compiler and query library for the Ledger plain-text
//! accounting format.
//!
//! # `.dop` binary format
//!
//! The `dop compile` command serialises an elaborated journal to a `.dop`
//! file. The file begins with an 8-byte header followed by the postcard +
//! XZ-compressed journal body:
//!
//! ```text
//! Offset  Length  Content
//! 0       4       Magic: b"DOP\0"
//! 4       2       Version: u16 LE (currently 1)
//! 6       2       Reserved: u16 LE (write 0, ignore on read)
//! ```
//!
//! Use [`dop_write_header`] / [`dop_read_header`] for portable, tested I/O of
//! this header.
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
//!   → serialisation                     (postcard + XZ → .dop)
//! ```
//!
//! The top-level entry point is [`compile`], which runs all three in-memory
//! stages and returns the elaborated [`Journal`]. For CLI usage see the
//! `dop` binary in `src/main.rs`.
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
//! # Serialising transactions as Ledger text
//!
//! Use [`write_ledger`] to serialise a sequence of [`resolution::Transaction`]
//! values back to canonical Ledger source text:
//!
//! ```rust
//! # use doppio::resolution::{Transaction, Posting};
//! # use chrono::NaiveDate;
//! let txns = vec![
//!     Transaction::new(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
//!         .with_posting(Posting::new("Expenses:Food").with_amount((
//!             rust_decimal::Decimal::from(50u32), "$",
//!         )))
//!         .with_posting(Posting::new("Assets:Checking")),
//! ];
//! let mut out = Vec::new();
//! doppio::write_ledger(txns, &mut out).unwrap();
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
/// # use doppio::resolution::{Transaction, Posting};
/// # use chrono::NaiveDate;
/// let txns = vec![
///     Transaction::new(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
///         .with_posting(Posting::new("Expenses:Food").with_amount((
///             rust_decimal::Decimal::from(50u32), "$",
///         )))
///         .with_posting(Posting::new("Assets:Checking")),
/// ];
/// let mut out = Vec::new();
/// doppio::write_ledger(txns, &mut out).unwrap();
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
    input: &str,
    mut parser: parser::Parser<F>,
) -> Result<elaboration::Journal, Box<dyn std::error::Error>>
where
    F: Fn(&str) -> String,
{
    let output = parser.parse(input)?;
    let hir: resolution::HIR = output.try_into()?;
    Ok(hir.try_into()?)
}

/// Evaluate a single [`resolution::Transaction`] through the elaboration stage.
///
/// This is the bridge between programmatic transaction construction (via the
/// [`resolution::Transaction`] builder API) and full elaboration. It resolves
/// aliases, evaluates amount expressions, balances postings, and applies cost
/// basis — returning a fully resolved transaction or an error.
///
/// The `context` parameter supplies alias definitions, commodity aliases, and
/// the default commodity. Use [`resolution::Context::default()`] when no
/// aliases or default commodity are needed.
///
/// Internally this constructs a minimal [`resolution::HIR`] containing the
/// single transaction, runs the elaboration pipeline, and extracts the result.
///
/// # Errors
///
/// Returns an [`elaboration::ElaborationError`] if the transaction cannot be
/// elaborated (e.g. unbalanced postings, expression evaluation failure, or
/// too many null postings).
///
/// # Example
///
/// ```rust
/// use doppio::resolution::{Context, Transaction, Posting};
/// use chrono::NaiveDate;
/// use rust_decimal::Decimal;
///
/// let txn = Transaction::new(
///     NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
///     "Groceries",
/// )
/// .with_posting(
///     Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "$")),
/// )
/// .with_posting(Posting::new("Assets:Checking"));
///
/// let resolved = doppio::eval_transaction(txn, &Context::default()).unwrap();
/// assert_eq!(resolved.description, "Groceries");
/// assert_eq!(resolved.postings.len(), 2);
/// ```
pub fn eval_transaction(
    txn: resolution::Transaction,
    context: &resolution::Context,
) -> Result<elaboration::ResolvedTransaction, elaboration::ElaborationError> {
    let hir = resolution::HIR {
        entries: vec![resolution::ResolutionEntry {
            context_id: 0,
            data: resolution::Entry::Transaction(txn),
        }],
        contexts: vec![context.clone()],
        ..Default::default()
    };
    let journal = elaboration::Journal::try_from(hir)?;
    // The HIR contained exactly one transaction, so the journal has exactly one.
    Ok(journal
        .transactions
        .into_iter()
        .next()
        .expect("journal should contain exactly one transaction"))
}

// ──────────────────────────────────────────────────────────────────────────────
// .dop header helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Four-byte magic that identifies every `.dop` file.
pub const DOP_MAGIC: [u8; 4] = *b"DOP\0";

/// Format version embedded in every `.dop` header.
///
/// Bump this constant (and update [`dop_read_header`]) whenever the
/// serialisation format changes in a breaking way.
pub const DOP_FORMAT_VERSION: u16 = 1;

/// Write the 8-byte `.dop` header to `writer`.
///
/// Layout: magic (4 bytes) + version LE u16 (2 bytes) + reserved u16 (2 bytes).
///
/// # Errors
///
/// Propagates any [`std::io::Error`] from `writer`.
pub fn dop_write_header<W: std::io::Write>(writer: &mut W) -> std::io::Result<()> {
    writer.write_all(&DOP_MAGIC)?;
    writer.write_all(&DOP_FORMAT_VERSION.to_le_bytes())?;
    writer.write_all(&0u16.to_le_bytes())?;
    Ok(())
}

/// Read and validate the 8-byte `.dop` header from `reader`.
///
/// `path` is used only for error messages; it should be the path of the file
/// being opened so diagnostics point to the right location.
///
/// # Errors
///
/// Returns a boxed error with a user-actionable message if:
/// - the magic bytes are missing or incorrect,
/// - the format version is not [`DOP_FORMAT_VERSION`].
pub fn dop_read_header<R: std::io::Read>(
    reader: &mut R,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut magic = [0u8; 4];
    // A short read here means the file is too small to be valid.
    reader.read_exact(&mut magic).map_err(|_| {
        format!(
            "{}: not a valid .dop file (missing magic header); \
             recompile from source with `dop compile`",
            path.display()
        )
    })?;
    if magic != DOP_MAGIC {
        return Err(format!(
            "{}: not a valid .dop file (missing magic header); \
             recompile from source with `dop compile`",
            path.display()
        )
        .into());
    }

    let mut version_bytes = [0u8; 2];
    reader.read_exact(&mut version_bytes)?;
    let version = u16::from_le_bytes(version_bytes);
    if version != DOP_FORMAT_VERSION {
        return Err(format!(
            "{}: incompatible .dop format version {} \
             (this binary supports version {}); \
             recompile from source with `dop compile`",
            path.display(),
            version,
            DOP_FORMAT_VERSION,
        )
        .into());
    }

    // Skip the 2 reserved bytes — ignore their value on read.
    let mut reserved = [0u8; 2];
    reader.read_exact(&mut reserved)?;
    Ok(())
}

#[cfg(test)]
mod write_ledger_tests {
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Parse a Ledger-format source string and return the resolved transactions.
    fn parse_transactions(source: &str) -> Vec<resolution::Transaction> {
        let mut p = parser::Parser {
            opener: |_: &str| String::new(),
            base_path: std::path::PathBuf::new(),
        };
        let ast_journal = p.parse(&source.to_string()).expect("parse failed");
        let hir: resolution::HIR = ast_journal.try_into().expect("resolution failed");
        hir.transactions().collect()
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
                resolution::Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([txn], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert_eq!(
            text,
            "2024-01-15 Groceries\n    Expenses:Food  50 $\n    Assets:Checking\n"
        );
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

        assert_eq!(text, "2024-01-01 First\n\n2024-01-02 Second\n");
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

        assert!(
            rt.tags.contains(&"income".to_string()),
            "tag 'income' missing from {rt:?}"
        );
        assert!(
            rt.comments.contains(&"Q2 payment".to_string()),
            "comment 'Q2 payment' missing from {rt:?}",
        );
        assert!(
            rt.comments.contains(&"approved".to_string()),
            "comment 'approved' missing from {rt:?}",
        );
        assert_eq!(
            rt.metadata.get("program").map(String::as_str),
            Some("Grant:UW:HARVEST")
        );
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
        assert_eq!(
            posting.metadata.get("employee").map(String::as_str),
            Some("alice")
        );
        assert!(posting.tags.contains(&"payroll".to_string()));
    }
}

#[cfg(test)]
mod eval_transaction_tests {
    use chrono::NaiveDate;
    use rust_decimal::{Decimal, dec};

    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn simple_two_posting_transaction() {
        let txn = resolution::Transaction::new(date(2024, 1, 15), "Groceries")
            .with_posting(
                resolution::Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let resolved = eval_transaction(txn, &resolution::Context::default()).unwrap();

        assert_eq!(resolved.description, "Groceries");
        assert_eq!(resolved.postings.len(), 2);

        let food = resolved
            .postings
            .iter()
            .find(|p| p.account == "Expenses:Food")
            .unwrap();
        assert_eq!(food.amount.0.get("$").copied(), Some(dec!(50)));

        let checking = resolved
            .postings
            .iter()
            .find(|p| p.account == "Assets:Checking")
            .unwrap();
        assert_eq!(checking.amount.0.get("$").copied(), Some(dec!(-50)));
    }

    #[test]
    fn null_posting_inferred() {
        let txn = resolution::Transaction::new(date(2024, 2, 1), "Rent")
            .with_posting(
                resolution::Posting::new("Expenses:Rent")
                    .with_amount((Decimal::from(1200u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let resolved = eval_transaction(txn, &resolution::Context::default()).unwrap();

        let checking = resolved
            .postings
            .iter()
            .find(|p| p.account == "Assets:Checking")
            .unwrap();
        assert_eq!(
            checking.amount.0.get("$").copied(),
            Some(dec!(-1200)),
            "null posting should be inferred as -$1200"
        );
    }

    #[test]
    fn explicit_balanced_amounts() {
        let txn = resolution::Transaction::new(date(2024, 3, 1), "Transfer")
            .with_posting(
                resolution::Posting::new("Assets:Savings")
                    .with_amount((Decimal::from(500u32), "$")),
            )
            .with_posting(
                resolution::Posting::new("Assets:Checking")
                    .with_amount((Decimal::from(-500i32), "$")),
            );

        let resolved = eval_transaction(txn, &resolution::Context::default()).unwrap();
        assert_eq!(resolved.postings.len(), 2);
    }

    #[test]
    fn unbalanced_transaction_returns_error() {
        let txn = resolution::Transaction::new(date(2024, 4, 1), "Bad")
            .with_posting(
                resolution::Posting::new("Expenses:Food").with_amount((Decimal::from(100u32), "$")),
            )
            .with_posting(
                resolution::Posting::new("Assets:Checking")
                    .with_amount((Decimal::from(-50i32), "$")),
            );

        let result = eval_transaction(txn, &resolution::Context::default());
        assert!(
            result.is_err(),
            "unbalanced transaction should return an error"
        );
        assert!(matches!(
            result.unwrap_err(),
            elaboration::ElaborationError::TransactionDoesNotBalance(_)
        ));
    }

    #[test]
    fn account_alias_resolved_via_context() {
        let mut context = resolution::Context::default();
        context
            .account_aliases
            .insert("Checking".into(), "Assets:Checking:Mercury:7920".into());

        let txn = resolution::Transaction::new(date(2024, 5, 1), "Deposit")
            .with_posting(
                resolution::Posting::new("Income:Salary")
                    .with_amount((Decimal::from(5000u32), "$")),
            )
            .with_posting(resolution::Posting::new("Checking"));

        let resolved = eval_transaction(txn, &context).unwrap();

        let checking = resolved
            .postings
            .iter()
            .find(|p| p.account == "Assets:Checking:Mercury:7920")
            .expect("alias should resolve to canonical account name");
        assert_eq!(checking.amount.0.get("$").copied(), Some(dec!(-5000)));
    }

    #[test]
    fn default_commodity_from_context() {
        let mut context = resolution::Context::default();
        context.default_commodity = Some("USD".into());

        // Amount with no commodity — should use the context default.
        // Use ValueExpr::Amount with commodity: None to produce a bare amount.
        let bare = ast::ValueExpr::Amount {
            value: Decimal::from(25u32),
            commodity: None,
        };
        let txn = resolution::Transaction::new(date(2024, 6, 1), "Bare amount")
            .with_posting(resolution::Posting::new("Expenses:Food").with_amount(bare))
            .with_posting(resolution::Posting::new("Assets:Cash"));

        let resolved = eval_transaction(txn, &context).unwrap();

        let food = resolved
            .postings
            .iter()
            .find(|p| p.account == "Expenses:Food")
            .unwrap();
        assert_eq!(
            food.amount.0.get("USD").copied(),
            Some(dec!(25)),
            "bare amount should use default commodity from context"
        );
    }

    #[test]
    fn resolved_transaction_preserves_fields() {
        let txn = resolution::Transaction::new(date(2024, 7, 4), "Independence Day")
            .with_state(ast::TransactionState::Cleared)
            .with_code("IND-04")
            .with_secondary_date(date(2024, 7, 5))
            .with_tag("holiday")
            .with_metadata("ref", "USA")
            .with_posting(
                resolution::Posting::new("Expenses:Celebration")
                    .with_amount((Decimal::from(200u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let resolved = eval_transaction(txn, &resolution::Context::default()).unwrap();

        assert_eq!(resolved.description, "Independence Day");
        assert!(matches!(
            resolved.state,
            elaboration::TransactionState::Cleared
        ));
        assert_eq!(resolved.code.as_deref(), Some("IND-04"));
        assert!(resolved.secondary_date.is_some());
        assert!(resolved.tags.contains(&"holiday".to_string()));
        assert_eq!(
            resolved.metadata.get("ref").map(String::as_str),
            Some("USA")
        );
    }

    #[test]
    fn too_many_null_postings_returns_error() {
        let txn = resolution::Transaction::new(date(2024, 8, 1), "Ambiguous")
            .with_posting(resolution::Posting::new("Expenses:A"))
            .with_posting(resolution::Posting::new("Expenses:B"));

        let result = eval_transaction(txn, &resolution::Context::default());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            elaboration::ElaborationError::TooManyNullPostings
        ));
    }
}
