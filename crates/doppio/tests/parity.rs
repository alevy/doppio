//! Feature parity test corpus.
//!
//! One small ledger fixture per feature lives under
//! `tests/parity/fixtures/<feature>.ledger`. Each test in this file loads
//! its fixture, compiles it through the full parse → resolve → elaborate
//! pipeline, and asserts properties of the resulting
//! [`doppio::elaboration::Journal`].
//!
//! Tests for **implemented** features must pass on every run. Tests for
//! **not-yet-implemented** features are marked `#[ignore = "tracks #N"]`
//! and carry the spec — real assertions on the elaborated journal that
//! describe what should be true once the tracking issue closes. Most fail
//! today (parsing rejects the syntax, the elaborator diverges from spec,
//! or a schema field doesn't yet exist); the failure messages show the
//! implementer where their work needs to land. The ignored count stays a
//! visible signal of remaining parity work.
//!
//! See `docs/SUPPORTED_FEATURES.md` for the human-readable matrix and
//! the Phase D milestone for the in-flight gap-fills.

use chrono::NaiveDate;
use doppio::elaboration::Journal;
use rust_decimal::dec;
use std::path::PathBuf;

// ──────────────────────────────────────────────────────────────────────────
// helpers
// ──────────────────────────────────────────────────────────────────────────

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parity")
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn compile(name: &str) -> Journal {
    let src = fixture(name);
    let parser = doppio::parser::Parser {
        // No-op opener: parity fixtures are self-contained, no `include`.
        opener: |_: &str| Ok::<String, Box<dyn std::error::Error>>(String::new()),
        base_path: PathBuf::new(),
    };
    doppio::compile(&src, parser).expect("compile failed")
}

// ──────────────────────────────────────────────────────────────────────────
// Implemented features — must pass.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn transactions_basic() {
    let j = compile("transactions_basic.ledger");
    assert_eq!(j.transactions.len(), 1);
    let t = &j.transactions[0];
    assert_eq!(t.description, "Groceries");
    assert_eq!(t.postings.len(), 2);
    assert_eq!(t.postings[0].account, "Expenses:Food");
    assert_eq!(t.postings[0].amount_in("$"), Some(dec!(50)));
    // Null posting on Assets:Checking inferred as -$50.
    assert_eq!(t.postings[1].account, "Assets:Checking");
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-50)));
}

#[test]
fn multi_commodity() {
    let j = compile("multi_commodity.ledger");
    assert_eq!(j.transactions.len(), 1);
    let t = &j.transactions[0];
    assert_eq!(t.postings.len(), 2);
    // Brokerage side: 10 AAPL.
    assert_eq!(t.postings[0].account, "Assets:Brokerage");
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    // Cash side: -$1500. The transaction is balanced because the @ $150
    // cost annotation contributes $1500 to the balance state.
    assert_eq!(t.postings[1].account, "Assets:Cash");
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1500)));
}

#[test]
fn lot_pricing_unit() {
    let j = compile("lot_pricing_unit.ledger");
    assert_eq!(j.transactions.len(), 1);
    let t = &j.transactions[0];
    // 10 AAPL @ $150 → cash side null posting = -$1500.
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1500)));
}

#[test]
fn lot_pricing_total() {
    let j = compile("lot_pricing_total.ledger");
    assert_eq!(j.transactions.len(), 1);
    let t = &j.transactions[0];
    // 10 AAPL @@ $1500 → cash side null posting = -$1500.
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1500)));
}

#[test]
fn balance_assertion() {
    // The fixture's `== $1000` assertion enforces during elaboration; if
    // the assertion failed the compile would error. Reaching this assert
    // means the assertion passed.
    let j = compile("balance_assertion.ledger");
    assert_eq!(j.transactions.len(), 1);
    let t = &j.transactions[0];
    assert_eq!(t.postings[0].amount_in("$"), Some(dec!(1000)));
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1000)));
}

