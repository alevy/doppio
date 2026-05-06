//! Beancount frontend (experimental): parse `.beancount` source text into an
//! [`ast::Journal`].
//!
//! Three layers, mirroring [`crate::grammars::ledger`] and
//! [`crate::grammars::hledger`]:
//!
//! 1. **[`BeancountParser`]** -- the pest-derived parser generated from
//!    `beancount.pest`.
//! 2. **[`parse_beancount`]** -- a convenience wrapper that uses a no-op
//!    include opener; useful in unit tests.
//! 3. **[`BeancountFrontend`]** -- implements [`crate::frontend::Frontend`]
//!    so the CLI dispatches `.beancount` files to this module.
//!
//! ## Mapping decisions (lowering Beancount → [`ast`])
//!
//! Beancount's directive set is wider than ledger-cli's, so a few directives
//! collapse onto existing AST nodes rather than gaining new variants:
//!
//! | Beancount | AST |
//! |-----------|-----|
//! | `open Account [currencies] [booking]` | [`Directive::Account`] |
//! | `close Account` | [`Entry::Comment`] (no semantics, preserved verbatim) |
//! | `commodity SYMBOL` | [`Directive::Commodity`] |
//! | `*` / `!` / `txn` transactions | [`Entry::Transaction`] |
//! | `balance Account amount` | [`Entry::Assertion`] (strict) |
//! | `price COMM amount` | [`Entry::HistoricalPrice`] |
//! | `note` / `document` / `event` / `query` / `custom` | [`Entry::Comment`] |
//! | `pad target source` | [`Entry::Pad`] -- marker; #147 owns elaboration |
//! | `option` / `plugin` | [`Entry::Comment`] |
//! | `include "path"` | recursively parsed via the same opener as hledger |
//! | `#tag` / `^link` on a transaction | folded into `transaction.notes` as `tag:NAME` / `link:NAME` |
//!
//! ## Known limitations / stubs
//!
//! - The lot syntax `{cost, date, label}` is parsed best-effort: comma-split
//!   parts are classified as cost (first amount-looking part), date (ISO),
//!   or quoted label. Beancount's wildcard `{*}` and `{*, ...}` forms are
//!   accepted but the wildcard is dropped (no AST representation today).
//! - `pad` is preserved as an [`Entry::Pad`] marker but the elaborator does
//!   not yet act on it; the algorithm is the subject of #147.
//! - String escape sequences inside quoted strings are not interpreted.
//! - `pushtag`/`poptag` and `pushmeta`/`popmeta` are not yet parsed.

use crate::ast::*;
use pest::Parser as _;
use pest::iterators::{Pair, Pairs};
use pest::pratt_parser::PrattParser;
use pest_derive::Parser;
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::sync::LazyLock;

/// The raw pest parser generated from `beancount.pest` via `pest_derive`.
#[derive(Parser)]
#[grammar = "grammars/beancount/beancount.pest"]
pub(crate) struct BeancountParser;

// ---
// Internal parser state
// ---
struct Parser<F: Fn(&str) -> Result<String, Box<dyn std::error::Error>>> {
    opener: F,
    base_path: PathBuf,
}

