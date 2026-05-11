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
// Grand-total row (refs #216)
// ---------------------------------------------------------------------------

/// A simple single-commodity journal that sums to zero (fully balanced).
fn balanced_usd_journal() -> &'static str {
    "2024-01-01 Salary
    Assets:Checking  1000 USD
    Income:Salary  -1000 USD
"
}

#[test]
fn balance_grand_total_row_separator_appears() {
    let f = tmp_journal_file(balanced_usd_journal());
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    assert!(
        out.contains("--------------------"),
        "20-dash separator should appear in balance output: {out}"
    );
}

#[test]
fn balance_grand_total_row_appears_after_separator() {
    let f = tmp_journal_file(balanced_usd_journal());
    let out = run(&["balance", f.path().to_str().unwrap(), "--flat"]);
    let lines: Vec<&str> = out.lines().collect();
    let sep_idx = lines.iter().position(|l| l.starts_with("---"));
    assert!(sep_idx.is_some(), "separator must appear: {out}");
    let sep_idx = sep_idx.unwrap();
    assert!(
        sep_idx + 1 < lines.len(),
        "there must be at least one total row after the separator: {out}"
    );
}

#[test]
fn balance_grand_total_correct_single_commodity() {
    // Two Assets accounts summing to 300 USD, one Income summing to -300 USD.
    // With a filter on Assets only the total should be 300 USD.
    let content = "2024-01-01 Checking deposit
    Assets:Bank:Checking  100 USD
    Income:Salary

2024-01-02 Savings deposit
    Assets:Bank:Savings  200 USD
    Income:Salary
";
    let f = tmp_journal_file(content);
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "Assets",
        "--flat",
    ]);
    // Total should be 300 USD.
    assert!(
        out.contains("300"),
        "grand total should be 300 for filtered Assets: {out}"
    );
    assert!(
        out.contains("USD"),
        "commodity USD should appear in grand total: {out}"
    );
}

#[test]
fn balance_grand_total_tree_mode_uses_top_level_subtrees() {
    // In tree mode the grand total aggregates top-level account subtrees only,
    // avoiding double-counting of intermediate nodes.
    let content = "2024-01-01 Salary
    Assets:Bank:Checking  1000 USD
    Income:Salary
";
    let f = tmp_journal_file(content);
    let out = run(&["balance", f.path().to_str().unwrap()]);
    let lines: Vec<&str> = out.lines().collect();
    let sep_idx = lines
        .iter()
        .position(|l| l.starts_with("---"))
        .expect("separator should appear");
    // There should be exactly one total row and it should contain "0"
    // (Assets +1000, Income -1000 → net 0).
    let total_line = lines[sep_idx + 1];
    assert!(
        total_line.contains("0"),
        "balanced journal should have 0 total: {total_line}"
    );
}

#[test]
fn balance_grand_total_multi_commodity() {
    // A journal with two commodities that don't cancel: grand total should
    // show one line per commodity.
    let content = "2024-01-01 USD
    Assets:Cash:USD  100 USD
    Income:Salary

2024-01-02 EUR
    Assets:Cash:EUR  200 EUR
    Income:Consulting
";
    let f = tmp_journal_file(content);
    // Filter to Assets only so the total is nonzero.
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "Assets",
        "--flat",
    ]);
    assert!(
        out.contains("USD"),
        "USD commodity should appear in multi-commodity total: {out}"
    );
    assert!(
        out.contains("EUR"),
        "EUR commodity should appear in multi-commodity total: {out}"
    );
    // Both the 100 and 200 amounts should appear.
    assert!(out.contains("100"), "100 USD should appear in total: {out}");
    assert!(out.contains("200"), "200 EUR should appear in total: {out}");
}

#[test]
fn balance_no_total_when_no_accounts_match() {
    // When the pattern matches no accounts, neither a separator nor a total
    // row should appear — matching ledger-cli's behaviour.
    let f = tmp_journal_file(balanced_usd_journal());
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "NonExistentAccount",
        "--flat",
    ]);
    assert!(
        !out.contains("--------------------"),
        "separator must not appear when no accounts match: {out}"
    );
}

