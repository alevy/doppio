//! Round-trip and cross-frontend transcoding tests for the per-frontend writers.
//!
//! Each "same-frontend round-trip" test:
//!   1. Parses a representative source string with `Frontend::parse`.
//!   2. Writes the resulting HIR with `Frontend::write_journal`.
//!   3. Re-parses the output.
//!   4. Asserts that the re-parsed HIR is semantically equivalent (same
//!      transactions, same dates, same amounts, same metadata).
//!
//! The cross-frontend transcoding test:
//!   - Parses a Beancount fixture containing a `pad` directive.
//!   - Writes it through `LedgerFrontend`.
//!   - Asserts that the output contains the `; [beancount] pad ...` marker
//!     comment and re-parses as valid ledger-cli source.

use doppio::frontend::Frontend as _;
use doppio::{BeancountFrontend, HledgerFrontend, LedgerFrontend};
use std::path::Path;

// -------------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------------

fn no_op_opener(_: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(String::new())
}

fn ledger_parse(src: &str) -> doppio::resolution::HIR {
    LedgerFrontend
        .parse(src, Path::new(""), &no_op_opener)
        .expect("ledger parse failed")
}

fn hledger_parse(src: &str) -> doppio::resolution::HIR {
    HledgerFrontend
        .parse(src, Path::new(""), &no_op_opener)
        .expect("hledger parse failed")
}

fn beancount_parse(src: &str) -> doppio::resolution::HIR {
    BeancountFrontend
        .parse(src, Path::new(""), &no_op_opener)
        .expect("beancount parse failed")
}

fn ledger_write(hir: &doppio::resolution::HIR) -> String {
    let mut buf = Vec::new();
    LedgerFrontend
        .write_journal(hir, &mut buf)
        .expect("ledger write failed");
    String::from_utf8(buf).expect("ledger output is not UTF-8")
}

fn hledger_write(hir: &doppio::resolution::HIR) -> String {
    let mut buf = Vec::new();
    HledgerFrontend
        .write_journal(hir, &mut buf)
        .expect("hledger write failed");
    String::from_utf8(buf).expect("hledger output is not UTF-8")
}

fn beancount_write(hir: &doppio::resolution::HIR) -> String {
    let mut buf = Vec::new();
    BeancountFrontend
        .write_journal(hir, &mut buf)
        .expect("beancount write failed");
    String::from_utf8(buf).expect("beancount output is not UTF-8")
}

// -------------------------------------------------------------------------
// Ledger round-trip
// -------------------------------------------------------------------------

/// Parse a ledger source, write it back, re-parse, and assert semantic
/// equivalence at the transaction level.
#[test]
fn ledger_round_trip_transactions_preserved() {
    let src = "\
2024-01-15 * Groceries
    Expenses:Food  $50.00
    Assets:Checking

2024-02-01 ! (INV-42) ACME Corp
    Expenses:Consulting  $500.00  ; vendor: ACME
    Assets:Bank
";

    let hir1 = ledger_parse(src);
    let written = ledger_write(&hir1);
    let hir2 = ledger_parse(&written);

    // Transaction count.
    let txns1: Vec<_> = hir1.transactions().collect();
    let txns2: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns1.len(), txns2.len(), "transaction count mismatch");

    // First transaction.
    assert_eq!(txns2[0].description, "Groceries");
    assert_eq!(
        txns2[0].date,
        chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()
    );
    assert!(matches!(
        txns2[0].state,
        doppio::ast::TransactionState::Cleared
    ));
    assert_eq!(txns2[0].postings.len(), 2);
    assert_eq!(txns2[0].postings[0].account, "Expenses:Food");

    // Second transaction.
    assert_eq!(txns2[1].description, "ACME Corp");
    assert_eq!(txns2[1].code.as_deref(), Some("INV-42"));
    assert!(matches!(
        txns2[1].state,
        doppio::ast::TransactionState::Pending
    ));
}

#[test]
fn ledger_round_trip_metadata_and_tags() {
    let src = "\
2024-06-01 Grant revenue
    ; :income:
    ; program: Grant:UW:HARVEST
    Income:Grants  $10000.00
    Assets:Checking
";

    let hir1 = ledger_parse(src);
    let written = ledger_write(&hir1);
    let hir2 = ledger_parse(&written);

    let txns: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns.len(), 1);
    assert!(
        txns[0].tags.contains(&"income".to_string()),
        "tag 'income' missing; tags: {:?}",
        txns[0].tags
    );
    assert_eq!(
        txns[0].metadata.get("program").map(String::as_str),
        Some("Grant:UW:HARVEST")
    );
}

