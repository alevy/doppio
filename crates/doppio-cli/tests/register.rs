//! Integration tests for the `register` subcommand's `--begin`, `--end`, and
//! `--cleared` flags.
//!
//! Each test writes a small `.ledger` file to a temporary directory, invokes
//! the binary under test via `std::process::Command`, and asserts on stdout.

use std::{io::Write as _, process::Command};

/// Write `content` to a temporary file and return the path.
fn tmp_journal_file(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::with_suffix(".ledger").expect("tempfile");
    f.write_all(content.as_bytes()).expect("write");
    f
}

/// Run the `dop` binary with the given arguments and return stdout as a
/// `String`. Panics if the process exits non-zero.
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

// ---------------------------------------------------------------------------
// --begin / --end filtering
// ---------------------------------------------------------------------------

/// Source journal with three transactions on three distinct dates.
fn three_transaction_journal() -> String {
    "2024-01-01 Early transaction
    Expenses:Food  10 USD
    Assets:Checking

2024-06-15 Middle transaction
    Expenses:Food  20 USD
    Assets:Checking

2024-12-31 Late transaction
    Expenses:Food  30 USD
    Assets:Checking
"
    .to_string()
}

#[test]
fn register_no_filter_shows_all_transactions() {
    let f = tmp_journal_file(&three_transaction_journal());
    let out = run(&["register", f.path().to_str().unwrap()]);
    assert!(out.contains("2024-01-01"), "expected early txn: {out}");
    assert!(out.contains("2024-06-15"), "expected middle txn: {out}");
    assert!(out.contains("2024-12-31"), "expected late txn: {out}");
}

#[test]
fn register_begin_excludes_earlier_transactions() {
    let f = tmp_journal_file(&three_transaction_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "--begin",
        "2024-06-15",
    ]);
    assert!(
        !out.contains("2024-01-01"),
        "early txn should be excluded: {out}"
    );
    assert!(
        out.contains("2024-06-15"),
        "middle txn should be present: {out}"
    );
    assert!(
        out.contains("2024-12-31"),
        "late txn should be present: {out}"
    );
}

#[test]
fn register_end_excludes_later_transactions() {
    let f = tmp_journal_file(&three_transaction_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "--end",
        "2024-06-15",
    ]);
    assert!(
        out.contains("2024-01-01"),
        "early txn should be present: {out}"
    );
    assert!(
        out.contains("2024-06-15"),
        "middle txn should be present: {out}"
    );
    assert!(
        !out.contains("2024-12-31"),
        "late txn should be excluded: {out}"
    );
}

#[test]
fn register_begin_and_end_restrict_to_window() {
    let f = tmp_journal_file(&three_transaction_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "--begin",
        "2024-06-15",
        "--end",
        "2024-06-15",
    ]);
    assert!(
        !out.contains("2024-01-01"),
        "early txn should be excluded: {out}"
    );
    assert!(
        out.contains("2024-06-15"),
        "middle txn should be present: {out}"
    );
    assert!(
        !out.contains("2024-12-31"),
        "late txn should be excluded: {out}"
    );
}

#[test]
fn register_begin_after_all_dates_returns_empty() {
    let f = tmp_journal_file(&three_transaction_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "--begin",
        "2025-01-01",
    ]);
    assert!(out.trim().is_empty(), "expected no output: {out}");
}

#[test]
fn register_end_before_all_dates_returns_empty() {
    let f = tmp_journal_file(&three_transaction_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "--end",
        "2023-12-31",
    ]);
    assert!(out.trim().is_empty(), "expected no output: {out}");
}

// ---------------------------------------------------------------------------
// --cleared filtering
// ---------------------------------------------------------------------------

fn mixed_cleared_journal() -> String {
    "2024-03-01 * Cleared transaction
    Expenses:Rent  1000 USD
    Assets:Checking

2024-03-15 Pending transaction
    Expenses:Food  50 USD
    Assets:Checking
"
    .to_string()
}

#[test]
fn register_cleared_shows_only_cleared_transactions() {
    let f = tmp_journal_file(&mixed_cleared_journal());
    let out = run(&["register", f.path().to_str().unwrap(), "--cleared"]);
    assert!(
        out.contains("Cleared transaction"),
        "cleared txn missing: {out}"
    );
    assert!(
        !out.contains("Pending transaction"),
        "pending txn should be absent: {out}"
    );
}

#[test]
fn register_without_cleared_shows_all_transactions() {
    let f = tmp_journal_file(&mixed_cleared_journal());
    let out = run(&["register", f.path().to_str().unwrap()]);
    assert!(
        out.contains("Cleared transaction"),
        "cleared txn missing: {out}"
    );
    assert!(
        out.contains("Pending transaction"),
        "pending txn missing: {out}"
    );
}

// ---------------------------------------------------------------------------
// Combinations: --begin + --cleared, --end + --cleared
// ---------------------------------------------------------------------------

