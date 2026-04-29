//! hledger frontend: parse `.hledger` / `.journal` source text into an
//! [`ast::Journal`].
//!
//! This module follows the same three-layer structure as [`crate::grammars::ledger`]:
//!
//! 1. **[`HledgerParser`]** — a `pest`-derived parser generated from
//!    `hledger.pest`.
//! 2. **[`parse_hledger`]** — convenience function that wraps the parser with a
//!    no-op include opener; useful in tests.
//! 3. **[`HledgerFrontend`]** — implements [`crate::frontend::Frontend`] so the
//!    CLI can select this parser by file extension (`.hledger`, `.journal`).
//!
//! ## Known limitations / stubs
//!
//! - **Automated posting arithmetic bodies** (`*N` multiplier expressions):
//!   the `auto_rule` grammar rule captures the shape of an automated posting
//!   rule but postings whose amounts use `*N` will produce a parse error.
//!   See TODO(#103).
//! - `comment` / `end comment` block comments are not yet supported.
//! - Full date inference from a `Y year` directive is not implemented.

use crate::ast::*;
use pest::Parser as _;
use pest::iterators::Pair;
use pest_derive::Parser;
use rust_decimal::Decimal;
use std::path::PathBuf;

/// The raw pest parser generated from `hledger.pest` via `pest_derive`.
#[derive(Parser)]
#[grammar = "grammars/hledger/hledger.pest"]
pub struct HledgerParser;

// ──────────────────────────────────────────────────────────────────────────────
// Internal parser state
// ──────────────────────────────────────────────────────────────────────────────

struct Parser<F: Fn(&str) -> Result<String, Box<dyn std::error::Error>>> {
    opener: F,
    base_path: PathBuf,
}

impl<F: Fn(&str) -> Result<String, Box<dyn std::error::Error>>> Parser<F> {
    fn parse(&mut self, input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
        let pairs = HledgerParser::parse(Rule::journal, input)?;
        let mut entries = Vec::new();

        for pair in pairs.into_iter().next().unwrap().into_inner() {
            match pair.as_rule() {
                Rule::transaction => {
                    entries.push(Entry::Transaction(parse_transaction(pair)));
                }
                Rule::comment_line => {
                    entries.push(Entry::Comment(pair.as_str().to_string()));
                }
                Rule::historical_price => {
                    entries.push(Entry::HistoricalPrice(parse_historical_price(pair)));
                }
                Rule::account_directive => {
                    entries.push(Entry::Directive(parse_account_directive(pair)));
                }
                Rule::commodity_directive => {
                    entries.push(Entry::Directive(parse_commodity_directive(pair)));
                }
                Rule::default_directive => {
                    entries.push(Entry::Directive(parse_default_directive(pair)?));
                }
                Rule::include_directive => {
                    let include_path = self.base_path.join(pair.into_inner().as_str());
                    let new_input = (self.opener)(include_path.as_os_str().to_str().unwrap())?;
                    let new_base_path = include_path
                        .parent()
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| self.base_path.clone());
                    let old_base_path = std::mem::replace(&mut self.base_path, new_base_path);
                    entries.append(&mut self.parse(&new_input)?.entries);
                    let _ = std::mem::replace(&mut self.base_path, old_base_path);
                }
                Rule::periodic_transaction => {
                    // Periodic transactions (`~ monthly …`) share the same
                    // shape as ledger-cli budget entries: captured but not
                    // elaborated. The postings are intentionally dropped.
                }
                Rule::auto_rule => {
                    // Automated posting rules (`= QUERY`) are not yet
                    // elaborated.  TODO(#103): implement automated postings.
                }
                _ => {}
            }
        }