impl<F: Fn(&str) -> Result<String, Box<dyn std::error::Error>>> Parser<F> {
    fn parse(&mut self, input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
        let pairs = BeancountParser::parse(Rule::journal, input)?;
        let mut entries = Vec::new();

        for pair in pairs.into_iter().next().unwrap().into_inner() {
            match pair.as_rule() {
                Rule::transaction => {
                    entries.push(Entry::Transaction(parse_transaction(pair)?));
                }
                Rule::open_directive => {
                    entries.push(Entry::Directive(parse_open_directive(pair)));
                }
                Rule::close_directive => {
                    // Preserved as a comment so the source line survives the
                    // round-trip; resolution discards comments.
                    entries.push(Entry::Comment(format!("close {}", pair.as_str().trim())));
                }
                Rule::commodity_directive => {
                    entries.push(Entry::Directive(parse_commodity_directive(pair)));
                }
                Rule::balance_directive => {
                    entries.push(Entry::Assertion(parse_balance_directive(pair)));
                }
                Rule::price_directive => {
                    entries.push(Entry::HistoricalPrice(parse_price_directive(pair)));
                }
                Rule::pad_directive => {
                    entries.push(Entry::Pad(parse_pad_directive(pair)));
                }
                Rule::note_directive
                | Rule::document_directive
                | Rule::event_directive
                | Rule::query_directive
                | Rule::custom_directive
                | Rule::option_directive
                | Rule::plugin_directive => {
                    entries.push(Entry::Comment(pair.as_str().trim().to_string()));
                }
                Rule::comment_line => {
                    entries.push(Entry::Comment(pair.as_str().to_string()));
                }
                Rule::include_directive => {
                    let raw = pair.into_inner().next().unwrap();
                    let path_str = string_inner(raw);
                    let include_path = self.base_path.join(&path_str);
                    let new_input = (self.opener)(include_path.as_os_str().to_str().unwrap())?;
                    let new_base_path = include_path
                        .parent()
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| self.base_path.clone());
                    let old_base_path = std::mem::replace(&mut self.base_path, new_base_path);
                    entries.append(&mut self.parse(&new_input)?.entries);
                    let _ = std::mem::replace(&mut self.base_path, old_base_path);
                }
                _ => {}
            }
        }

        Ok(Journal { entries })
    }
}

// ---
// Atom helpers
// ---
fn parse_date(pair: Pair<Rule>) -> Date {
    // date = ${ year ~ "-" ~ monthdate ~ "-" ~ monthdate }
    let mut inner = pair.into_inner();
    let year: i32 = inner.next().unwrap().as_str().parse().unwrap();
    let month: u32 = inner.next().unwrap().as_str().parse().unwrap();
    let day: u32 = inner.next().unwrap().as_str().parse().unwrap();
    Date {
        year: Some(year),
        month,
        date: day,
    }
}

fn parse_flag(s: &str) -> TransactionState {
    match s {
        "*" => TransactionState::Cleared,
        "!" => TransactionState::Pending,
        // `txn` keyword has no state-equivalent connotation in ledger semantics;
        // treat it as uncleared (the conservative interpretation).
        _ => TransactionState::Uncleared,
    }
}

/// Extract the inner text of a `string` pair (strip surrounding quotes).
fn string_inner(pair: Pair<Rule>) -> String {
    // string = ${ "\"" ~ string_inner ~ "\"" }
    let inner_pair = pair
        .into_inner()
        .next()
        .expect("string must contain string_inner");
    inner_pair.as_str().to_string()
}

// ---
// Transactions + postings
// ---
fn parse_transaction(pair: Pair<Rule>) -> Result<Transaction, Box<dyn std::error::Error>> {
    let mut inner = pair.into_inner();
    let header_pair = inner.next().expect("transaction must have a txn_header");

    let mut header_fields = header_pair.into_inner();
    let date = parse_date(header_fields.next().unwrap());

    let mut state = TransactionState::Uncleared;
    let mut payee_or_desc: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    for p in header_fields {
        match p.as_rule() {
            Rule::flag => state = parse_flag(p.as_str()),
            Rule::string => payee_or_desc.push(string_inner(p)),
            Rule::tag => notes.push(format!("tag:{}", &p.as_str()[1..])),
            Rule::link => notes.push(format!("link:{}", &p.as_str()[1..])),
            Rule::note => {
                notes.push(p.into_inner().as_str().trim().to_string());
            }
            _ => {}
        }
    }

    // Beancount transaction headers carry up to two strings: `"payee" "narration"`
    // or just `"narration"`. We map: one string -> description; two strings
    // -> code = payee, description = narration. The code field is the closest
    // ledger-cli analogue for the payee slot.
    let (code, description) = match payee_or_desc.len() {
        0 => (None, String::new()),
        1 => (None, payee_or_desc.remove(0)),
        _ => {
            let payee = payee_or_desc.remove(0);
            let narration = payee_or_desc.remove(0);
            (Some(payee), narration)
        }
    };

    let mut postings = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::posting => postings.push(parse_posting(p)?),
            Rule::metadata_line => {
                notes.push(metadata_line_to_note(p));
            }
            _ => {}
        }
    }

    Ok(Transaction {
        date,
        secondary_date: None,
        state,
        code,
        description,
        notes,
        postings,
    })
}