#[test]
fn balance_assignment() {
    // `Assets:Checking  = $1000` with no explicit amount: the elaborator
    // computes (target - current_balance) = ($1000 - $0) = $1000.
    let j = compile("balance_assignment.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings[0].account, "Assets:Checking");
    assert_eq!(t.postings[0].amount_in("$"), Some(dec!(1000)));
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1000)));
}

#[test]
fn account_assert() {
    // The account block's `assert commodity == "$"` is enforced for every
    // posting to Assets:Checking. The transaction's posting is in $, so
    // the assertion passes and elaboration succeeds.
    let j = compile("account_assert.ledger");
    assert_eq!(j.transactions.len(), 1);
    assert!(
        j.accounts.contains_key("Assets:Checking"),
        "account block should register Assets:Checking on the journal"
    );
}

#[test]
fn commodity_format() {
    let j = compile("commodity_format.ledger");
    let dollar = j.commodities.get("$").expect("$ commodity declared");
    // The format is captured on the commodity entry. (Application of the
    // format string is a render-time concern; here we just confirm
    // elaboration preserves it.)
    assert_eq!(dollar.format.as_deref(), Some("$1,000.00"));
}

#[test]
fn tag_check() {
    // `tag Statement / check value =~ /^foo/` — non-fatal warning if the
    // regex fails. Here it matches, so elaboration completes silently.
    let j = compile("tag_check.ledger");
    assert_eq!(j.transactions.len(), 1);
}

#[test]
fn define_param() {
    let j = compile("define_param.ledger");
    let t = &j.transactions[0];
    // double($50) = $50 * 2 = $100.
    let food = t
        .postings
        .iter()
        .find(|p| p.account == "Expenses:Food")
        .expect("food posting present");
    assert_eq!(food.amount_in("$"), Some(dec!(100)));
}

#[test]
fn historical_price_directive() {
    let j = compile("historical_price_directive.ledger");
    assert_eq!(j.prices.len(), 1, "one P directive parsed");
    let p = &j.prices[0];
    assert_eq!(p.commodity, "AAPL");
    assert_eq!(p.price_commodity, "$");
    assert_eq!(
        p.price.as_ref().expect("price set").to_decimal(),
        dec!(182.50)
    );
}

#[test]
fn metadata_inheritance() {
    // Transaction-header metadata `Statement: foobar` is visible to the
    // posting-level `tag()` lookup that the tag block's `assert` performs.
    // Reaching this point with no error means the assertion passed.
    let j = compile("metadata_inheritance.ledger");
    assert_eq!(j.transactions.len(), 1);
}

// ──────────────────────────────────────────────────────────────────────────
// Date / state / code / amount-form coverage.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn date_formats() {
    // Both `YYYY-MM-DD` and `YYYY/MM/DD` are accepted; the elaborated
    // dates are the same i32-epoch-days values regardless of the source
    // format.
    let j = compile("date_formats.ledger");
    assert_eq!(j.transactions.len(), 2);
    assert_eq!(
        j.transactions[0].date_naive(),
        chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()
    );
    assert_eq!(
        j.transactions[1].date_naive(),
        chrono::NaiveDate::from_ymd_opt(2024, 2, 20).unwrap()
    );
}

#[test]
fn secondary_date() {
    let j = compile("secondary_date.ledger");
    let t = &j.transactions[0];
    assert_eq!(
        t.date_naive(),
        chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()
    );
    assert_eq!(
        t.secondary_date_naive(),
        Some(chrono::NaiveDate::from_ymd_opt(2024, 1, 20).unwrap())
    );
}

#[test]
fn transaction_state() {
    use doppio::elaboration::TransactionState;
    let j = compile("transaction_state.ledger");
    assert_eq!(j.transactions.len(), 3);
    assert_eq!(j.transactions[0].state, TransactionState::Cleared as i32);
    assert_eq!(j.transactions[1].state, TransactionState::Pending as i32);
    assert_eq!(j.transactions[2].state, TransactionState::Uncleared as i32);
}

