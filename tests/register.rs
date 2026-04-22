//! Integration tests for the `register` subcommand's `--begin`, `--end`, and
//! `--cleared` flags.
//!
//! Each test writes a small `.ledger` file to a temporary directory, invokes
//! the binary under test via `std::process::Command`, and asserts on stdout.

use std::{io::Write as _, process::Command};

/// Write `content` to a temporary file and return the path.
fn tmp_ledger(content: &str) -> tempfile::NamedTempFile {
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

/// Source ledger with three transactions on three distinct dates.
fn three_transaction_ledger() -> String {
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
    let f = tmp_ledger(&three_transaction_ledger());
    let out = run(&["register", f.path().to_str().unwrap()]);
    assert!(out.contains("2024-01-01"), "expected early txn: {out}");
    assert!(out.contains("2024-06-15"), "expected middle txn: {out}");
    assert!(out.contains("2024-12-31"), "expected late txn: {out}");
}

#[test]
fn register_begin_excludes_earlier_transactions() {
    let f = tmp_ledger(&three_transaction_ledger());
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
    let f = tmp_ledger(&three_transaction_ledger());
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
    let f = tmp_ledger(&three_transaction_ledger());
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
    let f = tmp_ledger(&three_transaction_ledger());
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
    let f = tmp_ledger(&three_transaction_ledger());
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

fn mixed_cleared_ledger() -> String {
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
    let f = tmp_ledger(&mixed_cleared_ledger());
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
    let f = tmp_ledger(&mixed_cleared_ledger());
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

fn multi_filter_ledger() -> String {
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
    let f = tmp_ledger(&multi_filter_ledger());
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
    let f = tmp_ledger(&multi_filter_ledger());
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
    let f = tmp_ledger(&three_transaction_ledger());
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
