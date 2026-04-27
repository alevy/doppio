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