fn multi_filter_journal() -> String {
    "2024-01-10 * Cleared early
    Expenses:Food  100 USD
    Assets:Checking

2024-01-20 Pending early
    Expenses:Food  200 USD
    Assets:Checking

2024-02-10 * Cleared late
    Expenses:Food  300 USD
    Assets:Checking

2024-02-20 Pending late
    Expenses:Food  400 USD
    Assets:Checking
"
    .to_string()
}

#[test]
fn register_begin_and_cleared_combined() {
    let f = tmp_journal_file(&multi_filter_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "--begin",
        "2024-02-01",
        "--cleared",
    ]);
    assert!(
        !out.contains("Cleared early"),
        "early cleared should be excluded: {out}"
    );
    assert!(
        !out.contains("Pending early"),
        "early pending should be excluded: {out}"
    );
    assert!(
        out.contains("Cleared late"),
        "late cleared should be present: {out}"
    );
    assert!(
        !out.contains("Pending late"),
        "late pending should be excluded: {out}"
    );
}

#[test]
fn register_end_and_cleared_combined() {
    let f = tmp_journal_file(&multi_filter_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "--end",
        "2024-01-31",
        "--cleared",
    ]);
    assert!(
        out.contains("Cleared early"),
        "early cleared should be present: {out}"
    );
    assert!(
        !out.contains("Pending early"),
        "early pending should be excluded: {out}"
    );
    assert!(
        !out.contains("Cleared late"),
        "late cleared should be excluded: {out}"
    );
    assert!(
        !out.contains("Pending late"),
        "late pending should be excluded: {out}"
    );
}

// ---------------------------------------------------------------------------
// Running total correctness with --begin
// ---------------------------------------------------------------------------

#[test]
fn register_begin_filter_resets_running_total() {
    // With --begin 2024-06-15, only the last two transactions appear.
    // The running total for the first shown row should be 20 (not 30 = 10+20).
    let f = tmp_journal_file(&three_transaction_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "Expenses",
        "--begin",
        "2024-06-15",
    ]);
    // The first visible posting has amount 20, running total should start at 20.
    assert!(
        out.contains("USD 20"),
        "expected first running total of 20: {out}"
    );
    // The running total should not include the excluded 10 USD from Jan.
    // If it did, the first line would show 30 as the running total.
    let lines: Vec<&str> = out.lines().collect();
    assert!(!lines.is_empty(), "expected output lines");
    // First line should show running total = 20, not 30.
    assert!(
        !lines[0].contains("USD 30"),
        "running total should not include excluded transaction: {out}"
    );
}

// ---------------------------------------------------------------------------
// --real / -R flag: filter out virtual postings
// ---------------------------------------------------------------------------

fn virtual_register_journal() -> &'static str {
    "2024-01-15 Setup
    Assets:Checking         $100
    Equity:Opening         $-100
    (Equity:Reservations)   $-25
"
}

#[test]
fn register_without_real_flag_includes_virtual_unbalanced() {
    let f = tmp_journal_file(virtual_register_journal());
    let out = run(&["register", f.path().to_str().unwrap()]);
    assert!(
        out.contains("Equity:Reservations"),
        "virtual posting should appear by default: {out}"
    );
    assert!(
        out.contains("Assets:Checking"),
        "real posting should appear: {out}"
    );
}

#[test]
fn register_real_flag_excludes_virtual_unbalanced() {
    let f = tmp_journal_file(virtual_register_journal());
    let out = run(&["register", f.path().to_str().unwrap(), "--real"]);
    assert!(
        !out.contains("Equity:Reservations"),
        "virtual unbalanced posting should be hidden with --real: {out}"
    );
    assert!(
        out.contains("Assets:Checking"),
        "real posting should still appear: {out}"
    );
}

#[test]
fn register_real_short_flag_excludes_virtual_unbalanced() {
    // -R is the short form of --real; verify it works identically.
    let f = tmp_journal_file(virtual_register_journal());
    let out = run(&["register", f.path().to_str().unwrap(), "-R"]);
    assert!(
        !out.contains("Equity:Reservations"),
        "virtual unbalanced posting should be hidden with -R: {out}"
    );
    assert!(
        out.contains("Assets:Checking"),
        "real posting should still appear with -R: {out}"
    );
}

// ---------------------------------------------------------------------------
// --tag filtering
// ---------------------------------------------------------------------------

fn tagged_journal() -> String {
    "2024-01-01 Tagged transaction
    ; :payroll:
    Expenses:Salary  100 USD
    Assets:Checking

2024-02-01 Untagged transaction
    Expenses:Food  20 USD
    Assets:Checking
"
    .to_string()
}