        Ok(Journal { entries })
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// AST construction helpers
// ──────────────────────────────────────────────────────────────────────────────

fn parse_date(pair: Pair<Rule>) -> Date {
    // date = ${ year ~ date_sep ~ monthdate ~ date_sep ~ monthdate }
    let mut inner = pair.into_inner();
    let year: i32 = inner.next().unwrap().as_str().parse().unwrap();
    let month: u32 = inner.next().unwrap().as_str().parse().unwrap();
    let date: u32 = inner.next().unwrap().as_str().parse().unwrap();
    Date {
        year: Some(year),
        month,
        date,
    }
}

fn parse_state(s: &str) -> TransactionState {
    match s {
        "*" => TransactionState::Cleared,
        "!" => TransactionState::Pending,
        _ => TransactionState::Uncleared,
    }
}

fn parse_transaction(pair: Pair<Rule>) -> Transaction {
    let mut inner = pair.into_inner();
    let header_pair = inner.next().unwrap();
    let mut postings = Vec::new();
    let mut notes = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::transaction_note => {
                if let Some(note_pair) = p.into_inner().next() {
                    notes.push(note_pair.into_inner().as_str().trim().to_string());
                }
            }
            Rule::posting => {
                postings.push(parse_posting(p));
            }
            _ => {}
        }
    }

    // Parse header fields
    let mut header = header_pair.into_inner();
    let date = parse_date(header.next().unwrap());

    let mut state = TransactionState::Uncleared;
    let mut code = None;
    let mut description = String::new();

    for p in header {
        match p.as_rule() {
            Rule::status => state = parse_state(p.as_str()),
            Rule::code => {
                let s = p.as_str();
                // Strip the surrounding parentheses.
                code = Some(s[1..s.len() - 1].to_string());
            }
            Rule::description => {
                description = p.as_str().trim_end().to_string();
            }
            Rule::note => {
                notes.push(p.into_inner().as_str().trim().to_string());
            }
            _ => {}
        }
    }

    Transaction {
        date,
        secondary_date: None,
        state,
        code,
        description,
        notes,
        postings,
    }
}

fn parse_posting(pair: Pair<Rule>) -> Posting {
    let inner = pair.into_inner();
    let mut state = TransactionState::Uncleared;
    let mut account = String::new();
    let mut kind = PostingKind::Real;
    let mut amount = None;
    let mut notes = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::status => state = parse_state(p.as_str()),
            Rule::posting_account => {
                let inner_pair = p
                    .into_inner()
                    .next()
                    .expect("posting_account must have one child");
                match inner_pair.as_rule() {
                    Rule::virtual_unbalanced_account => {
                        kind = PostingKind::VirtualUnbalanced;
                        account = inner_pair
                            .into_inner()
                            .next()
                            .expect("virtual_unbalanced_account must have virtual_account_inner")
                            .as_str()
                            .trim()
                            .to_string();
                    }
                    Rule::virtual_balanced_account => {
                        kind = PostingKind::VirtualBalanced;
                        account = inner_pair
                            .into_inner()
                            .next()
                            .expect("virtual_balanced_account must have virtual_account_inner")
                            .as_str()
                            .trim()
                            .to_string();
                    }
                    Rule::account => {
                        account = inner_pair.as_str().trim().to_string();
                    }
                    _ => {}
                }
            }
            Rule::amount_logic => amount = Some(parse_amount_logic(p)),
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::posting_note => {
                if let Some(note_pair) = p.into_inner().next() {
                    notes.push(note_pair.into_inner().as_str().trim().to_string());
                }
            }
            _ => {}
        }
    }

    Posting {
        account,
        amount,
        state,
        notes,
        kind,
    }
}