fn metadata_line_to_note(pair: Pair<Rule>) -> String {
    // metadata_line = ${ indent+ ~ metadata_key ~ ":" ~ ws* ~ metadata_value ~ (NEWLINE | EOI) }
    let mut inner = pair.into_inner();
    let key = inner.next().unwrap().as_str().to_string();
    let val = inner
        .next()
        .map(|v| v.as_str().trim().to_string())
        .unwrap_or_default();
    format!("{key}: {val}")
}

fn parse_posting(pair: Pair<Rule>) -> Result<Posting, Box<dyn std::error::Error>> {
    let mut state = TransactionState::Uncleared;
    let mut account = String::new();
    let mut amount: Option<AmountDetails> = None;
    let mut notes: Vec<String> = Vec::new();

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::flag => state = parse_flag(p.as_str()),
            Rule::posting_account => {
                let inner_pair = p
                    .into_inner()
                    .next()
                    .expect("posting_account must have one child");
                account = inner_pair.as_str().trim().to_string();
            }
            Rule::amount_logic => amount = Some(parse_amount_logic(p)?),
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::metadata_line => notes.push(metadata_line_to_note(p)),
            _ => {}
        }
    }

    Ok(Posting {
        account,
        amount,
        state,
        notes,
        kind: PostingKind::Real,
    })
}

fn parse_amount_logic(pair: Pair<Rule>) -> Result<AmountDetails, Box<dyn std::error::Error>> {
    // amount_logic = ${ (ws{2,} | "\t") ~ value_expr ~ (ws* ~ lot_annotation_or_price)* }
    let mut value: Option<ValueExpr> = None;
    let mut lot_annotation = LotAnnotation::default();
    let mut has_lot_annotation = false;
    let mut lot_pricing: Option<LotPricing> = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::value_expr => value = Some(parse_expr(p)),
            Rule::lot_annotation => {
                has_lot_annotation = true;
                merge_lot_annotation_into(p, &mut lot_annotation);
            }
            Rule::lot_price => {
                let s = p.as_str();
                let inner_expr = parse_expr(p.into_inner().next().unwrap());
                if s.starts_with("@@") {
                    lot_pricing = Some(LotPricing::Total(inner_expr));
                } else {
                    lot_pricing = Some(LotPricing::Unit(inner_expr));
                }
            }
            _ => {}
        }
    }

    Ok(AmountDetails::Amount {
        value: value.expect("amount_logic without a value_expr"),
        lot_annotation: has_lot_annotation.then_some(lot_annotation),
        lot_pricing,
        balance_assertion: None,
    })
}

/// Parse the inner text of a Beancount `{...}` lot annotation.
///
/// Best-effort: comma-split the inner text and classify each part.
/// - First amount-looking part (`<number> <COMMODITY>`) becomes the cost.
/// - First ISO date becomes the lot date.
/// - First quoted string becomes the lot label.
///
/// Beancount's wildcard `*` (alone or as the first part) is silently dropped:
/// "automatic cost" semantics are not yet modelled.
fn merge_lot_annotation_into(pair: Pair<Rule>, acc: &mut LotAnnotation) {
    // lot_annotation = ${ ("{{" ~ lot_inner_total ~ "}}") | ("{" ~ lot_inner ~ "}") }
    let inner = pair
        .into_inner()
        .next()
        .expect("lot_annotation must have an inner");
    let raw = inner.as_str();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() || part == "*" {
            continue;
        }
        // Quoted label?
        if let Some(stripped) = part.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            if acc.note.is_none() {
                acc.note = Some(stripped.to_string());
            }
            continue;
        }
        // ISO date?
        if let Ok(d) = chrono::NaiveDate::parse_from_str(part, "%Y-%m-%d") {
            if acc.date.is_none() {
                acc.date = Some(d);
            }
            continue;
        }
        // Otherwise: try to parse as a value expression (the cost).
        if acc.cost.is_none()
            && let Ok(mut pairs) = BeancountParser::parse(Rule::value_expr, part)
            && let Some(p) = pairs.next()
        {
            acc.cost = Some(parse_expr(p));
        }
    }
}