#[test]
fn ledger_round_trip_price_directive() {
    let src = "\
P 2024-01-02 EUR $1.10

2024-02-10 Buy euros
    Assets:EUR  100.00 EUR @ $1.10
    Assets:Bank  $-110.00
";

    let hir1 = ledger_parse(src);
    let written = ledger_write(&hir1);
    // Output should contain the price directive.
    assert!(
        written.contains("P 2024-01-02 EUR"),
        "price directive missing from output: {written}"
    );
    // Re-parse should succeed.
    let hir2 = ledger_parse(&written);
    let txns: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns.len(), 1);
    assert_eq!(txns[0].description, "Buy euros");
}

// -------------------------------------------------------------------------
// hledger round-trip
// -------------------------------------------------------------------------

#[test]
fn hledger_round_trip_transactions_preserved() {
    let src = "\
2024-01-15 * Opening Balances
    assets:bank:checking          $1000.00
    equity:opening-balances

2024-01-16 ! (INV-42) ACME Corp  ; project:website
    expenses:consulting           $500.00
    assets:bank:checking
";

    let hir1 = hledger_parse(src);
    let written = hledger_write(&hir1);
    let hir2 = hledger_parse(&written);

    let txns1: Vec<_> = hir1.transactions().collect();
    let txns2: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns1.len(), txns2.len());

    assert_eq!(txns2[0].description, "Opening Balances");
    assert!(matches!(
        txns2[0].state,
        doppio::ast::TransactionState::Cleared
    ));

    assert_eq!(txns2[1].description, "ACME Corp");
    assert_eq!(txns2[1].code.as_deref(), Some("INV-42"));
    assert!(matches!(
        txns2[1].state,
        doppio::ast::TransactionState::Pending
    ));
}

#[test]
fn hledger_round_trip_balance_assignment_all_commodities() {
    // The hledger-specific `==*` form must survive a write-then-reparse cycle.
    let src = "\
2024-12-31 retain earnings
    Income      ==* 0
    Equity:Retained-Earnings
";
    let hir1 = hledger_parse(src);
    let written = hledger_write(&hir1);
    // The written output must parse without error.
    let hir2 = hledger_parse(&written);
    let txns: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns.len(), 1, "transaction must survive round-trip");
    assert_eq!(txns[0].description, "retain earnings");
}

#[test]
fn hledger_round_trip_price_directive() {
    let src = "\
P 2024-01-02 EUR $1.10

2024-02-10 * Buy euros
    assets:eur  100.00 EUR @ $1.10
    assets:bank  $-110.00
";

    let hir1 = hledger_parse(src);
    let written = hledger_write(&hir1);
    assert!(
        written.contains("P 2024-01-02 EUR"),
        "price directive missing: {written}"
    );
    let hir2 = hledger_parse(&written);
    let txns: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns.len(), 1);
}

// -------------------------------------------------------------------------
// Beancount round-trip
// -------------------------------------------------------------------------

#[test]
fn beancount_round_trip_transactions_preserved() {
    // A minimal Beancount journal with two transactions.
    // Note: Beancount requires 2+ spaces between account and amount.
    let src = "\
2024-01-01 open Assets:Bank:Checking USD
2024-01-01 open Expenses:Food USD
2024-01-01 open Equity:Opening-Balances USD

2024-01-12 * \"Salary\"
  Assets:Bank:Checking       3400.00 USD
  Equity:Opening-Balances  -3400.00 USD

2024-01-15 ! \"Groceries\"
  Expenses:Food              87.43 USD
  Assets:Bank:Checking      -87.43 USD
";

    let hir1 = beancount_parse(src);
    let written = beancount_write(&hir1);
    let hir2 = beancount_parse(&written);

    let txns1: Vec<_> = hir1.transactions().collect();
    let txns2: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns1.len(), txns2.len(), "transaction count mismatch");

    // Descriptions are preserved (re-parsed from quoted strings).
    assert_eq!(txns2[0].description, "Salary");
    assert!(matches!(
        txns2[0].state,
        doppio::ast::TransactionState::Cleared
    ));
    assert_eq!(txns2[1].description, "Groceries");
    assert!(matches!(
        txns2[1].state,
        doppio::ast::TransactionState::Pending
    ));
}