fn parse_amount_logic(pair: Pair<Rule>) -> AmountDetails {
    let p = pair.into_inner().next().unwrap();
    match p.as_rule() {
        Rule::value_logic => {
            let inner = p.into_inner();
            let mut value = None;
            let mut lot_annotation: LotAnnotation = LotAnnotation::default();
            let mut has_lot_annotation = false;
            let mut lot_pricing = None;
            let mut balance_assertion = None;

            for p in inner {
                match p.as_rule() {
                    Rule::value_expr => {
                        value = Some(parse_expr(p));
                    }
                    Rule::lot_annotation_or_price => {
                        let child = p.into_inner().next().unwrap();
                        match child.as_rule() {
                            Rule::lot_price => {
                                let s = child.as_str();
                                let inner_val = parse_expr(child.into_inner().next().unwrap());
                                if s.starts_with("@@") {
                                    lot_pricing = Some(LotPricing::Total(inner_val));
                                } else {
                                    lot_pricing = Some(LotPricing::Unit(inner_val));
                                }
                            }
                            Rule::lot_annotation => {
                                has_lot_annotation = true;
                                parse_lot_annotation_into(child, &mut lot_annotation);
                            }
                            _ => unreachable!(),
                        }
                    }
                    Rule::assertion => {
                        // assertion = ${ assertion_op ~ ws* ~ value_expr }
                        // The first inner child is assertion_op; the second is
                        // value_expr.  Skip assertion_op by filtering on rule.
                        let inner_expr_pair = p
                            .into_inner()
                            .find(|p| p.as_rule() == Rule::value_expr)
                            .expect("assertion must contain a value_expr");
                        balance_assertion = Some(parse_expr(inner_expr_pair));
                    }
                    _ => unreachable!(),
                }
            }
            AmountDetails::Amount {
                value: value.unwrap(),
                lot_annotation: has_lot_annotation.then_some(lot_annotation),
                lot_pricing,
                balance_assertion,
            }
        }
        Rule::assertion => {
            // The assertion rule: assertion_op ~ ws* ~ value_expr.
            // Skip the assertion_op child, take the value_expr.
            let inner_expr_pair = p
                .into_inner()
                .find(|p| p.as_rule() == Rule::value_expr)
                .expect("assertion must have a value_expr");
            AmountDetails::BalanceAssignment(parse_expr(inner_expr_pair))
        }
        _ => unreachable!("unexpected rule in amount_logic: {:?}", p.as_rule()),
    }
}

/// Merge a single `lot_annotation` grammar node into an accumulating
/// [`LotAnnotation`] struct.  Duplicate annotations of the same kind take the
/// last value (matching ledger-cli behaviour).
fn parse_lot_annotation_into(pair: Pair<Rule>, acc: &mut LotAnnotation) {
    let child = pair.into_inner().next().unwrap();
    match child.as_rule() {
        Rule::lot_cost => {
            let expr_pair = child.into_inner().next().unwrap();
            acc.cost = Some(parse_expr(expr_pair));
        }
        Rule::lot_date => {
            let date_pair = child.into_inner().next().unwrap();
            let d = parse_date(date_pair);
            if let (Some(year), month, day) = (d.year, d.month, d.date) {
                acc.date = chrono::NaiveDate::from_ymd_opt(year, month, day);
            }
        }
        Rule::lot_note => {
            let note_str = child.into_inner().next().unwrap().as_str().to_string();
            acc.note = Some(note_str);
        }
        _ => unreachable!(),
    }
}

fn parse_historical_price(pair: Pair<Rule>) -> HistoricalPrice {
    let mut inner = pair.into_inner();
    let date = parse_date(inner.next().unwrap());
    let mut commodity = String::new();
    let mut price_pair = None;

    for p in inner {
        match p.as_rule() {
            Rule::commodity => commodity = p.as_str().to_string(),
            Rule::value_expr => price_pair = Some(p),
            _ => {}
        }
    }

    HistoricalPrice {
        date,
        time: None,
        commodity,
        price: parse_expr(price_pair.expect("historical_price must have a price")),
    }
}

fn parse_account_directive(pair: Pair<Rule>) -> Directive {
    let mut inner = pair.into_inner();
    let name = inner.next().unwrap().as_str().to_string();
    let mut notes = Vec::new();
    let mut items = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::account_item => {
                items.push(parse_account_item(p));
            }
            _ => {}
        }
    }

    Directive::Account { name, notes, items }
}

