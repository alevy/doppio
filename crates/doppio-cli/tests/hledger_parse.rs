//! Integration tests for the hledger frontend.
//!
//! These tests invoke the `dop` binary directly (same as CLI users would) and
//! exercise:
//!
//! 1. Round-trip of every entry form in the prototype `sample.hledger`.
//! 2. File-extension dispatch: `.hledger` and `.journal` both reach the
//!    hledger frontend.
//! 3. End-to-end: `dop balance` against a minimal hledger journal exits 0.
//! 4. Balance assertions do not break elaboration.
//! 5. Lot-priced postings parse and appear in the balance output.

use std::{io::Write as _, path::PathBuf, process::Command};

// -- helpers ------------------------------------------------------------------─

/// Absolute path to the `dop` binary under test.
fn dop_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dop"))
}

/// Run `dop <args>` and return stdout on success, panicking with diagnostics on failure.
fn run(args: &[&str]) -> String {
    let out = Command::new(dop_bin())
        .args(args)
        .output()
        .expect("failed to run dop binary");
    if !out.status.success() {
        panic!(
            "dop exited with {}\nargs: {:?}\nstderr: {}",
            out.status,
            args,
            String::from_utf8_lossy(&out.stderr),
        );
    }
    String::from_utf8(out.stdout).expect("non-UTF-8 stdout")
}

/// Run `dop <args>` and expect failure; returns the stderr text.
fn run_fail(args: &[&str]) -> String {
    let out = Command::new(dop_bin())
        .args(args)
        .output()
        .expect("failed to run dop binary");
    assert!(
        !out.status.success(),
        "expected dop to fail but it exited 0\nargs: {args:?}"
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Write `content` to a temporary file with the given suffix and return it.
/// The file is kept alive for the duration of the test via the returned handle.
fn tmp_file(content: &str, suffix: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::with_suffix(suffix).expect("tempfile");
    f.write_all(content.as_bytes()).expect("write");
    f
}

/// Path to the bundled sample fixture.
fn sample_hledger_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.hledger")
}

// -- extension dispatch --------------------------------------------------------

/// A `.hledger` file is dispatched to the hledger frontend and parses cleanly.
#[test]
fn hledger_extension_dispatches_to_hledger_frontend() {
    let f = tmp_file(
        "2024-01-01 * Test\n    expenses:food  $10\n    assets:cash\n",
        ".hledger",
    );
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("10"), "amount should appear: {out}");
}

/// A `.journal` file is dispatched to the hledger frontend and parses cleanly.
#[test]
fn journal_extension_dispatches_to_hledger_frontend() {
    let f = tmp_file(
        "2024-01-01 * Test\n    expenses:food  $10\n    assets:cash\n",
        ".journal",
    );
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("10"), "amount should appear: {out}");
}

// -- sample fixture round-trips ------------------------------------------------

/// `dop balance sample.hledger` exits 0 (end-to-end smoke test).
#[test]
fn sample_hledger_balance_exits_zero() {
    let path = sample_hledger_path();
    // The fixture has an `include common.journal` directive which will fail to
    // open. Run without includes by relying on the fact that the CLI passes
    // `file_opener` which returns an error -- but `include` silently fails on
    // missing includes only in the no-op opener.  To keep the fixture clean,
    // we make a copy without the include line.
    let content = std::fs::read_to_string(&path).expect("read sample");
    let without_include: String = content
        .lines()
        .filter(|l| !l.trim_start().starts_with("include"))
        .map(|l| format!("{l}\n"))
        .collect();
    let f = tmp_file(&without_include, ".hledger");
    run(&["balance", f.path().to_str().unwrap(), "--flat"]);
}