// ---
// Directives
// ---
fn parse_open_directive(pair: Pair<Rule>) -> Directive {
    // open_directive = ${ date ~ ws+ ~ "open" ~ ws+ ~ account
    //   ~ (ws+ ~ commodity_list)? ~ (ws+ ~ booking_method)?
    //   ~ (ws* ~ note)? ~ (NEWLINE | EOI) ~ metadata_line* }
    let mut inner = pair.into_inner();
    let _date = parse_date(inner.next().unwrap()); // date is captured at the marker level; not used in Directive::Account
    let name = inner.next().unwrap().as_str().to_string();

    let mut notes = Vec::new();
    let items = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::commodity_list => {
                // Currencies on `open` aren't representable as a structured
                // AccountItem yet; preserve them as a metadata note.
                let list: Vec<&str> = p.into_inner().map(|c| c.as_str()).collect();
                notes.push(format!("currencies: {}", list.join(",")));
            }
            Rule::booking_method => {
                let inner_pair = p.into_inner().next().unwrap();
                notes.push(format!("booking: {}", inner_pair.as_str()));
            }
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::metadata_line => notes.push(metadata_line_to_note(p)),
            _ => {}
        }
    }

    Directive::Account { name, notes, items }
}

fn parse_commodity_directive(pair: Pair<Rule>) -> Directive {
    // commodity_directive = ${ date ~ ws+ ~ "commodity" ~ ws+ ~ commodity ... }
    let mut inner = pair.into_inner();
    let _date = parse_date(inner.next().unwrap());
    let name = inner.next().unwrap().as_str().to_string();

    let mut notes = Vec::new();
    let items = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::metadata_line => notes.push(metadata_line_to_note(p)),
            _ => {}
        }
    }

    Directive::Commodity { name, notes, items }
}

fn parse_balance_directive(pair: Pair<Rule>) -> AssertionDirective {
    // balance_directive = ${ date ~ ws+ ~ "balance" ~ ws+ ~ account ~ ws+ ~ value_expr ... }
    let mut inner = pair.into_inner();
    let date = parse_date(inner.next().unwrap());
    let account = inner.next().unwrap().as_str().to_string();
    let value_expr_pair = inner
        .find(|p| p.as_rule() == Rule::value_expr)
        .expect("balance_directive must have a value_expr");
    let amount = parse_expr(value_expr_pair);

    AssertionDirective {
        date,
        account,
        amount,
        // Beancount balance assertions are strict by definition (the
        // tolerance comes from the precision of the asserted amount, not
        // from a weak-vs-strict toggle as in ledger-cli/hledger).
        strict: true,
    }
}

fn parse_price_directive(pair: Pair<Rule>) -> HistoricalPrice {
    // price_directive = ${ date ~ ws+ ~ "price" ~ ws+ ~ commodity ~ ws+ ~ value_expr ... }
    let mut inner = pair.into_inner();
    let date = parse_date(inner.next().unwrap());
    let commodity = inner.next().unwrap().as_str().to_string();
    let value_expr_pair = inner
        .find(|p| p.as_rule() == Rule::value_expr)
        .expect("price_directive must have a value_expr");
    let price = parse_expr(value_expr_pair);

    HistoricalPrice {
        date,
        time: None,
        commodity,
        price,
    }
}