#[test]
fn transaction_code() {
    let j = compile("transaction_code.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.code.as_deref(), Some("INV-042"));
    assert_eq!(t.description, "Invoice paid");
}

#[test]
fn amount_forms() {
    let j = compile("amount_forms.ledger");
    assert_eq!(j.transactions.len(), 5);
    // Symbol-first positive: $100
    assert_eq!(
        j.transactions[0].postings[0].amount_in("$"),
        Some(dec!(100))
    );
    // Symbol-first leading minus: -$100
    assert_eq!(
        j.transactions[1].postings[0].amount_in("$"),
        Some(dec!(-100))
    );
    // Symbol-first inside minus: $-100
    assert_eq!(
        j.transactions[2].postings[0].amount_in("$"),
        Some(dec!(-100))
    );
    // Number-first positive: 100 USD
    assert_eq!(
        j.transactions[3].postings[0].amount_in("USD"),
        Some(dec!(100))
    );
    // Number-first negative: -100 USD
    assert_eq!(
        j.transactions[4].postings[0].amount_in("USD"),
        Some(dec!(-100))
    );
}

#[test]
fn posting_state() {
    use doppio::elaboration::TransactionState;
    let j = compile("posting_state.ledger");
    let postings = &j.transactions[0].postings;
    assert_eq!(postings.len(), 3);
    assert_eq!(postings[0].state, TransactionState::Cleared as i32);
    assert_eq!(postings[1].state, TransactionState::Pending as i32);
    // The null posting (the third one, no explicit marker) inherits the
    // transaction's state, which is Uncleared by default.
    assert_eq!(postings[2].state, TransactionState::Uncleared as i32);
}

// ──────────────────────────────────────────────────────────────────────────
// Comments / metadata / tags.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn transaction_notes() {
    // Header-level metadata + posting-level metadata both survive
    // elaboration. The `KeyA: ValueA` becomes a metadata entry on the
    // transaction; `KeyB: ValueB` on the posting.
    let j = compile("transaction_notes.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.metadata.get("KeyA").map(String::as_str), Some("ValueA"));
    let food = t
        .postings
        .iter()
        .find(|p| p.account == "Expenses:Food")
        .expect("food posting present");
    assert_eq!(
        food.metadata.get("KeyB").map(String::as_str),
        Some("ValueB")
    );
}

#[test]
fn bare_tag_list() {
    // The `; :urgent:reviewed:` line appears in the transaction-header
    // position (above any posting), so the tags attach to the
    // transaction, not to a specific posting.
    let j = compile("bare_tag_list.ledger");
    let t = &j.transactions[0];
    assert!(
        t.tags.iter().any(|s| s == "urgent"),
        "expected `urgent` tag on transaction, got {:?}",
        t.tags
    );
    assert!(
        t.tags.iter().any(|s| s == "reviewed"),
        "expected `reviewed` tag on transaction, got {:?}",
        t.tags
    );
}

#[test]
fn comment_chars() {
    // Top-level lines starting with ; # * % | are full-line comments.
    // Only the one real transaction in the fixture should make it into
    // the elaborated journal.
    let j = compile("comment_chars.ledger");
    assert_eq!(j.transactions.len(), 1);
    assert_eq!(j.transactions[0].description, "Real transaction");
}

// ──────────────────────────────────────────────────────────────────────────
// Directive completeness.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn account_check() {
    // Non-fatal `check` (vs the fatal `assert`). The fixture's commodity
    // matches, so no warning is emitted, but even on a failing check
    // elaboration would still complete.
    let j = compile("account_check.ledger");
    assert_eq!(j.transactions.len(), 1);
    assert!(j.accounts.contains_key("Assets:Checking"));
}

#[test]
fn account_note() {
    let j = compile("account_note.ledger");
    let brokerage = j
        .accounts
        .get("Assets:Brokerage")
        .expect("account block declared");
    assert_eq!(brokerage.note.as_deref(), Some("Schwab #1234"));
}

