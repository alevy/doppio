//! External-construction round-trip tests for `resolution::HIR`.
//!
//! Verifies that callers outside the `doppio` crate can build an `HIR` from
//! scratch (via `HIR::new()` + `HIR::append_entry`), serialise it through
//! each frontend, and re-parse the result without error.
//!
//! These tests exercise only the public API, mirroring the external-caller
//! experience: the import-and-emit use case for tools like bookie.

use std::path::Path;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use doppio::resolution::{Entry, HIR, HistoricalPrice, Posting, Transaction};
use doppio::{BeancountFrontend, Frontend, HledgerFrontend, LedgerFrontend};

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

fn no_opener() -> Box<doppio::frontend::Opener> {
    Box::new(|_| Ok(String::new()))
}

fn render<F: Frontend>(frontend: &F, hir: &HIR) -> String {
    let mut buf = Vec::new();
    doppio::write_journal(frontend, hir, &mut buf).expect("write_journal succeeds");
    String::from_utf8(buf).expect("output is valid UTF-8")
}

fn roundtrip<F: Frontend>(frontend: &F, text: &str) -> HIR {
    let opener = no_opener();
    frontend
        .parse(text, Path::new(""), &opener)
        .unwrap_or_else(|e| panic!("re-parse failed: {e}"))
}

#[test]
fn append_entry_writes_through_all_frontends() {
    // "USD" rather than "$" because beancount commodities must start with an
    // uppercase ASCII letter -- $ is not a valid beancount commodity.
    let txn = Transaction::new(date(2024, 3, 10), "Market Run")
        .with_posting(Posting::new("Expenses:Groceries").with_amount((Decimal::from(75u32), "USD")))
        .with_posting(Posting::new("Assets:Checking"));

    let mut hir = HIR::new();
    hir.append_entry(Entry::Transaction(txn));

    for (name, text) in [
        ("ledger", render(&LedgerFrontend, &hir)),
        ("hledger", render(&HledgerFrontend, &hir)),
        ("beancount", render(&BeancountFrontend, &hir)),
    ] {
        assert!(
            !text.trim().is_empty(),
            "[{name}] output should be non-empty"
        );
        assert!(
            text.contains("Market Run"),
            "[{name}] output should contain the description; got:\n{text}"
        );
        assert!(
            text.contains("Expenses:Groceries"),
            "[{name}] output should contain the first account; got:\n{text}"
        );
        assert!(
            text.contains("Assets:Checking"),
            "[{name}] output should contain the second account; got:\n{text}"
        );
    }

    // Round-trip: each frontend should re-parse its own output to exactly
    // one transaction. `transactions()` consumes the HIR; clone-by-render-and-parse
    // means each assertion gets a fresh parsed HIR.
    let parsed = roundtrip(&LedgerFrontend, &render(&LedgerFrontend, &hir));
    assert_eq!(parsed.transactions().count(), 1, "ledger round-trip");
    let parsed = roundtrip(&HledgerFrontend, &render(&HledgerFrontend, &hir));
    assert_eq!(parsed.transactions().count(), 1, "hledger round-trip");
    let parsed = roundtrip(&BeancountFrontend, &render(&BeancountFrontend, &hir));
    assert_eq!(parsed.transactions().count(), 1, "beancount round-trip");
}

#[test]
fn append_entry_with_prices() {
    let txn = Transaction::new(date(2024, 6, 1), "Stock Purchase")
        .with_posting(Posting::new("Assets:Brokerage").with_amount((Decimal::from(10u32), "AAPL")))
        .with_posting(Posting::new("Assets:Cash").with_amount((Decimal::from(-1750i32), "USD")));

    let price = HistoricalPrice {
        date: date(2024, 6, 1),
        time: None,
        commodity: "AAPL".to_string(),
        price: (Decimal::from(175u32), "USD").into(),
    };

    let mut hir = HIR::new();
    hir.prices.push(price);
    hir.append_entry(Entry::Transaction(txn));

    for (name, text) in [
        ("ledger", render(&LedgerFrontend, &hir)),
        ("hledger", render(&HledgerFrontend, &hir)),
        ("beancount", render(&BeancountFrontend, &hir)),
    ] {
        assert!(
            text.contains("Stock Purchase"),
            "[{name}] should contain transaction description; got:\n{text}"
        );
        assert!(
            text.contains("AAPL") && text.contains("175"),
            "[{name}] should contain the price directive; got:\n{text}"
        );
    }
}