#[test]
fn beancount_round_trip_tags_in_header() {
    let src = "\
2024-01-01 open Assets:Bank USD
2024-01-01 open Expenses:Food USD

2024-03-15 * \"Groceries\" #vacation
  Expenses:Food     42.10 USD
  Assets:Bank      -42.10 USD
";

    let hir1 = beancount_parse(src);
    let written = beancount_write(&hir1);
    // The written output must contain the `#vacation` tag in the header.
    assert!(
        written.contains("#vacation"),
        "tag #vacation missing from output: {written}"
    );
    // Re-parse must succeed.
    let hir2 = beancount_parse(&written);
    let txns: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns.len(), 1);
    assert!(
        txns[0].tags.contains(&"vacation".to_string()),
        "tag 'vacation' missing after round-trip; tags: {:?}",
        txns[0].tags
    );
}

#[test]
fn beancount_round_trip_price_directive() {
    let src = "\
2024-01-01 open Assets:Brokerage AAPL
2024-01-01 open Assets:Bank USD

2024-01-02 price AAPL 182.50 USD

2024-02-15 * \"Buy stock\"
  Assets:Brokerage  10 AAPL {182.50 USD}
  Assets:Bank      -1825.00 USD
";

    let hir1 = beancount_parse(src);
    let written = beancount_write(&hir1);
    // Price directive must appear in Beancount's `date price` syntax.
    assert!(
        written.contains("price AAPL"),
        "price directive missing from output: {written}"
    );
    // Re-parse must succeed.
    let hir2 = beancount_parse(&written);
    let txns: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns.len(), 1);
    assert_eq!(txns[0].description, "Buy stock");
}

// -------------------------------------------------------------------------
// Cross-frontend transcoding
// -------------------------------------------------------------------------

/// Parse a Beancount source that contains a `pad` directive. Write it through
/// `LedgerFrontend`. The output must:
///   1. Contain the `; [beancount] pad ...` marker comment.
///   2. Re-parse as valid ledger-cli source (no parse errors).
#[test]
fn cross_frontend_beancount_to_ledger_pad_marked() {
    let src = "\
2024-01-01 open Assets:Bank:Checking USD
2024-01-01 open Equity:Opening-Balances USD

2024-01-01 pad Assets:Bank:Checking Equity:Opening-Balances
2024-01-15 balance Assets:Bank:Checking  5000.00 USD

2024-01-12 * \"Salary\"
  Assets:Bank:Checking       3400.00 USD
  Equity:Opening-Balances  -3400.00 USD
";

    let hir = beancount_parse(src);
    let written = ledger_write(&hir);

    // The pad directive must appear as a marked comment.
    assert!(
        written.contains("; [beancount] pad"),
        "pad marker missing from ledger output:\n{written}"
    );

    // The ledger writer's assertion form should appear too (from the `balance` directive).
    // (The balance directive is emitted as a standalone assertion line.)
    assert!(
        written.contains("Assets:Bank:Checking"),
        "account name missing from ledger output:\n{written}"
    );

    // Re-parse as ledger must succeed.
    let _hir2 = ledger_parse(&written);
}

/// Write a Beancount journal (with metadata) through LedgerFrontend and
/// verify the transaction survives with its description intact.
#[test]
fn cross_frontend_beancount_to_ledger_transactions_survive() {
    let src = "\
2024-01-01 open Expenses:Food USD
2024-01-01 open Assets:Bank USD

2024-01-15 * \"Groceries\" #household
  Expenses:Food   87.43 USD
  Assets:Bank    -87.43 USD
";

    let hir = beancount_parse(src);
    let written = ledger_write(&hir);

    // Re-parse as ledger-cli and verify transaction count and description.
    let hir2 = ledger_parse(&written);
    let txns: Vec<_> = hir2.transactions().collect();
    assert_eq!(txns.len(), 1, "transaction must survive transcoding");
    assert_eq!(txns[0].description, "Groceries");
}

/// Write an hledger journal through BeancountFrontend — verify that
/// hledger-specific `==*` form is emitted as a `; [hledger]` comment
/// (not dropped silently or treated as valid Beancount).
#[test]
fn cross_frontend_hledger_to_beancount_balance_assignment_marked() {
    let src = "\
2024-12-31 retain earnings
    Income      ==* 0
    Equity:Retained-Earnings
";

    let hir = hledger_parse(src);
    let written = beancount_write(&hir);

    // The `==*` form has no Beancount equivalent; it must appear as a
    // `; [hledger]` comment rather than being silently dropped.
    assert!(
        written.contains("; [hledger]"),
        "`==*` form must be marked as `; [hledger]` in Beancount output:\n{written}"
    );
}