#[test]
fn commodity_default() {
    // After `commodity $ ; default`, a bare `100` should pick up `$`.
    let j = compile("commodity_default.ledger");
    let t = &j.transactions[0];
    let food = &t.postings[0];
    assert_eq!(food.amount_in("$"), Some(dec!(100)));
}

#[test]
fn top_level_alias() {
    // `alias Checking = Assets:Checking` — postings using `Checking`
    // resolve to the canonical `Assets:Checking` in the elaborated
    // journal.
    let j = compile("top_level_alias.ledger");
    let t = &j.transactions[0];
    let checking = t
        .postings
        .iter()
        .find(|p| p.account == "Assets:Checking")
        .expect("alias should resolve to Assets:Checking");
    assert_eq!(checking.amount_in("$"), Some(dec!(1000)));
}

#[test]
fn standalone_balance_assertion() {
    // `<date> = <account>  <amount>` — passes if the running balance at
    // that point matches. Reaching elaboration without error means the
    // assertion passed.
    let j = compile("standalone_balance_assertion.ledger");
    assert_eq!(j.transactions.len(), 1);
}

#[test]
fn define_zero_arg() {
    // `define monthly_rent = $1500.00` — body substituted at use site.
    let j = compile("define_zero_arg.ledger");
    let rent = j.transactions[0]
        .postings
        .iter()
        .find(|p| p.account == "Expenses:Rent")
        .expect("rent posting present");
    assert_eq!(rent.amount_in("$"), Some(dec!(1500.00)));
}

#[test]
fn budget_directive() {
    // `~` budget directives parse but are intentionally not elaborated;
    // the surrounding journal still elaborates as if the budget weren't
    // there. Only the real 2024-01-01 transaction should appear.
    let j = compile("budget_directive.ledger");
    assert_eq!(
        j.transactions.len(),
        1,
        "budget directive should not produce an elaborated transaction"
    );
    assert_eq!(j.transactions[0].description, "Real spending");
}

// ──────────────────────────────────────────────────────────────────────────
// Expressions.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn regex_match() {
    // `assert account =~ /^Expenses:/` in an account block. The posting
    // matches, so elaboration succeeds.
    let j = compile("regex_match.ledger");
    assert_eq!(j.transactions.len(), 1);
}

#[test]
fn arithmetic_expression() {
    // `($30 + $20)` should evaluate to $50.
    let j = compile("arithmetic_expression.ledger");
    let food = j.transactions[0]
        .postings
        .iter()
        .find(|p| p.account == "Expenses:Food")
        .expect("food posting present");
    assert_eq!(food.amount_in("$"), Some(dec!(50)));
}

// ──────────────────────────────────────────────────────────────────────────
// Multi-transaction state.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn running_balance() {
    // Three transactions accumulate $1400 in Assets:Checking; the
    // standalone balance assertion at month-end confirms the running
    // total. Exercises the elaborator's per-account balance bookkeeping
    // across multiple entries.
    let j = compile("running_balance.ledger");
    assert_eq!(j.transactions.len(), 3);
    // The fourth entry is the standalone assertion, not a transaction.
}

// ──────────────────────────────────────────────────────────────────────────
// Not-yet-implemented features. Each `#[ignore]` references its tracking
// issue. The test body carries the **expected** spec — real assertions on
// the elaborated journal that should pass once the feature lands. Today
// each test fails (some panic in `compile()` because the fixture won't
// parse; some panic in the assertions because current behavior diverges
// from spec). When the tracking issue closes, remove the `#[ignore]` and
// the test should turn green; if not, the assertions document where
// implementation diverged from the spec.
//
// Where a spec field requires a schema change that doesn't exist yet
// (e.g. `proto::Posting.lot` for #139, `proto::Posting.kind` for #140),
// the new-field assertion is left as a `// TODO(#N)` comment block. The
// developer landing the schema un-comments it as part of their PR.
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// Lot persistence — #139
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn lot_persistence_cost() {
    // Fixture: 10 AAPL {$150} @ $155.
    //
    // Cost basis ($150/share) is the historical lot annotation; price
    // ($155) is the actual transaction value. The cash side balances
    // against the price, not the cost — the `@` price wins over `{cost}`
    // when both are present.
    let j = compile("lot_persistence_cost.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings.len(), 2);
    assert_eq!(t.postings[0].account, "Assets:Brokerage");
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    // Cash side null posting: -(10 * $155) = -$1550. Price drives balance.
    assert_eq!(t.postings[1].account, "Assets:Cash");
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1550)));
    // Lot annotation preserved on the proto posting.
    let lot = t.postings[0].lot.as_ref().expect("lot annotation present");
    assert_eq!(lot.cost.as_ref().and_then(|a| a.get("$")), Some(dec!(150)));
}

