//! Integration tests for [`doppio::resolution::HirBuilder`].
//!
//! These tests exercise only the public API, mirroring the experience of an
//! external caller constructing and serialising transactions without access to
//! crate-private types.

use chrono::NaiveDate;
use doppio::frontend::Frontend as _;
use doppio::resolution::{HirBuilder, HistoricalPrice, Posting, Transaction};
use doppio::{BeancountFrontend, HledgerFrontend, LedgerFrontend};
use rust_decimal::Decimal;
use std::path::Path;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn no_op_opener(_: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(String::new())
}

fn write_ledger(hir: &doppio::resolution::HIR) -> String {
    let mut buf = Vec::new();
    LedgerFrontend
        .write_journal(hir, &mut buf)
        .expect("ledger write failed");
    String::from_utf8(buf).expect("ledger output is not UTF-8")
}

fn write_hledger(hir: &doppio::resolution::HIR) -> String {
    let mut buf = Vec::new();
    HledgerFrontend
        .write_journal(hir, &mut buf)
        .expect("hledger write failed");
    String::from_utf8(buf).expect("hledger output is not UTF-8")
}

fn write_beancount(hir: &doppio::resolution::HIR) -> String {
    let mut buf = Vec::new();
    BeancountFrontend
        .write_journal(hir, &mut buf)
        .expect("beancount write failed");
    String::from_utf8(buf).expect("beancount output is not UTF-8")
}

/// Build a simple 2-posting transaction via [`HirBuilder`], write it through all
/// three frontends, and verify that each frontend's output:
///
/// - is non-empty,
/// - contains the payee string,
/// - contains both account names, and
/// - round-trips through the same frontend's parser without error.
#[test]
fn hir_builder_writes_through_all_frontends() {
    // Use "USD" as the commodity so the output round-trips through the
    // beancount parser, which requires commodities to start with an uppercase
    // letter (symbols like "$" are not valid beancount commodities).
    let txn = Transaction::new(date(2024, 3, 10), "Market Run")
        .with_posting(
            Posting::new("Expenses:Groceries").with_amount((Decimal::from(75u32), "USD")),
        )
        .with_posting(Posting::new("Assets:Checking"));

    let hir = HirBuilder::new().push_transaction(txn).build();

    // --- Ledger ---
    let ledger_out = write_ledger(&hir);
    assert!(!ledger_out.is_empty(), "ledger output should be non-empty");
    assert!(
        ledger_out.contains("Market Run"),
        "ledger output should contain the payee"
    );
    assert!(
        ledger_out.contains("Expenses:Groceries"),
        "ledger output should contain Expenses:Groceries"
    );
    assert!(
        ledger_out.contains("Assets:Checking"),
        "ledger output should contain Assets:Checking"
    );
    // Round-trip: output must parse back without error.
    LedgerFrontend
        .parse(&ledger_out, Path::new(""), &no_op_opener)
        .expect("ledger round-trip parse failed");

    // --- hledger ---
    let hledger_out = write_hledger(&hir);
    assert!(
        !hledger_out.is_empty(),
        "hledger output should be non-empty"
    );
    assert!(
        hledger_out.contains("Market Run"),
        "hledger output should contain the payee"
    );
    assert!(
        hledger_out.contains("Expenses:Groceries"),
        "hledger output should contain Expenses:Groceries"
    );
    assert!(
        hledger_out.contains("Assets:Checking"),
        "hledger output should contain Assets:Checking"
    );
    HledgerFrontend
        .parse(&hledger_out, Path::new(""), &no_op_opener)
        .expect("hledger round-trip parse failed");

    // --- Beancount ---
    let beancount_out = write_beancount(&hir);
    assert!(
        !beancount_out.is_empty(),
        "beancount output should be non-empty"
    );
    assert!(
        beancount_out.contains("Market Run"),
        "beancount output should contain the payee"
    );
    assert!(
        beancount_out.contains("Expenses:Groceries"),
        "beancount output should contain Expenses:Groceries"
    );
    assert!(
        beancount_out.contains("Assets:Checking"),
        "beancount output should contain Assets:Checking"
    );
    BeancountFrontend
        .parse(&beancount_out, Path::new(""), &no_op_opener)
        .expect("beancount round-trip parse failed");
}

/// Build an HIR containing a transaction and a [`HistoricalPrice`] directive,
/// then verify that all three frontends emit both the price and the transaction.
#[test]
fn hir_builder_handles_prices() {
    // Use "USD" as the cash commodity so the output is valid beancount syntax.
    let txn = Transaction::new(date(2024, 6, 1), "Stock Purchase")
        .with_posting(
            Posting::new("Assets:Brokerage").with_amount((Decimal::from(10u32), "AAPL")),
        )
        .with_posting(
            Posting::new("Assets:Cash").with_amount((Decimal::from(-1750i32), "USD")),
        );

    let price = HistoricalPrice {
        date: date(2024, 6, 1),
        time: None,
        commodity: "AAPL".to_string(),
        price: doppio::ast::ValueExpr::Amount {
            value: Decimal::from(175u32),
            commodity: Some("USD".to_string()),
        },
    };

    let hir = HirBuilder::new()
        .push_transaction(txn)
        .push_price(price)
        .build();

    // --- Ledger ---
    let ledger_out = write_ledger(&hir);
    assert!(
        ledger_out.contains("Stock Purchase"),
        "ledger output should contain the payee"
    );
    // Ledger and hledger price directives start with "P".
    assert!(
        ledger_out.contains('P'),
        "ledger output should contain a P price directive"
    );
    assert!(
        ledger_out.contains("AAPL"),
        "ledger output should contain the commodity name"
    );
    LedgerFrontend
        .parse(&ledger_out, Path::new(""), &no_op_opener)
        .expect("ledger round-trip parse failed for price + transaction");

    // --- hledger ---
    let hledger_out = write_hledger(&hir);
    assert!(
        hledger_out.contains("Stock Purchase"),
        "hledger output should contain the payee"
    );
    assert!(
        hledger_out.contains('P'),
        "hledger output should contain a P price directive"
    );
    assert!(
        hledger_out.contains("AAPL"),
        "hledger output should contain the commodity name"
    );
    HledgerFrontend
        .parse(&hledger_out, Path::new(""), &no_op_opener)
        .expect("hledger round-trip parse failed for price + transaction");

    // --- Beancount ---
    let beancount_out = write_beancount(&hir);
    assert!(
        beancount_out.contains("Stock Purchase"),
        "beancount output should contain the payee"
    );
    // Beancount uses the "price" keyword.
    assert!(
        beancount_out.contains("price"),
        "beancount output should contain a 'price' directive"
    );
    assert!(
        beancount_out.contains("AAPL"),
        "beancount output should contain the commodity name"
    );
    BeancountFrontend
        .parse(&beancount_out, Path::new(""), &no_op_opener)
        .expect("beancount round-trip parse failed for price + transaction");
}
