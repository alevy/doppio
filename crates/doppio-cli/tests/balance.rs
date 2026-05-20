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

/// Run `dop` with the given args, returning raw bytes so we can check for
/// ANSI escape sequences.
fn run_bytes(args: &[&str]) -> Vec<u8> {
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
    out.stdout
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
    // `format $1,000.00` -> prefix `$`, thousands `,`, 2 decimal places.
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
    // `nomarket` is a flag -- it should not affect output or cause an error.
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
    // `format 1.000,00 EUR` -> suffix ` EUR`, thousands `.`, decimal `,`, 2 places.
    let content = "commodity EUR
    format 1.000,00 EUR

2024-01-01 Invoice
    Income:Consulting  -2500 EUR
    Assets:Bank  2500 EUR
";
    let f = tmp_journal_file(content);
    let out = run(&["register", f.path().to_str().unwrap()]);
    // The register output should show `2,500.00 EUR` ... but the format uses
    // `.` as thousands sep and `,` as decimal -- so it should be `2.500,00 EUR`.
    assert!(
        out.contains("2.500,00 EUR"),
        "formatted amount should appear as '2.500,00 EUR': {out}"
    );
}

// ---------------------------------------------------------------------------
// --real / -R flag: filter out virtual postings
// ---------------------------------------------------------------------------

fn virtual_journal() -> &'static str {
    "2024-01-15 Setup
    Assets:Checking         $100
    Equity:Opening         $-100
    (Equity:Reservations)   $-25
"
}

#[test]
fn balance_real_flag_excludes_virtual_unbalanced() {
    let f = tmp_journal_file(virtual_journal());
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat", "--real"]);
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
fn balance_without_real_flag_includes_virtual_unbalanced() {
    let f = tmp_journal_file(virtual_journal());
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(
        out.contains("Equity:Reservations"),
        "virtual posting should be included by default: {out}"
    );
}

fn virtual_balanced_journal() -> &'static str {
    "2024-01-15 Setup
    Assets:Checking          $100
    [Equity:Reservations]    $25
    Equity:Opening          $-125
"
}

#[test]
fn balance_real_flag_excludes_virtual_balanced() {
    let f = tmp_journal_file(virtual_balanced_journal());
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat", "--real"]);
    assert!(
        !out.contains("Equity:Reservations"),
        "virtual balanced posting should be hidden with --real: {out}"
    );
}