#[test]
fn lot_persistence_date() {
    // Fixture: 10 AAPL {$150} [2024-03-01]. Cost + lot acquisition date.
    let j = compile("lot_persistence_date.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    // No `@ price` — cash side null posting balances against cost ($150 * 10 = $1500).
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1500)));
    let lot = t.postings[0].lot.as_ref().expect("lot annotation present");
    assert_eq!(lot.cost.as_ref().and_then(|a| a.get("$")), Some(dec!(150)));
    assert_eq!(
        t.postings[0].lot_date_naive(),
        Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()),
    );
}

#[test]
fn lot_persistence_note() {
    // Fixture: 10 AAPL {$150} ((BUY-2024-01)). Cost + free-form note.
    let j = compile("lot_persistence_note.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1500)));
    let lot = t.postings[0].lot.as_ref().expect("lot annotation present");
    assert_eq!(lot.cost.as_ref().and_then(|a| a.get("$")), Some(dec!(150)));
    assert_eq!(t.postings[0].lot_note(), Some("BUY-2024-01"));
}

#[test]
fn lot_persistence_combined() {
    // Fixture: 10 AAPL {$150} [2024-03-01] ((BUY-2024-01)).
    // All three annotations combined; cost drives cash balance (no @ price).
    let j = compile("lot_persistence_combined.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1500)));
    assert!(t.postings[0].has_lot());
    assert_eq!(
        t.postings[0].lot_cost_in("$"),
        Some(dec!(150)),
        "lot cost should be $150/share"
    );
    assert_eq!(
        t.postings[0].lot_date_naive(),
        Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap()),
    );
    assert_eq!(t.postings[0].lot_note(), Some("BUY-2024-01"));
}

#[test]
fn lot_persistence_cost_vs_price() {
    // Fixture: 10 AAPL {$150} @ $155.
    // Price ($155) drives the cash balance; cost ($150) is the lot basis.
    // This is the canonical cost-vs-price scenario: they differ because of
    // e.g. a non-cash acquisition or a price at time of split.
    let j = compile("lot_persistence_cost_vs_price.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings[0].account, "Assets:Brokerage");
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    // Price ($155/share) drives cash: -(10 * $155) = -$1550.
    assert_eq!(t.postings[1].account, "Assets:Cash");
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-1550)));
    // Lot cost annotation preserved as $150 (NOT $155).
    assert_eq!(
        t.postings[0].lot_cost_in("$"),
        Some(dec!(150)),
        "lot cost should be $150, not the price $155"
    );
}