fn parse_pad_directive(pair: Pair<Rule>) -> PadDirective {
    // pad_directive = ${ date ~ ws+ ~ "pad" ~ ws+ ~ account ~ ws+ ~ account ... }
    let mut inner = pair.into_inner();
    let date = parse_date(inner.next().unwrap());
    let target_account = inner.next().unwrap().as_str().to_string();
    let source_account = inner.next().unwrap().as_str().to_string();

    PadDirective {
        date,
        target_account,
        source_account,
    }
}

// ---
// Value expression parser (Pratt) -- shape mirrors hledger's.
// ---
static PRATT: LazyLock<PrattParser<Rule>> = LazyLock::new(|| {
    use Rule::*;
    use pest::pratt_parser::{Assoc::*, Op};
    PrattParser::new()
        .op(Op::infix(add, Left) | Op::infix(sub, Left))
        .op(Op::infix(mul, Left) | Op::infix(div, Left))
        .op(Op::prefix(prefix_op))
});

fn parse_expr(pair: Pair<Rule>) -> ValueExpr {
    // value_expr = ${ expr ~ (ws+ ~ commodity)? }
    let mut inner = pair.into_inner();
    let expr_pair = inner.next().expect("empty value_expr");
    let mut ast = run_pratt(expr_pair.into_inner());

    if let Some(comm_pair) = inner.next() {
        ast = ValueExpr::Typed {
            expr: Box::new(ast),
            commodity: comm_pair.as_str().to_string(),
        };
    }
    ast
}

fn run_pratt(pairs: Pairs<Rule>) -> ValueExpr {
    PRATT
        .map_primary(|pair| match pair.as_rule() {
            Rule::term => run_pratt(pair.into_inner()),
            Rule::primary => {
                let base = pair.into_inner().next().expect("primary must have a base");
                run_pratt(pest::iterators::Pairs::single(base))
            }
            Rule::amount => {
                let mut inner = pair.into_inner();
                let first = inner.next().unwrap();
                match first.as_rule() {
                    Rule::number => {
                        let val = clean_decimal(first.as_str());
                        let comm = inner.next().map(|c| c.as_str().to_string());
                        ValueExpr::Amount {
                            value: val,
                            commodity: comm,
                        }
                    }
                    _ => unreachable!("unexpected amount sub-rule: {:?}", first.as_rule()),
                }
            }
            Rule::commodity => ValueExpr::Commodity(pair.as_str().to_string()),
            Rule::expr => run_pratt(pair.into_inner()),
            _ => unreachable!("unexpected primary rule: {:?}", pair.as_rule()),
        })
        .map_prefix(|op, expr| ValueExpr::Unary {
            op: if op.as_str() == "-" { Op::Sub } else { Op::Add },
            expr: Box::new(expr),
        })
        .map_infix(|lhs, op, rhs| {
            let op = match op.as_rule() {
                Rule::add => Op::Add,
                Rule::sub => Op::Sub,
                Rule::mul => Op::Mul,
                Rule::div => Op::Div,
                _ => unreachable!(),
            };
            ValueExpr::Binary {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                op,
            }
        })
        .parse(pairs)
}

fn clean_decimal(s: &str) -> Decimal {
    s.replace(',', "").parse().unwrap_or(Decimal::ZERO)
}

// ---
// Convenience function (test-only)
// ---
/// Parse Beancount source with no `include` support.
///
/// Any `include` directive in the input is silently resolved to an empty
/// string (no entries pulled in). Useful for unit tests and standalone
/// parsing.
#[cfg(test)]
pub(crate) fn parse_beancount(input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
    Parser {
        opener: |_| Ok(String::new()),
        base_path: PathBuf::new(),
    }
    .parse(input)
}