#[test]
fn commodity_format_single_separator_three_digits_is_thousands() {
    // `format $1.000` -- a single separator followed by exactly 3 digits is a
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

#[test]
fn balance_exchange_missing_rate_leaves_amount_native() {
    // No price directive for GBP→USD.  `dop balance -X USD` should keep the
    // GBP posting in its native currency and emit a warning to stderr — it
    // must NOT silently drop or zero the amount.
    let content = "2024-01-15 London expense
    Expenses:Travel  50 GBP
    Assets:Checking
";
    let f = tmp_journal_file(content);
    let bin = env!("CARGO_BIN_EXE_dop");
    let result = std::process::Command::new(bin)
        .args([
            "balance",
            f.path().to_str().unwrap(),
            "--flat",
            "--exchange",
            "USD",
        ])
        .output()
        .expect("failed to run dop");
    // The command should succeed (exit 0) even when conversion is unavailable.
    assert!(
        result.status.success(),
        "dop should exit successfully even when no FX path exists: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let out = String::from_utf8(result.stdout).expect("non-UTF-8 stdout");
    // The native commodity GBP must appear — the amount is kept unconverted.
    assert!(
        out.contains("GBP"),
        "unconvertible commodity GBP should remain in output: {out}"
    );
    assert!(
        out.contains("50"),
        "unconverted amount 50 should appear in output: {out}"
    );
    // USD must not appear — there was no conversion.
    assert!(
        !out.contains("USD"),
        "target commodity USD should be absent when no FX path exists: {out}"
    );
    // A warning should have been emitted to stderr.
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("GBP"),
        "stderr warning should mention the unconverted commodity GBP: {stderr}"
    );
}

#[test]
fn balance_exchange_partial_conversion_mixed_total() {
    // EUR has a USD price directive; GBP does not.
    // After `-X USD`, the total should have both USD (from EUR) and GBP (residual).
    let content = "P 2024-01-01 EUR USD 1.10

2024-01-10 EUR expense
    Expenses:Foreign  100 EUR
    Assets:Checking

2024-01-11 GBP expense
    Expenses:UK  50 GBP
    Assets:Checking
";
    let f = tmp_journal_file(content);
    let bin = env!("CARGO_BIN_EXE_dop");
    let result = std::process::Command::new(bin)
        .args([
            "balance",
            f.path().to_str().unwrap(),
            "--flat",
            "--exchange",
            "USD",
        ])
        .output()
        .expect("failed to run dop");
    assert!(
        result.status.success(),
        "dop should exit successfully with partial conversion: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let out = String::from_utf8(result.stdout).expect("non-UTF-8 stdout");
    // EUR was converted → USD should appear (110 USD from 100 EUR × 1.10).
    assert!(
        out.contains("USD"),
        "converted commodity USD should appear in output: {out}"
    );
    assert!(
        out.contains("110"),
        "converted amount 110 USD should appear in output: {out}"
    );
    // GBP had no path → it remains native in the output.
    assert!(
        out.contains("GBP"),
        "unconverted commodity GBP should remain in output: {out}"
    );
    assert!(
        out.contains("50"),
        "unconverted amount 50 GBP should appear in output: {out}"
    );
    // The grand-total footer should show both commodities — USD for the
    // converted EUR lines and GBP for the residual line.
    let lines: Vec<&str> = out.lines().collect();
    let sep_idx = lines.iter().position(|l| l.starts_with("----"));
    assert!(
        sep_idx.is_some(),
        "grand-total separator line should be present: {out}"
    );
    // Lines after the separator are the grand total.
    let footer: Vec<&str> = sep_idx.map(|i| lines[i + 1..].to_vec()).unwrap_or_default();
    let footer_str = footer.join("\n");
    assert!(
        footer_str.contains("USD"),
        "grand total should include USD for converted amounts: {footer_str}"
    );
    assert!(
        footer_str.contains("GBP"),
        "grand total should include GBP residual for unconverted amounts: {footer_str}"
    );
}

// ---------------------------------------------------------------------------
// Tree-mode balance rollup (refs #216)
// ---------------------------------------------------------------------------

/// Journal with two leaf accounts under a common parent that has no direct
/// postings. Tree mode should show the parent row with the rolled-up subtotal.
fn bank_hierarchy_journal() -> &'static str {
    "2024-01-01 Checking deposit
    Assets:Bank:Checking  100 USD
    Income:Salary

2024-01-02 Savings deposit
    Assets:Bank:Savings  200 USD
    Income:Salary
"
}

#[test]
fn tree_mode_parent_shows_rolled_up_subtotal() {
    let f = tmp_journal_file(bank_hierarchy_journal());
    let out = run(&["balance", f.path().to_str().unwrap()]);

    // The "Bank" row (Assets:Bank) should show 300 USD — the sum of Checking
    // and Savings — even though Assets:Bank has no direct postings.
    assert!(
        out.contains("300"),
        "parent row should carry rolled-up subtotal (300 USD): {out}"
    );
    // Leaf rows still show their own direct balances.
    assert!(
        out.contains("100"),
        "Checking leaf should show 100 USD: {out}"
    );
    assert!(
        out.contains("200"),
        "Savings leaf should show 200 USD: {out}"
    );
    // The parent account label appears in the output.
    assert!(
        out.contains("Bank"),
        "parent account 'Bank' label should appear in tree output: {out}"
    );
}

#[test]
fn tree_mode_parent_row_appears_even_with_no_direct_postings() {
    let f = tmp_journal_file(bank_hierarchy_journal());
    let out = run(&["balance", f.path().to_str().unwrap()]);
    // `Bank` is an intermediate node with no direct postings; it must appear
    // as its own row (not be silently skipped).
    let bank_count = out.lines().filter(|l| l.contains("Bank")).count();
    assert!(
        bank_count >= 1,
        "at least one row for 'Bank' (the parent) should appear: {out}"
    );
}

#[test]
fn flat_mode_shows_only_direct_balances() {
    let f = tmp_journal_file(bank_hierarchy_journal());
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);

    // Flat mode should not show a synthetic Assets:Bank row (no direct postings).
    assert!(
        !out.contains("Assets:Bank\n") && !out.contains("Assets:Bank "),
        "flat mode should not synthesise a parent row for Assets:Bank: {out}"
    );
    // The full account names should appear.
    assert!(
        out.contains("Assets:Bank:Checking"),
        "full account name Assets:Bank:Checking should appear in flat mode: {out}"
    );
    assert!(
        out.contains("Assets:Bank:Savings"),
        "full account name Assets:Bank:Savings should appear in flat mode: {out}"
    );
    // The leaf values are the direct balances, not rolled-up subtotals.
    assert!(
        out.contains("100"),
        "Checking balance 100 should appear in flat mode: {out}"
    );
    assert!(
        out.contains("200"),
        "Savings balance 200 should appear in flat mode: {out}"
    );
    // Rolled-up 300 should NOT appear as a separate synthetic row — though it
    // may appear if any individual account happens to have 300 USD directly.
    // This test is intentionally loose on that point; the key check is that no
    // synthetic parent row is emitted.
}

#[test]
fn tree_mode_multi_commodity_parent_rollup() {
    let content = "2024-01-01 USD deposit
    Assets:Bank:Checking  100 USD
    Income:Salary

2024-01-02 EUR deposit
    Assets:Bank:Savings  200 EUR
    Income:Consulting
";
    let f = tmp_journal_file(content);
    let out = run(&["balance", f.path().to_str().unwrap()]);

    // The parent row (Assets:Bank / Bank) should show both 100 USD and 200 EUR.
    assert!(
        out.contains("100") && out.contains("200"),
        "parent row should carry both commodity totals: {out}"
    );
    assert!(
        out.contains("USD"),
        "USD commodity should appear in rolled-up parent: {out}"
    );
    assert!(
        out.contains("EUR"),
        "EUR commodity should appear in rolled-up parent: {out}"
    );
}