fn parse_account_item(pair: Pair<Rule>) -> AccountItem {
    let mut inner = pair.into_inner();
    let key = inner.next().unwrap().as_str().to_string();
    let mut val = None;

    for p in inner {
        if p.as_rule() == Rule::account_val {
            val = Some(p.as_str().trim().to_string());
        }
    }

    match key.as_str() {
        "alias" => AccountItem::Alias(val.unwrap_or_default()),
        "note" => AccountItem::Note(val.unwrap_or_default()),
        _ => AccountItem::Unknown(key, val),
    }
}

/// Parse a bare `D <amount>` directive into a [`Directive::Commodity`] AST node.
///
/// hledger supports the same compact form as ledger-cli: `D $1,000.00` declares
/// the default commodity and its display format simultaneously. It is lowered to
/// the same internal representation as:
///
/// ```hledger
/// commodity $1,000.00
///     default
/// ```
///
/// Both symbol-first (`D $1,000.00`) and number-first (`D 1,000.00 USD`) forms
/// are accepted. If the expression carries no commodity (e.g. `D 1000.00` — a
/// bare number), an error is returned because there is no symbol to register.
fn parse_default_directive(pair: Pair<Rule>) -> Result<Directive, Box<dyn std::error::Error>> {
    let value_expr_pair = pair
        .into_inner()
        .next()
        .expect("default_directive must contain a value_expr");

    let format_str = value_expr_pair.as_str().trim().to_string();
    let parsed = parse_expr(value_expr_pair);

    let commodity = match &parsed {
        ValueExpr::Amount {
            commodity: Some(c), ..
        } => c.clone(),
        _ => {
            return Err(format!(
                "bare `D` directive requires an amount with an explicit commodity symbol \
                 (e.g. `D $1,000.00`); got `{format_str}` which carries no commodity"
            )
            .into());
        }
    };

    Ok(Directive::Commodity {
        name: commodity,
        notes: vec![],
        items: vec![CommodityItem::Default, CommodityItem::Format(format_str)],
    })
}

fn parse_commodity_directive(pair: Pair<Rule>) -> Directive {
    let mut inner = pair.into_inner();
    // commodity_format is the first child — the raw format string.
    let raw = inner.next().unwrap().as_str().trim().to_string();

    // Derive the canonical commodity name from the format string by stripping
    // digits, commas, dots, and whitespace.
    let name = derive_commodity_name(&raw);

    let mut notes = Vec::new();
    let mut items = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::commodity_item => {
                items.push(parse_commodity_item(p));
            }
            _ => {}
        }
    }

    // Store the format string as a `Format` item so downstream code can use it.
    if !raw.is_empty() {
        items.insert(0, CommodityItem::Format(raw));
    }

    Directive::Commodity { name, notes, items }
}

/// Extract the commodity symbol from a commodity format string.
///
/// hledger commodity directives carry a format string like `$1,000.00` or
/// `1,000.00 EUR`.  This function strips numeric content and whitespace to
/// recover the bare symbol (`$`, `EUR`, etc.).
fn derive_commodity_name(format: &str) -> String {
    // Strip digits, commas, dots, and whitespace to isolate the symbol.
    let sym: String = format
        .chars()
        .filter(|c| !c.is_ascii_digit() && *c != ',' && *c != '.' && !c.is_whitespace())
        .collect();
    if sym.is_empty() {
        format.trim().to_string()
    } else {
        sym.trim().to_string()
    }
}