// ---------------------------------------------------------------------------
// --exchange missing-rate fallback
// ---------------------------------------------------------------------------

#[test]
fn balance_exchange_missing_rate_leaves_amount_native() {
    // When no P directive exists for a commodity-to-target pair, the amount
    // should be left in its native commodity (fallback, no error).
    let content = "2024-01-01 Purchase
    Expenses:Travel  100 GBP
    Assets:Checking
";
    let f = tmp_journal_file(content);
    // No P directive for GBP -> USD, so GBP should remain.
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "--flat",
        "--exchange",
        "USD",
    ]);
    assert!(
        out.contains("GBP"),
        "native commodity GBP should appear when no rate available: {out}"
    );
    assert!(
        out.contains("100"),
        "native amount 100 should appear: {out}"
    );
    // No USD should appear (since the only account has GBP with no rate).
    assert!(
        !out.contains("USD"),
        "target commodity USD should not appear when no rate exists: {out}"
    );
}

#[test]
fn balance_exchange_partial_conversion_mixed_total() {
    // A journal where one commodity has a rate and another doesn't.
    // The total row should have one line for the converted commodity and one
    // for the unconverted native commodity.
    let content = "P 2024-01-01 EUR USD 1.10

2024-01-15 EUR expense (convertible)
    Expenses:Travel  100 EUR
    Assets:Bank

2024-01-16 GBP expense (no rate)
    Expenses:Hotels  50 GBP
    Assets:Bank
";
    let f = tmp_journal_file(content);
    let out = run(&[
        "balance",
        f.path().to_str().unwrap(),
        "Expenses",
        "--flat",
        "--exchange",
        "USD",
    ]);
    // EUR should be converted to USD (110).
    assert!(out.contains("USD"), "converted USD should appear: {out}");
    assert!(out.contains("110"), "converted amount 110 should appear: {out}");
    // GBP should remain unconverted.
    assert!(out.contains("GBP"), "unconverted GBP should appear: {out}");
    assert!(out.contains("50"), "unconverted amount 50 should appear: {out}");
}

// ---------------------------------------------------------------------------
// --color flag
// ---------------------------------------------------------------------------

/// Helper: run `dop balance` and return raw stdout bytes (not UTF-8 decoded)
/// so we can check for ANSI escape sequences.
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

#[test]
fn balance_color_always_applies_red_to_negative_total() {
    // Force --color=always so ANSI codes appear even when stdout is not a TTY.
    // A journal filtered to show only Income (negative amounts) should have
    // red ANSI codes (\x1b[31m) in the total row.
    let content = "2024-01-01 Salary
    Assets:Checking  1000 USD
    Income:Salary  -1000 USD
";
    let f = tmp_journal_file(content);
    let bytes = run_bytes(&[
        "--color=always",
        "balance",
        f.path().to_str().unwrap(),
        "Income",
        "--flat",
    ]);
    // ANSI red: ESC [ 3 1 m
    let red_code: &[u8] = b"\x1b[31m";
    assert!(
        bytes.windows(red_code.len()).any(|w| w == red_code),
        "ANSI red escape sequence should appear for negative total with --color=always"
    );
}

#[test]
fn balance_color_never_strips_ansi_codes() {
    // --color=never must produce no ANSI escape sequences even for negative amounts.
    let content = "2024-01-01 Salary
    Assets:Checking  1000 USD
    Income:Salary  -1000 USD
";
    let f = tmp_journal_file(content);
    let bytes = run_bytes(&[
        "--color=never",
        "balance",
        f.path().to_str().unwrap(),
        "Income",
        "--flat",
    ]);
    assert!(
        !bytes.contains(&0x1b_u8),
        "no ESC byte should appear with --color=never"
    );
}