// ---------------------------------------------------------------------------
// --color flag: ANSI color output (refs #216)
// ---------------------------------------------------------------------------

/// Journal with a negative balance (Income account) and a positive balance
/// (Assets account) — enough to exercise both the red-negative and
/// blue-account-name color paths.
fn color_test_journal() -> &'static str {
    "2024-01-01 Salary
    Assets:Checking  $2000.00
    Income:Salary   $-2000.00
"
}

#[test]
fn color_always_emits_ansi_escapes() {
    let f = tmp_journal_file(color_test_journal());
    let bytes = run_bytes(&[
        "--color=always",
        "balance",
        f.path().to_str().unwrap(),
        "--flat",
    ]);

    // The ANSI escape introducer ESC[ must appear at least once.
    assert!(
        bytes.windows(2).any(|w| w == b"\x1b["),
        "--color=always should emit ANSI escape sequences"
    );
}

#[test]
fn color_always_colors_negative_amounts_red() {
    let f = tmp_journal_file(color_test_journal());
    let bytes = run_bytes(&[
        "--color=always",
        "balance",
        f.path().to_str().unwrap(),
        "--flat",
    ]);
    let output = String::from_utf8(bytes).expect("non-UTF-8 stdout");

    // Red is ANSI code 31; the negative amount line must contain \x1b[31m.
    assert!(
        output.contains("\x1b[31m"),
        "--color=always should render negative amounts in red (\\x1b[31m): {output}"
    );
}

#[test]
fn color_always_colors_account_names_blue() {
    let f = tmp_journal_file(color_test_journal());
    let bytes = run_bytes(&[
        "--color=always",
        "balance",
        f.path().to_str().unwrap(),
        "--flat",
    ]);
    let output = String::from_utf8(bytes).expect("non-UTF-8 stdout");

    // Blue is ANSI code 34; account names must be wrapped.
    assert!(
        output.contains("\x1b[34m"),
        "--color=always should render account names in blue (\\x1b[34m): {output}"
    );
}

#[test]
fn color_never_suppresses_ansi_escapes() {
    let f = tmp_journal_file(color_test_journal());
    let bytes = run_bytes(&[
        "--color=never",
        "balance",
        f.path().to_str().unwrap(),
        "--flat",
    ]);

    assert!(
        !bytes.windows(2).any(|w| w == b"\x1b["),
        "--color=never should produce plain ASCII with no ANSI escapes"
    );
}

#[test]
fn color_auto_with_no_color_env_suppresses_ansi() {
    // NO_COLOR=1 with --color=auto (the default) must suppress color even if
    // forced through an otherwise color-capable path.
    let f = tmp_journal_file(color_test_journal());
    let bin = env!("CARGO_BIN_EXE_dop");
    let out = Command::new(bin)
        .args(["balance", f.path().to_str().unwrap(), "--flat"])
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run dop");
    assert!(
        out.status.success(),
        "dop should exit successfully: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.stdout.windows(2).any(|w| w == b"\x1b["),
        "NO_COLOR=1 should suppress ANSI escapes in --color=auto mode"
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
fn balance_single_pattern_matches_only_that_account() {
    let f = tmp_journal_file(multi_account_journal());
    let out = run(&["balance", f.path().to_str().unwrap(), "Checking", "--flat"]);
    assert!(
        out.contains("Assets:Checking"),
        "Checking account should appear: {out}"
    );
    assert!(
        !out.contains("Expenses:Food"),
        "Food account should be excluded: {out}"
    );
    assert!(
        !out.contains("Income:Salary"),
        "Income account should be excluded: {out}"
    );
}

#[test]
fn balance_multiple_patterns_match_any_account() {
    let f = tmp_journal_file(multi_account_journal());
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "Checking",
        "Food",
        "--flat",
    ]);
    assert!(
        out.contains("Assets:Checking"),
        "Checking account should appear: {out}"
    );
    assert!(
        out.contains("Expenses:Food"),
        "Food account should appear: {out}"
    );
    assert!(
        !out.contains("Income:Salary"),
        "Income account should be excluded by multi-pattern filter: {out}"
    );
}

#[test]
fn balance_no_patterns_shows_all_accounts() {
    let f = tmp_journal_file(multi_account_journal());
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(
        out.contains("Assets:Checking"),
        "Checking should appear with no patterns: {out}"
    );
    assert!(
        out.contains("Expenses:Food"),
        "Food should appear with no patterns: {out}"
    );
    assert!(
        out.contains("Income:Salary"),
        "Income should appear with no patterns: {out}"
    );
}

#[test]
fn balance_invalid_pattern_exits_with_error() {
    let f = tmp_journal_file(multi_account_journal());
    let bin = env!("CARGO_BIN_EXE_dop");
    let result = std::process::Command::new(bin)
        .args(["balance", f.path().to_str().unwrap(), "[invalid", "--flat"])
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