#[test]
fn lot_persistence_date_only() {
    // Fixture: 10 AAPL [2024-01-15] — date annotation only, no cost.
    // Exercises the cost-fallback path: no {cost} or @ price means the
    // null posting balances in the posted commodity (AAPL contributes
    // itself as -10 AAPL). The lot's date field is still preserved.
    let j = compile("lot_persistence_date_only.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    // Cost-fallback: AAPL contributes itself, null posting = -10 AAPL.
    assert_eq!(t.postings[1].amount_in("AAPL"), Some(dec!(-10)));
    assert!(t.postings[0].has_lot(), "lot annotation should be present");
    assert_eq!(
        t.postings[0].lot_date_naive(),
        Some(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()),
    );
    assert_eq!(t.postings[0].lot_cost_in("$"), None, "no cost annotation");
    assert_eq!(t.postings[0].lot_note(), None, "no note annotation");
}

#[test]
fn lot_persistence_note_only() {
    // Fixture: 10 AAPL ((BUY-2024-01)) — note annotation only, no cost.
    // Exercises the cost-fallback path: no {cost} or @ price means the
    // null posting balances in AAPL. The lot's note field is preserved.
    let j = compile("lot_persistence_note_only.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings[0].amount_in("AAPL"), Some(dec!(10)));
    // Cost-fallback: null posting = -10 AAPL.
    assert_eq!(t.postings[1].amount_in("AAPL"), Some(dec!(-10)));
    assert!(t.postings[0].has_lot(), "lot annotation should be present");
    assert_eq!(t.postings[0].lot_note(), Some("BUY-2024-01"));
    assert_eq!(t.postings[0].lot_date_naive(), None, "no date annotation");
    assert_eq!(t.postings[0].lot_cost_in("$"), None, "no cost annotation");
}

#[test]
fn virtual_posting_unbalanced() {
    // Fixture has 3 postings: Assets:Checking $100, Equity:Opening -$100,
    // (Equity:Reservations) -$25. The parens mark the third as a virtual
    // *unbalanced* posting — excluded from the transaction's balance
    // check, so the two real postings must balance among themselves
    // (they do: +$100 + -$100 = 0).
    let j = compile("virtual_posting_unbalanced.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings.len(), 3);

    // Parens stripped from the account name on the elaborated posting.
    let virt = t
        .postings
        .iter()
        .find(|p| p.account == "Equity:Reservations")
        .expect("virtual posting account name should have parens stripped");
    assert_eq!(virt.amount_in("$"), Some(dec!(-25)));

    // Real postings balance among themselves.
    let real_sum: rust_decimal::Decimal = t
        .postings
        .iter()
        .filter(|p| p.account != "Equity:Reservations")
        .filter_map(|p| p.amount_in("$"))
        .sum();
    assert_eq!(real_sum, dec!(0), "real postings should balance to 0");

    // Virtual-unbalanced posting carries the correct kind field.
    use doppio::elaboration::PostingKind;
    assert_eq!(
        virt.kind,
        PostingKind::VirtualUnbalanced as i32,
        "virtual posting should carry PostingKind::VirtualUnbalanced"
    );

    // Real postings carry kind == Real.
    let real_postings: Vec<_> = t
        .postings
        .iter()
        .filter(|p| p.account != "Equity:Reservations")
        .collect();
    for p in &real_postings {
        assert_eq!(
            p.kind,
            PostingKind::Real as i32,
            "non-virtual posting {} should carry PostingKind::Real",
            p.account
        );
    }
}

#[test]
fn virtual_posting_balanced() {
    // Fixture has 3 postings: Assets:Checking $100, [Equity:Reservations]
    // $25, Equity:Opening -$125. Brackets mark the second as a virtual
    // *balanced* posting — DOES participate in balance accounting (so
    // all three sum to 0), but is flagged so reports can hide it via
    // `--real`.
    let j = compile("virtual_posting_balanced.ledger");
    let t = &j.transactions[0];
    assert_eq!(t.postings.len(), 3);

    // Brackets stripped from the account name.
    let virt = t
        .postings
        .iter()
        .find(|p| p.account == "Equity:Reservations")
        .expect("virtual posting account name should have brackets stripped");
    assert_eq!(virt.amount_in("$"), Some(dec!(25)));

    // All three postings (including the virtual balanced one) sum to 0.
    let total: rust_decimal::Decimal = t.postings.iter().filter_map(|p| p.amount_in("$")).sum();
    assert_eq!(
        total,
        dec!(0),
        "virtual balanced posting should participate in balance"
    );

    // Virtual-balanced posting carries the correct kind field.
    use doppio::elaboration::PostingKind;
    assert_eq!(
        virt.kind,
        PostingKind::VirtualBalanced as i32,
        "virtual balanced posting should carry PostingKind::VirtualBalanced"
    );
}

#[test]
fn fx_conversion_p_directive() {
    // The fixture declares `P 2024-01-01 EUR $1.10` and a posting in EUR.
    let j = compile("fx_conversion_p_directive.ledger");

    // Prerequisite: the price was parsed and stored.
    assert_eq!(j.prices.len(), 1);
    assert_eq!(j.prices[0].commodity, "EUR");
    assert_eq!(j.prices[0].price_commodity, "$");

    // Prerequisite: the EUR posting elaborated.
    let travel = j.transactions[0]
        .postings
        .iter()
        .find(|p| p.account == "Expenses:Travel")
        .expect("travel posting present");
    assert_eq!(travel.amount_in("EUR"), Some(dec!(100)));

    // `Journal::exchange_rate_at` resolves the EUR→$ conversion from the P directive.
    let rate = j
        .exchange_rate_at("EUR", "$", None)
        .expect("EUR→$ quote is present in the journal");
    assert_eq!(rate, dec!(1.10));

    // Applying the rate: 100 EUR * 1.10 = $110.
    let eur_balance = travel.amount_in("EUR").unwrap();
    assert_eq!(eur_balance * rate, dec!(110));
}

#[test]
fn bare_d_directive() {
    // Fixture declares `D $1,000.00` (default commodity + format) then a
    // posting using a bare amount `50` — which should pick up `$` as
    // its commodity via the default-commodity inference.
    let j = compile("bare_d_directive.ledger");
    let t = &j.transactions[0];

    // Bare `50` should infer `$` from the D directive's default commodity.
    assert_eq!(t.postings[0].account, "Expenses:Food");
    assert_eq!(t.postings[0].amount_in("$"), Some(dec!(50)));
    assert_eq!(t.postings[1].account, "Assets:Checking");
    assert_eq!(t.postings[1].amount_in("$"), Some(dec!(-50)));

    // The format string from the D directive should land on the
    // commodity declaration just like `commodity $ ; format ...` would.
    let dollar = j.commodities.get("$").expect("D directive declares $");
    assert_eq!(dollar.format.as_deref(), Some("$1,000.00"));
}

#[test]
fn bare_d_directive_postfix() {
    // Fixture declares `D 1,000.00 USD` (number-first / postfix form) then a
    // posting using a bare amount `50` — which should pick up `USD` as its
    // commodity via the default-commodity inference.
    let j = compile("bare_d_directive_postfix.ledger");
    let t = &j.transactions[0];

    // Bare `50` should infer `USD` from the D directive's default commodity.
    assert_eq!(t.postings[0].account, "Expenses:Food");
    assert_eq!(t.postings[0].amount_in("USD"), Some(dec!(50)));
    assert_eq!(t.postings[1].account, "Assets:Checking");
    assert_eq!(t.postings[1].amount_in("USD"), Some(dec!(-50)));

    // The format string from the D directive should be stored on the
    // commodity entry — same as a `commodity 1,000.00 USD` block would yield.
    let usd = j.commodities.get("USD").expect("D directive declares USD");
    assert_eq!(usd.format.as_deref(), Some("1,000.00 USD"));
}

#[test]
fn account_alias_subdir() {
    // The simplest case: `account Assets:Checking / alias Checking`,
    // then a posting using `Checking` should resolve to `Assets:Checking`.
    // This case turns out to work end-to-end through the parse → resolve
    // → elaborate pipeline (PR #153 surfaced it). The `🔧 Partial` flag
    // in SUPPORTED_FEATURES.md was left over from before the integration
    // test corpus existed.
    let j = compile("account_alias_subdir.ledger");
    let t = &j.transactions[0];
    let checking = t
        .postings
        .iter()
        .find(|p| p.account == "Assets:Checking")
        .expect("alias `Checking` should resolve to `Assets:Checking`");
    assert_eq!(checking.amount_in("$"), Some(dec!(1000)));
    assert!(
        !t.postings.iter().any(|p| p.account == "Checking"),
        "alias should resolve at resolution time; unaliased `Checking` \
         must not appear in the elaborated journal"
    );
}

#[test]
fn account_alias_multiple_per_block() {
    // An `account` block can carry multiple `alias` sub-directives; each
    // should be added to the alias map and resolve independently.
    let j = compile("account_alias_multiple_per_block.ledger");
    assert_eq!(j.transactions.len(), 2);

    // Both transactions' aliased postings should resolve to Assets:Checking.
    let t0 = &j.transactions[0];
    assert!(
        t0.postings
            .iter()
            .any(|p| p.account == "Assets:Checking" && p.amount_in("$") == Some(dec!(100))),
        "long alias `Checking` should resolve to Assets:Checking; got {:?}",
        t0.postings.iter().map(|p| &p.account).collect::<Vec<_>>()
    );
    let t1 = &j.transactions[1];
    assert!(
        t1.postings
            .iter()
            .any(|p| p.account == "Assets:Checking" && p.amount_in("$") == Some(dec!(50))),
        "short alias `C` should resolve to Assets:Checking; got {:?}",
        t1.postings.iter().map(|p| &p.account).collect::<Vec<_>>()
    );

    // Neither raw alias name should appear in the elaborated journal.
    for t in &j.transactions {
        for p in &t.postings {
            assert_ne!(p.account, "Checking", "alias `Checking` not resolved");
            assert_ne!(p.account, "C", "alias `C` not resolved");
        }
    }
}

#[test]
fn account_alias_across_blocks() {
    // Two account blocks each declare their own alias. Both aliases must
    // remain live after the second block is parsed, so a transaction that
    // uses both resolves correctly.
    let j = compile("account_alias_across_blocks.ledger");
    let t = &j.transactions[0];

    let checking = t
        .postings
        .iter()
        .find(|p| p.account == "Assets:Checking")
        .expect("Checking alias should resolve");
    assert_eq!(checking.amount_in("$"), Some(dec!(-100)));

    let savings = t
        .postings
        .iter()
        .find(|p| p.account == "Assets:Savings")
        .expect("Savings alias should resolve");
    assert_eq!(savings.amount_in("$"), Some(dec!(100)));
}

#[test]
fn account_alias_forward_only() {
    // The alias affects entries declared AFTER the account block, not
    // before. The pre-declaration posting keeps the literal `Checking`
    // name; the post-declaration one resolves to `Assets:Checking`.
    // (In ledger-cli the pre-declaration `Checking` is just a regular
    // account name — there's no error, just no resolution.)
    let j = compile("account_alias_forward_only.ledger");
    assert_eq!(j.transactions.len(), 2);

    // Pre-declaration: literal `Checking` is the elaborated account name.
    let pre = &j.transactions[0];
    assert!(
        pre.postings.iter().any(|p| p.account == "Checking"),
        "pre-declaration posting should keep literal `Checking` name; \
         got {:?}",
        pre.postings.iter().map(|p| &p.account).collect::<Vec<_>>()
    );
    assert!(
        !pre.postings.iter().any(|p| p.account == "Assets:Checking"),
        "alias must not retroactively apply"
    );

    // Post-declaration: `Checking` resolves to `Assets:Checking`.
    let post = &j.transactions[1];
    assert!(
        post.postings.iter().any(|p| p.account == "Assets:Checking"),
        "post-declaration posting should resolve via alias"
    );
}

#[test]
fn account_alias_inside_block_assert() {
    // The same block carries both an `alias` and an `assert`. The assert
    // is evaluated against postings that arrive via the alias — alias
    // resolution must happen before the assert is applied so the
    // assert sees the canonical account name (in this fixture, the
    // assert checks `commodity == "$"`, which the aliased posting
    // satisfies). Reaching elaboration without error means the assert
    // fired against the resolved posting.
    let j = compile("account_alias_inside_block_assert.ledger");
    let t = &j.transactions[0];
    assert!(
        t.postings.iter().any(|p| p.account == "Assets:Checking"),
        "alias should resolve and the assert should pass on the resolved posting"
    );
}
