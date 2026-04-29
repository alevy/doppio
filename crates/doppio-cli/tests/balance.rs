//! Integration tests for the `balance` subcommand.
//!
//! These tests use `std::process::Command` to invoke the binary directly,
//! exercising the same code paths a CLI user hits.

use std::{io::Write as _, process::Command};

fn tmp_journal_file(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::with_suffix(".ledger").expect("tempfile");
    f.write_all(content.as_bytes()).expect("write");
    f
}

fn run(args: &[&str]) -> String {
    let bin = env!("CARGO_BIN_EXE_dop");
    let out = Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run dop binary");
    if !out.status.success() {
        panic!(
            "dop exited with {}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8(out.stdout).expect("non-UTF-8 stdout")
}

fn tagged_journal() -> &'static str {
    "2024-01-01 Tagged
    ; :payroll:
    Expenses:Salary  100 USD
    Assets:Checking

2024-02-01 Untagged
    Expenses:Food  20 USD
    Assets:Checking
"
}

#[test]
fn balance_tag_includes_only_matching_postings() {
    let f = tmp_journal_file(tagged_journal());
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "--tag",
        "payroll",
        "--flat",
    ]);
    assert!(
        out.contains("Expenses:Salary"),
        "tagged-txn account should appear: {out}"
    );
    assert!(
        !out.contains("Expenses:Food"),
        "untagged-txn account should be excluded: {out}"
    );
}

#[test]
fn balance_tag_no_match_returns_empty() {
    let f = tmp_journal_file(tagged_journal());
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "--tag",
        "nonexistent",
        "--flat",
    ]);
    assert!(out.trim().is_empty(), "expected no output: {out}");
}

// ---------------------------------------------------------------------------
// commodity directive: default, format, nomarket
// ---------------------------------------------------------------------------

#[test]
fn commodity_default_sets_default_commodity() {
    // A bare amount (no commodity symbol) in a posting should be interpreted
    // as `$` when `commodity $\n    default` is declared.
    let content = "commodity $
    default

2024-01-01 Groceries
    Expenses:Food  100
    Assets:Checking
";
    let f = tmp_journal_file(content);
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    // The bare `100` should be tagged as `$`, so the balance output should
    // contain `$` as the commodity label.
    assert!(
        out.contains('$'),
        "default commodity '$' should appear in balance output: {out}"
    );
    assert!(
        out.contains("100"),
        "amount 100 should appear in balance output: {out}"
    );
}

#[test]
fn commodity_format_prefix_symbol_applied_to_balance() {
    // `format $1,000.00` → prefix `$`, thousands `,`, 2 decimal places.
    // A balance of 1234.56 should render as `$1,234.56`, not `$ 1234.56`.
    let content = "commodity $
    format $1,000.00

2024-01-01 Salary
    Income:Salary  -1234.56 $
    Assets:Checking  1234.56 $
";
    let f = tmp_journal_file(content);
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(
        out.contains("$1,234.56"),
        "formatted amount should appear as '$1,234.56': {out}"
    );
}

#[test]
fn commodity_nomarket_stored_does_not_break_output() {
    // `nomarket` is a flag — it should not affect output or cause an error.
    let content = "commodity USD
    nomarket

2024-01-01 Purchase
    Expenses:Food  50 USD
    Assets:Checking
";
    let f = tmp_journal_file(content);
    // Should not panic or produce an error.
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("USD"), "commodity should still appear: {out}");
    assert!(out.contains("50"), "amount should appear: {out}");
}

#[test]
fn commodity_format_suffix_symbol_applied_to_register() {
    // `format 1.000,00 EUR` → suffix ` EUR`, thousands `.`, decimal `,`, 2 places.
    let content = "commodity EUR
    format 1.000,00 EUR

2024-01-01 Invoice
    Income:Consulting  -2500 EUR
    Assets:Bank  2500 EUR
";
    let f = tmp_journal_file(content);
    let out = run(&["register", f.path().to_str().unwrap()]);
    // The register output should show `2,500.00 EUR` ... but the format uses
    // `.` as thousands sep and `,` as decimal — so it should be `2.500,00 EUR`.
    assert!(
        out.contains("2.500,00 EUR"),
        "formatted amount should appear as '2.500,00 EUR': {out}"
    );
}

#[test]
fn commodity_format_single_separator_three_digits_is_thousands() {
    // `format $1.000` — a single separator followed by exactly 3 digits is a
    // *thousands* separator per ledger convention, not a decimal point.
    // A balance of 1000 should render as `$1.000`, not `$1` (which would
    // result from misinterpreting `.000` as 3 decimal places on `1.000`).
    let content = "commodity $
    format $1.000

2024-01-01 Deposit
    Assets:Checking  1000 $
    Income:Salary
";
    let f = tmp_journal_file(content);
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(
        out.contains("$1.000"),
        "amount 1000 with format '$1.000' should render as '$1.000': {out}"
    );
}

// ---------------------------------------------------------------------------
// --exchange FX conversion
// ---------------------------------------------------------------------------

#[test]
fn balance_exchange_converts_eur_to_usd() {
    // A price directive `P 2024-01-01 EUR $ 1.10` declares that 1 EUR = $1.10.
    // A posting of 100 EUR should appear as $ 110 in the balance when
    // --exchange $ is passed.
    let content = "P 2024-01-01 EUR $ 1.10

2024-01-15 Foreign purchase
    Expenses:Travel  100 EUR
    Assets:Checking
";
    let f = tmp_journal_file(content);
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "--flat",
        "--exchange",
        "$",
    ]);
    // After conversion: 100 EUR × 1.10 = 110 $
    assert!(
        out.contains("110"),
        "converted amount 110 should appear in balance output: {out}"
    );
    assert!(
        out.contains('$'),
        "target commodity '$' should appear in balance output: {out}"
    );
    assert!(
        !out.contains("EUR"),
        "source commodity 'EUR' should be absent after conversion: {out}"
    );
}