/// The sample fixture contains cleared transactions that affect balances.
#[test]
fn sample_hledger_cleared_transactions_appear_in_balance() {
    let content = fixture_without_includes();
    let f = tmp_file(&content, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    // The sample has postings to assets:bank:checking and expenses:consulting.
    assert!(
        out.contains("assets:bank:checking") || out.contains("checking"),
        "checking account should appear: {out}"
    );
}

/// Historical price directives parse without error (they are stored as prices).
#[test]
fn sample_hledger_historical_prices_parse() {
    let content = fixture_without_includes();
    let f = tmp_file(&content, ".hledger");
    // If prices fail to parse, `dop balance` would return a non-zero exit code.
    run(&["balance", f.path().to_str().unwrap(), "--flat"]);
}

/// Account directives parse without error.
#[test]
fn sample_hledger_account_directives_parse() {
    let content = "account assets:bank:checking    ; type:A\naccount expenses:groceries\n\n2024-01-01 * Test\n    expenses:groceries  $10\n    assets:bank:checking\n";
    let f = tmp_file(content, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("10"), "amount should appear: {out}");
}

/// Commodity directives parse without error and set formatting.
#[test]
fn sample_hledger_commodity_directives_parse() {
    let content =
        "commodity $1,000.00\n\n2024-01-01 * Test\n    expenses:food  $10\n    assets:cash\n";
    let f = tmp_file(content, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("10"), "amount should appear: {out}");
}

// -- specific entry forms ------------------------------------------------------

/// Simple two-posting cleared transaction (entry form 1).
#[test]
fn entry_form_simple_cleared() {
    let input = "\
2024-01-15 * Opening Balances
    assets:bank:checking          $1000.00
    equity:opening-balances
";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("1000"), "balance should contain 1000: {out}");
    assert!(
        out.contains("assets:bank:checking"),
        "account should appear: {out}"
    );
}

/// Pending transaction with code and posting comment (entry form 2).
#[test]
fn entry_form_pending_with_code() {
    let input = "\
2024-01-16 ! (INV-42) ACME Corp  ; project:website
    expenses:consulting           $500.00 ; vendor: ACME
    assets:bank:checking
";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("500"), "balance should contain 500: {out}");
}

/// Balance assertion posting (entry form 3): parses without error.
///
/// The assertion `$0.00 = $500.00` means "after this posting the account
/// balance should be $500".  We prime the account with a prior transaction
/// so the assertion passes during elaboration.
#[test]
fn entry_form_balance_assertion() {
    let input = "\
2024-01-15 * Opening Balances
    assets:bank:checking          $500.00
    equity:opening-balances

2024-01-31 * end-of-month check
    assets:bank:checking            $0.00 = $500.00
    expenses:adjustments
";
    let f = tmp_file(input, ".hledger");
    run(&["balance", f.path().to_str().unwrap(), "--flat"]);
}

/// Strict balance assertion `==` (entry form 4): parses without error.
///
/// Same as the weak assertion test -- prime the account first so the
/// assertion holds during elaboration.
#[test]
fn entry_form_strict_balance_assertion() {
    let input = "\
2024-01-15 * Opening Balances
    assets:bank:checking          $500.00
    equity:opening-balances

2024-02-01 * checked
    assets:bank:checking            $0.00 == $500.00
    expenses:adjustments
";
    let f = tmp_file(input, ".hledger");
    run(&["balance", f.path().to_str().unwrap(), "--flat"]);
}

/// Balance assignment `= target` with no LHS amount (entry form 5).
#[test]
fn entry_form_balance_assignment() {
    let input = "\
2024-02-05 * Reset to known balance
    assets:bank:checking          = $750.00
    income:adjustments
";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    // The assignment is elaborated: checking becomes $750, income becomes -$750.
    assert!(
        out.contains("750"),
        "balance assignment amount should appear: {out}"
    );
}

/// Lot pricing `@` unit price (entry form 6).
#[test]
fn entry_form_lot_unit_price() {
    let input = "\
2024-02-10 * Buy euros
    assets:eur                  100.00 EUR @ $1.10
    assets:bank:checking         $-110.00
";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("EUR"), "EUR commodity should appear: {out}");
}

/// Lot pricing `@@` total price (entry form 7).
#[test]
fn entry_form_lot_total_price() {
    let input = "\
2024-02-11 * Buy stock
    assets:brokerage             10 AAPL @@ $1825.00
    assets:bank:checking        $-1825.00
";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("AAPL"), "AAPL commodity should appear: {out}");
}

/// Periodic transaction (entry form 8): silently ignored, does not break balance.
#[test]
fn entry_form_periodic_transaction_ignored() {
    let input = "\
~ monthly  Rent
    expenses:rent                $2000
    assets:bank:checking

2024-01-01 * Real
    expenses:food  $10
    assets:cash
";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    // Only the real transaction's amounts should appear.
    assert!(
        out.contains("10"),
        "real transaction amount should appear: {out}"
    );
    // $2000 from the periodic transaction must NOT appear.
    assert!(
        !out.contains("2000"),
        "periodic transaction amount must not appear: {out}"
    );
}

/// hledger `/` date separator is accepted (entry form 9 variant).
#[test]
fn entry_form_slash_date_separator() {
    let input = "2024/03/15 * Groceries\n    expenses:food  $30\n    assets:cash\n";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("30"), "amount should appear: {out}");
}