#[test]
fn register_tag_includes_only_tagged_transactions() {
    let f = tmp_journal_file(&tagged_journal());
    let out = run(&["register", f.path().to_str().unwrap(), "--tag", "payroll"]);
    assert!(out.contains("Salary"), "expected tagged txn: {out}");
    assert!(
        !out.contains("Food"),
        "untagged txn should be excluded: {out}"
    );
}

#[test]
fn register_tag_matches_posting_level_tag() {
    // Tag attached to a posting (not the transaction header) should still
    // pass the --tag filter, per the union semantics.
    let content = "2024-01-01 Mixed entry
    Expenses:Salary  100 USD
        ; :payroll:
    Assets:Checking

2024-02-01 Untagged
    Expenses:Food  20 USD
    Assets:Checking
";
    let f = tmp_journal_file(content);
    let out = run(&["register", f.path().to_str().unwrap(), "--tag", "payroll"]);
    assert!(out.contains("Salary"), "expected posting-tagged txn: {out}");
    assert!(
        !out.contains("Food"),
        "untagged txn should be excluded: {out}"
    );
}

#[test]
fn register_tag_no_match_returns_empty() {
    let f = tmp_journal_file(&tagged_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "--tag",
        "nonexistent",
    ]);
    assert!(!out.contains("Salary"), "no tagged txn should match: {out}");
    assert!(
        !out.contains("Food"),
        "untagged txn should be excluded: {out}"
    );
}

// ---------------------------------------------------------------------------
// --exchange FX conversion
// ---------------------------------------------------------------------------

#[test]
fn register_exchange_converts_eur_to_usd_and_accumulates_running_total() {
    // A price directive `P 2024-01-01 EUR $ 1.10` declares that 1 EUR = $1.10.
    // Two postings of 100 EUR each should appear as $ 110 per posting, with a
    // running total of $ 220 after the second posting, when --exchange $ is
    // passed.  This exercises both the conversion dispatch and the running-
    // total accumulation under the converted commodity.
    let content = "P 2024-01-01 EUR $ 1.10

2024-01-15 First foreign purchase
    Expenses:Travel  100 EUR
    Assets:Checking

2024-02-15 Second foreign purchase
    Expenses:Travel  100 EUR
    Assets:Checking
";
    let f = tmp_journal_file(content);
    // Filter to Expenses only so the running total accumulates across both
    // travel postings (110 + 110 = 220) rather than resetting per account.
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "Expenses",
        "--exchange",
        "$",
    ]);
    // Each posting converts to 110 $; running total after two postings is 220.
    assert!(
        out.contains("110"),
        "converted posting amount 110 should appear in register output: {out}"
    );
    assert!(
        out.contains("220"),
        "accumulated running total 220 should appear in register output: {out}"
    );
    assert!(
        out.contains('$'),
        "target commodity '$' should appear in register output: {out}"
    );
    assert!(
        !out.contains("EUR"),
        "source commodity 'EUR' should be absent after conversion: {out}"
    );
}

// ---------------------------------------------------------------------------
// Multi-pattern account filtering
// ---------------------------------------------------------------------------

fn multi_account_journal() -> &'static str {
    "2024-01-01 Salary
    Assets:Checking  500 USD
    Income:Salary

2024-01-02 Groceries
    Expenses:Food  50 USD
    Assets:Checking

2024-01-03 Books
    Expenses:Books  30 USD
    Assets:Checking
"
}

#[test]
fn register_multiple_patterns_match_any_account() {
    let f = tmp_journal_file(multi_account_journal());
    let out = run(&[
        "register",
        f.path().to_str().unwrap(),
        "Checking",
        "Food",
    ]);
    assert!(
        out.contains("Assets:Checking"),
        "Checking postings should appear: {out}"
    );
    assert!(
        out.contains("Expenses:Food"),
        "Food postings should appear: {out}"
    );
    assert!(
        !out.contains("Income:Salary"),
        "Income postings should be excluded by multi-pattern filter: {out}"
    );
}

#[test]
fn register_no_patterns_shows_all_accounts() {
    let f = tmp_journal_file(multi_account_journal());
    let out = run(&["register", f.path().to_str().unwrap()]);
    assert!(
        out.contains("Assets:Checking"),
        "Checking postings should appear with no patterns: {out}"
    );
    assert!(
        out.contains("Expenses:Food"),
        "Food postings should appear with no patterns: {out}"
    );
    assert!(
        out.contains("Income:Salary"),
        "Income postings should appear with no patterns: {out}"
    );
}

#[test]
fn register_invalid_pattern_exits_with_error() {
    let f = tmp_journal_file(multi_account_journal());
    let bin = env!("CARGO_BIN_EXE_dop");
    let result = std::process::Command::new(bin)
        .args(["register", f.path().to_str().unwrap(), "[invalid"])
        .output()
        .expect("failed to run dop");
    assert!(
        !result.status.success(),
        "dop should exit non-zero for an invalid regex pattern"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("invalid"),
        "stderr should report an invalid pattern error: {stderr}"
    );
}