// ---
// Frontend impl
// ---
/// The Beancount file-format frontend (experimental).
///
/// Recognises `.beancount` files and lowers them to a
/// [`crate::resolution::HIR`] via the Beancount PEG grammar and the
/// AST adapter in this module.
///
/// ## Extension dispatch
///
/// | Extension | Frontend |
/// |-----------|----------|
/// | `.beancount` | [`BeancountFrontend`] |
///
/// ## Experimental
///
/// The Beancount frontend is shipped behind the `M#9` milestone marker
/// because the `pad` directive evaluator (#147) is not yet implemented:
/// pads are preserved in the AST as markers but produce no balancing
/// transaction during elaboration. See the module-level docs for the
/// full mapping table and known gaps.
pub struct BeancountFrontend;

impl crate::frontend::Frontend for BeancountFrontend {
    fn extensions(&self) -> &'static [&'static str] {
        &["beancount"]
    }

    fn parse(
        &self,
        input: &str,
        base_path: &std::path::Path,
        opener: &crate::frontend::Opener,
    ) -> Result<crate::resolution::HIR, Box<dyn std::error::Error>> {
        let ast_journal = Parser {
            opener: |path: &str| opener(path),
            base_path: base_path.to_path_buf(),
        }
        .parse(input)?;
        Ok(ast_journal.try_into()?)
    }
}