fn parse_commodity_item(pair: Pair<Rule>) -> CommodityItem {
    let mut inner = pair.into_inner();
    let key = inner.next().unwrap().as_str().to_string();
    let mut val = None;

    for p in inner {
        if p.as_rule() == Rule::commodity_val {
            val = Some(p.as_str().trim().to_string());
        }
    }

    match key.as_str() {
        "alias" => CommodityItem::Alias(val.unwrap_or_default()),
        "format" => CommodityItem::Format(val.unwrap_or_default()),
        "nomarket" => CommodityItem::NoMarket,
        "default" => CommodityItem::Default,
        "note" => CommodityItem::Note(val.unwrap_or_default()),
        _ => CommodityItem::Unknown(key, val),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Value expression parser (Pratt)
// ──────────────────────────────────────────────────────────────────────────────

use pest::iterators::Pairs;
use pest::pratt_parser::PrattParser;
use std::sync::LazyLock;

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
                    Rule::commodity => {
                        let comm = first.as_str().to_string();
                        let val_str = inner.next().unwrap().as_str();
                        ValueExpr::Amount {
                            value: clean_decimal(val_str),
                            commodity: Some(comm),
                        }
                    }
                    Rule::number => {
                        let val = clean_decimal(first.as_str());
                        let comm = inner.next().map(|c| c.as_str().to_string());
                        ValueExpr::Amount {
                            value: val,
                            commodity: comm,
                        }
                    }
                    _ => unreachable!(),
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
    let cleaned = s.replace(',', "");
    cleaned.parse().unwrap_or(Decimal::ZERO)
}

// ──────────────────────────────────────────────────────────────────────────────
// Convenience function
// ──────────────────────────────────────────────────────────────────────────────

/// Parse hledger source with no `include` support.
///
/// Any `include` directives in the input are silently resolved to empty (no
/// entries included). Useful for unit tests and standalone parsing.
pub fn parse_hledger(input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
    Parser {
        opener: |_| Ok(String::new()),
        base_path: PathBuf::new(),
    }
    .parse(input)
}

// ──────────────────────────────────────────────────────────────────────────────
// Frontend impl
// ──────────────────────────────────────────────────────────────────────────────

/// The hledger file-format frontend.
///
/// Recognises `.hledger` and `.journal` files and converts them to
/// [`ast::Journal`] via the hledger PEG grammar.
///
/// ## Extension dispatch
///
/// | Extension | Frontend |
/// |-----------|----------|
/// | `.hledger` | [`HledgerFrontend`] |
/// | `.journal` | [`HledgerFrontend`] |
/// | `.ledger`  | [`crate::grammars::ledger::LedgerFrontend`] |
///
/// ## Known limitations
///
/// See the [module-level documentation](self) for the list of hledger features
/// that are stubbed out or not yet supported.
pub struct HledgerFrontend;

impl crate::frontend::Frontend for HledgerFrontend {
    fn extensions(&self) -> &'static [&'static str] {
        &["hledger", "journal"]
    }

    fn parse(
        &self,
        input: &str,
        base_path: &std::path::Path,
        opener: &crate::frontend::Opener,
    ) -> Result<crate::ast::Journal, Box<dyn std::error::Error>> {
        Parser {
            opener: |path: &str| opener(path),
            base_path: base_path.to_path_buf(),
        }
        .parse(input)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Unit tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rust_decimal::dec;

    use super::*;

    // ── simple transaction ────────────────────────────────────────────────────

    #[test]
    fn simple_cleared_transaction() {
        let input = "\
2024-01-15 * Opening Balances
    assets:bank:checking          $1000.00
    equity:opening-balances
";
        let journal = parse_hledger(input).expect("parse");
        assert_eq!(journal.entries.len(), 1);
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction");
        };
        assert_eq!(tx.date.year, Some(2024));
        assert_eq!(tx.date.month, 1);
        assert_eq!(tx.date.date, 15);
        assert!(matches!(tx.state, TransactionState::Cleared));
        assert_eq!(tx.description, "Opening Balances");
        assert_eq!(tx.postings.len(), 2);
        assert_eq!(tx.postings[0].account, "assets:bank:checking");
        assert!(tx.postings[1].amount.is_none(), "null posting");
    }

    // ── pending status, code, inline comment, posting note ───────────────────

    #[test]
    fn pending_with_code_and_comments() {
        let input = "\
2024-01-16 ! (INV-42) ACME Corp  ; project:website
    expenses:consulting           $500.00 ; vendor: ACME
    assets:bank:checking
";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        assert!(matches!(tx.state, TransactionState::Pending));
        assert_eq!(tx.code.as_deref(), Some("INV-42"));
        assert_eq!(tx.description, "ACME Corp");
        // Inline transaction note is captured.
        assert!(!tx.notes.is_empty(), "should have note from inline comment");
        // Posting note.
        assert_eq!(tx.postings[0].notes.len(), 1);
        assert!(tx.postings[0].notes[0].contains("vendor"));
    }

    // ── balance assertion (=) ─────────────────────────────────────────────────

    #[test]
    fn balance_assertion_single_commodity() {
        let input = "\
2024-01-31 * end-of-month check
    assets:bank:checking            $0.00 = $500.00
    expenses:adjustments
";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let p = &tx.postings[0];
        assert!(
            matches!(
                &p.amount,
                Some(AmountDetails::Amount {
                    balance_assertion: Some(_),
                    ..
                })
            ),
            "should have balance assertion"
        );
    }

    // ── strict balance assertion (==) ─────────────────────────────────────────

    #[test]
    fn balance_assertion_strict() {
        let input = "\
2024-02-01 * checked
    assets:bank:checking            $0.00 == $500.00
    expenses:adjustments
";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let p = &tx.postings[0];
        assert!(
            matches!(
                &p.amount,
                Some(AmountDetails::Amount {
                    balance_assertion: Some(_),
                    ..
                })
            ),
            "should have strict balance assertion"
        );
    }

    // ── balance assignment (= target, no LHS amount) ──────────────────────────

    #[test]
    fn balance_assignment() {
        let input = "\
2024-02-05 * Reset to known balance
    assets:bank:checking          = $750.00
    income:adjustments
";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let p = &tx.postings[0];
        assert!(
            matches!(&p.amount, Some(AmountDetails::BalanceAssignment(_))),
            "should be BalanceAssignment, got {:?}",
            p.amount
        );
    }

    // ── lot pricing @ ─────────────────────────────────────────────────────────

    #[test]
    fn lot_pricing_unit() {
        let input = "\
2024-02-10 * Buy euros
    assets:eur                  100.00 EUR @ $1.10
    assets:bank:checking         $-110.00
";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let p = &tx.postings[0];
        assert!(
            matches!(
                &p.amount,
                Some(AmountDetails::Amount {
                    lot_pricing: Some(LotPricing::Unit(_)),
                    ..
                })
            ),
            "should have unit lot pricing"
        );
    }

    // ── lot pricing @@ ────────────────────────────────────────────────────────

    #[test]
    fn lot_pricing_total() {
        let input = "\
2024-02-11 * Buy stock
    assets:brokerage             10 AAPL @@ $1825.00
    assets:bank:checking        $-1825.00
";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let p = &tx.postings[0];
        assert!(
            matches!(
                &p.amount,
                Some(AmountDetails::Amount {
                    lot_pricing: Some(LotPricing::Total(_)),
                    ..
                })
            ),
            "should have total lot pricing"
        );
    }

    // ── historical price ──────────────────────────────────────────────────────

    #[test]
    fn historical_price() {
        let input = "P 2024-01-02 EUR $1.10\n";
        let journal = parse_hledger(input).expect("parse");
        assert_eq!(journal.entries.len(), 1);
        let Entry::HistoricalPrice(hp) = &journal.entries[0] else {
            panic!("expected HistoricalPrice");
        };
        assert_eq!(hp.date.year, Some(2024));
        assert_eq!(hp.commodity, "EUR");
        assert!(matches!(
            hp.price,
            ValueExpr::Amount {
                commodity: Some(ref c),
                ..
            } if c == "$"
        ));
    }

    // ── account directive ─────────────────────────────────────────────────────

    #[test]
    fn account_directive_with_note() {
        let input = "account assets:bank:checking    ; type:A\n";
        let journal = parse_hledger(input).expect("parse");
        assert_eq!(journal.entries.len(), 1);
        let Entry::Directive(Directive::Account { name, notes, .. }) = &journal.entries[0] else {
            panic!("expected Account directive");
        };
        assert_eq!(name, "assets:bank:checking");
        assert!(!notes.is_empty(), "should capture the note");
    }

    // ── commodity directive ───────────────────────────────────────────────────

    #[test]
    fn commodity_directive_prefix_symbol() {
        let input = "commodity $1,000.00\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Directive(Directive::Commodity { name, items, .. }) = &journal.entries[0] else {
            panic!("expected Commodity directive");
        };
        assert_eq!(name, "$");
        // Format item should be present.
        assert!(
            items.iter().any(|i| matches!(i, CommodityItem::Format(_))),
            "should have Format item"
        );
    }

    #[test]
    fn commodity_directive_suffix_symbol() {
        let input = "commodity 1,000.00 EUR\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Directive(Directive::Commodity { name, .. }) = &journal.entries[0] else {
            panic!("expected Commodity directive");
        };
        assert_eq!(name, "EUR");
    }

    // ── date separators ───────────────────────────────────────────────────────

    #[test]
    fn date_slash_separator() {
        let input = "2024/03/15 Test\n    expenses:food  $10\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse with / separator");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        assert_eq!(tx.date.year, Some(2024));
        assert_eq!(tx.date.month, 3);
        assert_eq!(tx.date.date, 15);
    }

    #[test]
    fn date_dot_separator() {
        let input = "2024.06.01 Test\n    expenses:food  $10\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse with . separator");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        assert_eq!(tx.date.month, 6);
        assert_eq!(tx.date.date, 1);
    }

    // ── hash comment ─────────────────────────────────────────────────────────

    #[test]
    fn hash_comment_line() {
        let input = "# This is a hash comment\n";
        let journal = parse_hledger(input).expect("parse");
        assert_eq!(journal.entries.len(), 1);
        assert!(matches!(journal.entries[0], Entry::Comment(_)));
    }

    // ── negative amount ───────────────────────────────────────────────────────

    #[test]
    fn negative_prefix_amount() {
        let input = "2024-01-01 Test\n    assets:bank  $-110.00\n    income:salary\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let p = &tx.postings[0];
        // $-110.00 → Unary(Sub, Amount($, 110.00))
        assert!(p.amount.is_some());
    }

    // ── amount with comma thousands separator ─────────────────────────────────

    #[test]
    fn amount_comma_thousands() {
        let input = "2024-01-01 Test\n    expenses:food  $1,234.56\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let AmountDetails::Amount { value, .. } = tx.postings[0].amount.as_ref().unwrap() else {
            panic!();
        };
        assert!(
            matches!(value, ValueExpr::Amount { value, .. } if *value == dec!(1234.56)),
            "expected 1234.56, got {value:?}"
        );
    }

    // ── arithmetic in posting amount ──────────────────────────────────────────

    #[test]
    fn arithmetic_amount() {
        let input = "2024-01-01 Test\n    expenses:food  (100 + 50) USD\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let AmountDetails::Amount { value, .. } = tx.postings[0].amount.as_ref().unwrap() else {
            panic!();
        };
        assert!(
            matches!(value, ValueExpr::Typed { .. }),
            "expected Typed wrapping an arithmetic expr, got {value:?}"
        );
    }

    // ── D default-commodity directive ─────────────────────────────────────────

    #[test]
    fn d_directive_prefix_symbol() {
        let input = "D $1,000.00\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Directive(Directive::Commodity { name, items, .. }) = &journal.entries[0] else {
            panic!("expected Commodity directive");
        };
        assert_eq!(name, "$");
        assert!(
            items.iter().any(|i| matches!(i, CommodityItem::Default)),
            "should have Default item"
        );
        assert!(
            items.iter().any(|i| matches!(i, CommodityItem::Format(_))),
            "should have Format item"
        );
    }

    #[test]
    fn d_directive_suffix_symbol() {
        let input = "D 1,000.00 USD\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Directive(Directive::Commodity { name, .. }) = &journal.entries[0] else {
            panic!("expected Commodity directive");
        };
        assert_eq!(name, "USD");
    }

    #[test]
    fn d_directive_rejects_bare_number() {
        // `D 1000.00` carries no commodity — the parser must reject it with a
        // message that mentions "commodity" so the user knows what is missing.
        let err = parse_hledger("D 1000.00\n").unwrap_err();
        assert!(
            err.to_string().contains("commodity"),
            "error should mention 'commodity', got: {err}"
        );
    }

    // ── periodic transaction is silently ignored ──────────────────────────────

    #[test]
    fn periodic_transaction_ignored() {
        let input = "\
~ monthly  Rent
    expenses:rent                $2000
    assets:bank:checking

2024-01-01 Real
    expenses:food  $10
    assets:cash
";
        let journal = parse_hledger(input).expect("parse");
        // The periodic transaction produces no entry; only the real transaction does.
        let txns: Vec<_> = journal
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Transaction(_)))
            .collect();
        assert_eq!(txns.len(), 1);
    }

    // ── virtual postings ──────────────────────────────────────────────────────

    #[test]
    fn virtual_unbalanced_posting_parses_to_correct_kind() {
        // `(Equity:Reservations)` — parentheses denote a virtual-unbalanced
        // posting. The account name must have the parens stripped and the kind
        // must be `PostingKind::VirtualUnbalanced`.
        let input = "\
2024-01-15 Setup
    Assets:Checking         $100
    Equity:Opening         $-100
    (Equity:Reservations)   $-25
";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction");
        };
        let virt = tx
            .postings
            .iter()
            .find(|p| p.account == "Equity:Reservations")
            .expect("virtual posting should be present with parens stripped");
        assert_eq!(
            virt.kind,
            PostingKind::VirtualUnbalanced,
            "posting kind should be VirtualUnbalanced"
        );
    }

    #[test]
    fn virtual_balanced_posting_parses_to_correct_kind() {
        // `[Equity:Reservations]` — square brackets denote a virtual-balanced posting.
        let input = "\
2024-01-15 Setup
    Assets:Checking          $100
    [Equity:Reservations]    $25
    Equity:Opening          $-125
";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction");
        };
        let virt = tx
            .postings
            .iter()
            .find(|p| p.account == "Equity:Reservations")
            .expect("virtual posting should be present with brackets stripped");
        assert_eq!(
            virt.kind,
            PostingKind::VirtualBalanced,
            "posting kind should be VirtualBalanced"
        );
    }

    // ── full sample-journal smoke test ────────────────────────────────────────

    #[test]
    fn sample_journal_parses_without_error() {
        // The sample.hledger from doppio-research, minus the include directive
        // (which would attempt file I/O).
        let input = "\
; A small but representative hledger journal

account assets:bank:checking    ; type:A
account expenses:groceries
account income:salary

commodity $1,000.00
commodity 1,000.00 EUR

P 2024-01-02 EUR $1.10
P 2024-02-15 AAPL $182.50

2024-01-15 * Opening Balances
    assets:bank:checking          $1000.00
    equity:opening-balances

2024-01-16 ! (INV-42) ACME Corp  ; project:website
    expenses:consulting           $500.00 ; vendor: ACME
    assets:bank:checking

2024-01-31 * end-of-month check
    assets:bank:checking            $0.00 = $500.00
    expenses:adjustments

2024-02-01 * checked
    assets:bank:checking            $0.00 == $500.00
    expenses:adjustments

2024-02-05 * Reset to known balance
    assets:bank:checking          = $750.00
    income:adjustments

2024-02-10 * Buy euros
    assets:eur                  100.00 EUR @ $1.10
    assets:bank:checking         $-110.00

2024-02-11 * Buy stock
    assets:brokerage             10 AAPL @@ $1825.00
    assets:bank:checking        $-1825.00
";
        parse_hledger(input).expect("sample journal should parse without error");
    }
}
