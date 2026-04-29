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
//! and panic with the tracking issue number, so the ignored count stays
//! a visible signal of remaining parity work. As each feature lands, the
//! corresponding ignore is removed and the panic is replaced with real
//! assertions.
//!
//! See `docs/SUPPORTED_FEATURES.md` for the human-readable matrix and
//! the Phase D milestone for the in-flight gap-fills.

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
// issue. When the feature lands, remove the `#[ignore]` and replace the
// panic with real assertions that consume the corresponding `lift the
// fixture out of #[ignore]` task in the issue's acceptance criteria.
// ──────────────────────────────────────────────────────────────────────────

#[test]
#[ignore = "tracks #139 — lot persistence {cost} not yet supported"]
fn lot_persistence_cost() {
    // When implemented: fixture should compile, and the brokerage posting's
    // lot annotation should carry cost = $150. Schema change required —
    // see #139 for the proto::Posting `lot` field.
    panic!("feature unimplemented; tracks #139");
}

#[test]
#[ignore = "tracks #139 — lot persistence [date] not yet supported"]
fn lot_persistence_date() {
    panic!("feature unimplemented; tracks #139");
}

#[test]
#[ignore = "tracks #139 — lot persistence ((note)) not yet supported"]
fn lot_persistence_note() {
    panic!("feature unimplemented; tracks #139");
}

#[test]
#[ignore = "tracks #140 — virtual postings (Account) not yet supported"]
fn virtual_posting_unbalanced() {
    // When implemented: posting kind = VirtualUnbalanced; the posting must
    // be excluded from balance accounting (so the real postings must
    // balance among themselves) and emitted in the elaborated journal with
    // the kind preserved.
    panic!("feature unimplemented; tracks #140");
}

#[test]
#[ignore = "tracks #140 — virtual postings [Account] not yet supported"]
fn virtual_posting_balanced() {
    // When implemented: posting kind = VirtualBalanced; participates in
    // the balance check but is flagged so `--real` filters can hide it.
    panic!("feature unimplemented; tracks #140");
}

#[test]
#[ignore = "tracks #141 — FX conversion via P directive not yet wired into reports"]
fn fx_conversion_p_directive() {
    // When implemented: `dop balance --exchange USD` (or the library
    // helper) should consult the P chain to convert the EUR balance to
    // its USD-equivalent at the as-of date. Today the P directive is
    // stored but never consulted by reports.
    panic!("feature unimplemented; tracks #141");
}

#[test]
#[ignore = "tracks #142 — bare D directive not yet supported"]
fn bare_d_directive() {
    // When implemented: `D $1000.00` should lower to the same internal
    // representation as `commodity $ ; default ; format $1,000.00`. The
    // subsequent bare amount `50` should pick up `$` as its commodity.
    panic!("feature unimplemented; tracks #142");
}

#[test]
#[ignore = "tracks #143 — account alias sub-directive resolution incomplete"]
fn account_alias_subdir() {
    // When implemented: the posting's `Checking` should resolve to
    // `Assets:Checking` in the elaborated journal. Today this resolution
    // path may differ from the top-level `alias` directive, per the
    // 🔧 Partial flag in docs/SUPPORTED_FEATURES.md.
    panic!("feature unimplemented; tracks #143");
}