// ---
// Tests
// ---
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    /// The fixture covers every directive type the grammar supports;
    /// it is the contract for #145.
    const SAMPLE: &str = include_str!("../../../tests/fixtures/sample.beancount");

    // -- grammar-level coverage (carried over from #145) ----------------------

    #[test]
    fn sample_fixture_parses() {
        BeancountParser::parse(Rule::journal, SAMPLE).unwrap_or_else(|e| {
            panic!("sample.beancount failed to parse:\n{e}");
        });
    }

    fn parse_one(rule: Rule, input: &str) {
        BeancountParser::parse(rule, input)
            .unwrap_or_else(|e| panic!("expected `{input}` to match {rule:?}, got: {e}"));
    }

    #[test]
    fn date_iso_only() {
        parse_one(Rule::date, "2024-01-15");
    }

    #[test]
    fn date_rejects_slash_separator() {
        assert!(BeancountParser::parse(Rule::date, "2024/01/15").is_err());
    }

    #[test]
    fn currency_uppercase_identifier() {
        parse_one(Rule::commodity, "USD");
        parse_one(Rule::commodity, "EUR");
        parse_one(Rule::commodity, "AAPL");
        parse_one(Rule::commodity, "BTC");
        parse_one(Rule::commodity, "VHT_2024");
    }

    #[test]
    fn currency_must_start_uppercase() {
        assert!(BeancountParser::parse(Rule::commodity, "usd").is_err());
    }

    #[test]
    fn account_colon_segments() {
        parse_one(Rule::account, "Assets:Bank:Checking");
        parse_one(Rule::account, "Equity:Opening-Balances");
        parse_one(Rule::account, "Income:US:Acme:Salary");
    }

    #[test]
    fn account_requires_at_least_two_segments() {
        assert!(BeancountParser::parse(Rule::account, "Assets").is_err());
    }

    #[test]
    fn string_double_quoted() {
        parse_one(Rule::string, "\"hello world\"");
        parse_one(Rule::string, "\"\"");
    }

    #[test]
    fn flag_recognises_star_bang_txn() {
        parse_one(Rule::flag, "*");
        parse_one(Rule::flag, "!");
        parse_one(Rule::flag, "txn");
    }

    #[test]
    fn tag_and_link_chars() {
        parse_one(Rule::tag, "#vacation");
        parse_one(Rule::tag, "#trip-2024");
        parse_one(Rule::link, "^statement-2024-01");
    }

    #[test]
    fn plugin_directive_with_and_without_arg() {
        parse_one(Rule::plugin_directive, "plugin \"beancount.plugins.foo\"\n");
        parse_one(
            Rule::plugin_directive,
            "plugin \"beancount.plugins.foo\" \"argument-string\"\n",
        );
    }

    // -- adapter / Frontend tests --------------------------------------------

    #[test]
    fn simple_complete_transaction() {
        let input = "\
2024-01-12 * \"Acme Studio\" \"Invoice 0123\"
  Assets:Bank:Checking          3400.00 USD
  Income:Salary                -3400.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction");
        };
        assert!(matches!(tx.state, TransactionState::Cleared));
        assert_eq!(tx.code.as_deref(), Some("Acme Studio"));
        assert_eq!(tx.description, "Invoice 0123");
        assert_eq!(tx.postings.len(), 2);
        assert_eq!(tx.postings[0].account, "Assets:Bank:Checking");
    }

    #[test]
    fn flagged_transaction_with_single_narration() {
        let input = "\
2024-01-15 ! \"Groceries\"
  Liabilities:CreditCard         -87.43 USD
  Expenses:Food                   87.43 USD
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        assert!(matches!(tx.state, TransactionState::Pending));
        assert!(tx.code.is_none(), "single-string header has no payee/code");
        assert_eq!(tx.description, "Groceries");
    }

    #[test]
    fn txn_keyword_is_uncleared() {
        let input = "\
2024-02-15 txn \"Apple lot purchase\"
  Assets:Brokerage              10 AAPL
  Assets:Bank:Checking       -1825.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        assert!(matches!(tx.state, TransactionState::Uncleared));
    }

    #[test]
    fn tags_and_links_become_notes() {
        let input = "\
2024-01-12 * \"Trip\" #vacation #beach ^trip-001
  Expenses:Travel    100.00 USD
  Assets:Bank:Checking
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        assert!(tx.notes.iter().any(|n| n == "tag:vacation"));
        assert!(tx.notes.iter().any(|n| n == "tag:beach"));
        assert!(tx.notes.iter().any(|n| n == "link:trip-001"));
    }

    #[test]
    fn open_directive_becomes_account_directive() {
        let input = "2024-01-01 open Assets:Bank:Checking USD\n";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Directive(Directive::Account { name, notes, .. }) = &journal.entries[0] else {
            panic!("expected Account directive, got {:?}", journal.entries[0]);
        };
        assert_eq!(name, "Assets:Bank:Checking");
        assert!(notes.iter().any(|n| n.starts_with("currencies:")));
    }

    #[test]
    fn open_with_booking_method() {
        let input = "2024-01-01 open Assets:Brokerage AAPL,USD \"FIFO\"\n";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Directive(Directive::Account { name, notes, .. }) = &journal.entries[0] else {
            panic!();
        };
        assert_eq!(name, "Assets:Brokerage");
        assert!(notes.iter().any(|n| n == "booking: FIFO"));
        assert!(notes.iter().any(|n| n.contains("AAPL,USD")));
    }

    #[test]
    fn close_directive_becomes_comment() {
        let input = "2024-12-31 close Liabilities:CreditCard\n";
        let journal = parse_beancount(input).expect("parse");
        assert!(matches!(journal.entries[0], Entry::Comment(_)));
    }

    #[test]
    fn commodity_directive_round_trip() {
        let input = "\
2024-01-01 commodity USD
  name: \"US Dollar\"
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Directive(Directive::Commodity { name, notes, .. }) = &journal.entries[0] else {
            panic!();
        };
        assert_eq!(name, "USD");
        assert!(notes.iter().any(|n| n.starts_with("name:")));
    }

    #[test]
    fn balance_directive_becomes_assertion() {
        let input = "2024-01-15 balance Assets:Bank:Checking 5000.00 USD\n";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Assertion(a) = &journal.entries[0] else {
            panic!();
        };
        assert_eq!(a.account, "Assets:Bank:Checking");
        assert_eq!(a.date.year, Some(2024));
        assert_eq!(a.date.month, 1);
        assert_eq!(a.date.date, 15);
        assert!(a.strict, "Beancount balance assertions are strict");
        match &a.amount {
            ValueExpr::Amount { value, commodity } => {
                assert_eq!(*value, dec!(5000.00));
                assert_eq!(commodity.as_deref(), Some("USD"));
            }
            other => panic!("unexpected amount shape: {other:?}"),
        }
    }

    #[test]
    fn price_directive_becomes_historical_price() {
        let input = "2024-02-15 price AAPL 182.50 USD\n";
        let journal = parse_beancount(input).expect("parse");
        let Entry::HistoricalPrice(hp) = &journal.entries[0] else {
            panic!();
        };
        assert_eq!(hp.commodity, "AAPL");
        assert_eq!(hp.date.year, Some(2024));
    }

    #[test]
    fn pad_directive_becomes_pad_marker() {
        let input = "2024-01-01 pad Assets:Bank:Checking Equity:Opening-Balances\n";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Pad(p) = &journal.entries[0] else {
            panic!("expected Pad marker, got {:?}", journal.entries[0]);
        };
        assert_eq!(p.target_account, "Assets:Bank:Checking");
        assert_eq!(p.source_account, "Equity:Opening-Balances");
    }

    #[test]
    fn note_document_event_query_custom_become_comments() {
        let input = "\
2024-01-20 note Assets:Bank:Checking \"Need to verify\"
2024-01-22 document Assets:Bank:Checking \"jan-statement.txt\"
2024-01-01 event \"location\" \"Berlin\"
2024-06-30 query \"q1\" \"SELECT date\"
2024-01-01 custom \"budget\" \"Expenses:Food\" 200.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        // All five preserve as comments (no semantic mapping yet).
        assert_eq!(
            journal.entries.len(),
            5,
            "five entries; got {}",
            journal.entries.len()
        );
        for e in &journal.entries {
            assert!(matches!(e, Entry::Comment(_)));
        }
    }

    #[test]
    fn lot_annotation_cost_date_label() {
        let input = "\
2024-02-15 * \"Apple lot purchase\"
  Assets:Brokerage   10 AAPL {182.50 USD, 2024-02-15, \"buy-2024-02\"}
  Assets:Bank:Checking       -1825.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let Some(AmountDetails::Amount { lot_annotation, .. }) = &tx.postings[0].amount else {
            panic!("expected Amount details");
        };
        let ann = lot_annotation.as_ref().expect("lot annotation present");
        assert!(ann.cost.is_some(), "cost should be parsed");
        assert_eq!(
            ann.date,
            chrono::NaiveDate::from_ymd_opt(2024, 2, 15),
            "date should be parsed"
        );
        assert_eq!(ann.note.as_deref(), Some("buy-2024-02"));
    }

    #[test]
    fn lot_price_at_per_unit() {
        let input = "\
2024-05-15 * \"Buy euros\"
  Assets:Cash:EUR    500.00 EUR @ 1.07 USD
  Assets:Bank:Checking
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let Some(AmountDetails::Amount { lot_pricing, .. }) = &tx.postings[0].amount else {
            panic!();
        };
        assert!(matches!(lot_pricing, Some(LotPricing::Unit(_))));
    }

    #[test]
    fn arithmetic_amount_in_posting() {
        let input = "\
2024-03-01 *
  Expenses:Food    (50.00 + 50.00) USD
  Assets:Bank:Checking
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let Some(AmountDetails::Amount { value, .. }) = &tx.postings[0].amount else {
            panic!();
        };
        assert!(
            matches!(value, ValueExpr::Typed { .. }),
            "expected Typed wrapping arithmetic, got {value:?}"
        );
    }

    #[test]
    fn sample_fixture_round_trips_through_resolution() {
        // The full parity fixture should walk through parse + resolution
        // without errors. `pad` survives as a marker; resolution drops it
        // (#147 owns the elaboration path).
        let journal = parse_beancount(SAMPLE).expect("parse sample.beancount");
        assert!(!journal.entries.is_empty());
        let _hir: crate::resolution::HIR = journal.try_into().expect("resolve sample.beancount");
    }

    #[test]
    fn frontend_extensions_lists_beancount() {
        use crate::frontend::Frontend;
        assert!(BeancountFrontend.extensions().contains(&"beancount"));
    }
}