/// hledger `.` date separator is accepted (entry form 10 variant).
#[test]
fn entry_form_dot_date_separator() {
    let input = "2024.06.01 * Salary\n    income:salary  $-5000\n    assets:bank\n";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("5000"), "amount should appear: {out}");
}

/// Comment lines starting with `#` parse cleanly (entry form 11).
#[test]
fn entry_form_hash_comment() {
    let input =
        "# This is a hash comment\n2024-01-01 * Test\n    expenses:food  $10\n    assets:cash\n";
    let f = tmp_file(input, ".hledger");
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("10"), "amount should appear: {out}");
}

// -- automated posting rule stub ----------------------------------------------─

/// An automated posting rule with a plain account posting (no `*N` arithmetic)
/// is silently ignored -- it should not crash the parser.
///
/// Note: `*N` multiplier bodies are stubbed out (TODO #103).
#[test]
fn auto_rule_without_arithmetic_body_is_ignored() {
    let input = "\
= expenses:groceries
    (budget:groceries)           10

2024-01-01 * Groceries
    expenses:groceries  $10
    assets:cash
";
    let f = tmp_file(input, ".hledger");
    // Auto rules are ignored; only the real transaction contributes to balance.
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(out.contains("10"), "real transaction should appear: {out}");
    // The automated posting itself must NOT appear in the balance.
    assert!(
        !out.contains("budget"),
        "automated posting must not be elaborated: {out}"
    );
}

// -- date formats --------------------------------------------------------------

/// `dop balance` on a minimal `.hledger` file exits 0 (pure end-to-end check).
#[test]
fn end_to_end_balance_exits_zero() {
    let input = "2024-01-01 * Salary\n    income:salary  $-1000\n    assets:checking  $1000\n";
    let f = tmp_file(input, ".hledger");
    run(&["balance", f.path().to_str().unwrap()]);
}

/// `dop balance` on a `.journal` extension exits 0.
#[test]
fn end_to_end_journal_extension_balance() {
    let input = "2024-01-01 * Salary\n    income:salary  $-1000\n    assets:checking  $1000\n";
    let f = tmp_file(input, ".journal");
    run(&["balance", f.path().to_str().unwrap()]);
}

/// An invalid hledger file produces a non-zero exit code.
#[test]
fn invalid_hledger_file_returns_error() {
    // A truncated header that cannot parse (missing year's century).
    let input = "24-01-01 Bad date\n    expenses:food  $10\n    assets:cash\n";
    let f = tmp_file(input, ".hledger");
    let stderr = run_fail(&["balance", f.path().to_str().unwrap()]);
    assert!(
        !stderr.is_empty(),
        "error output should explain the failure: {stderr}"
    );
}

// -- helpers ------------------------------------------------------------------─

/// Load sample.hledger, strip the include directive (which would attempt I/O).
fn fixture_without_includes() -> String {
    let path = sample_hledger_path();
    let content = std::fs::read_to_string(path).expect("read sample fixture");
    content
        .lines()
        .filter(|l| !l.trim_start().starts_with("include"))
        .map(|l| format!("{l}\n"))
        .collect()
}
