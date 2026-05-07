//! hledger frontend: parse `.hledger` / `.journal` source text into an
//! [`ast::Journal`].
//!
//! This module follows the same three-layer structure as [`crate::grammars::ledger`]:
//!
//! 1. **[`HledgerParser`]** -- a `pest`-derived parser generated from
//!    `hledger.pest`.
//! 2. **[`parse_hledger`]** -- convenience function that wraps the parser with a
//!    no-op include opener; useful in tests.
//! 3. **[`HledgerFrontend`]** -- implements [`crate::frontend::Frontend`] so the
//!    CLI can select this parser by file extension (`.hledger`, `.journal`).
//!
//! ## Known limitations / stubs
//!
//! - **Automated posting arithmetic bodies** (`*N` multiplier expressions):
//!   the `auto_rule` grammar rule captures the shape of an automated posting
//!   rule but postings whose amounts use `*N` will produce a parse error.
//!   See TODO(#103).
//! - Full date inference from a `Y year` directive is not implemented.
//!
//! ## Block comments
//!
//! `comment` ... `end comment` block comments are accepted; the
//! contents are preserved verbatim as `Entry::Comment` and resolution
//! discards them. An unclosed `comment` block (no matching
//! `end comment`) is implicitly terminated at EOF, matching hledger's
//! own behaviour.
//!
//! ## Lot-cost forms
//!
//! Both per-unit `{cost}` and total `{{total}}` forms are supported.
//! The adapter records the brace count on
//! [`ast::LotAnnotation::cost_is_total`]; the elaborator divides by
//! the posting's unit count when applying a total-cost lot, so the
//! canonical per-unit basis flows through the rest of the pipeline.
//!
//! ## Balance assignment forms
//!
//! - `Account = X`        — single-commodity balance assignment
//! - `Account == X`       — same as `=`; both are accepted
//! - `Account =* X` /
//!   `Account ==* X`      — strict-zero across every commodity in
//!   the account's running inventory. Typically `==* 0` in
//!   fiscal-year retained-earnings transactions. The elaborator
//!   synthesizes a multi-commodity posting that brings each
//!   currently-held commodity to `X`.

use crate::ast::*;
use pest::Parser as _;
use pest::iterators::Pair;
use pest_derive::Parser;
use rust_decimal::Decimal;
use std::path::PathBuf;

/// The raw pest parser generated from `hledger.pest` via `pest_derive`.
#[derive(Parser)]
#[grammar = "grammars/hledger/hledger.pest"]
pub(crate) struct HledgerParser;

// ---
// Internal parser state
// ---
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
                    entries.push(Entry::Transaction(parse_transaction(pair)?));
                }
                Rule::comment_line => {
                    entries.push(Entry::Comment(pair.as_str().to_string()));
                }
                Rule::block_comment => {
                    // Preserve the full source text (including the
                    // `comment` / `end comment` markers) so the round-trip
                    // representation is faithful; resolution discards.
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
                    // Periodic transactions (`~ monthly ...`) share the same
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

// ---
// AST construction helpers
// ---
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

fn parse_transaction(pair: Pair<Rule>) -> Result<Transaction, Box<dyn std::error::Error>> {
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
                postings.push(parse_posting(p)?);
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

fn parse_posting(pair: Pair<Rule>) -> Result<Posting, Box<dyn std::error::Error>> {
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
            Rule::amount_logic => amount = Some(parse_amount_logic(p)?),
            Rule::note => notes.push(p.into_inner().as_str().trim().to_string()),
            Rule::posting_note => {
                if let Some(note_pair) = p.into_inner().next() {
                    notes.push(note_pair.into_inner().as_str().trim().to_string());
                }
            }
            _ => {}
        }
    }

    Ok(Posting {
        account,
        amount,
        state,
        notes,
        kind,
    })
}

fn parse_amount_logic(pair: Pair<Rule>) -> Result<AmountDetails, Box<dyn std::error::Error>> {
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
            Ok(AmountDetails::Amount {
                value: value.unwrap(),
                lot_annotation: has_lot_annotation.then_some(lot_annotation),
                lot_pricing,
                balance_assertion,
            })
        }
        Rule::assertion => {
            // The assertion rule: assertion_op ~ ws* ~ value_expr.
            // The op text is `=` / `==` / `=*` / `==*`. The trailing `*`
            // qualifier ("all commodities") routes to a different
            // AmountDetails variant so the elaborator can synthesize a
            // multi-commodity posting that zeroes every commodity in the
            // account's inventory.
            let mut inner = p.into_inner();
            let op_pair = inner
                .next()
                .expect("assertion must contain an assertion_op");
            assert_eq!(op_pair.as_rule(), Rule::assertion_op);
            let all_commodities = op_pair.as_str().ends_with('*');
            let inner_expr_pair = inner
                .find(|p| p.as_rule() == Rule::value_expr)
                .expect("assertion must have a value_expr");
            let target = parse_expr(inner_expr_pair);
            Ok(if all_commodities {
                AmountDetails::BalanceAssignmentAllCommodities(target)
            } else {
                AmountDetails::BalanceAssignment(target)
            })
        }
        _ => unreachable!("unexpected rule in amount_logic: {:?}", p.as_rule()),
    }
}

/// Merge a single `lot_annotation` grammar node into an accumulating
/// [`LotAnnotation`] struct.  Duplicate annotations of the same kind take the
/// last value (matching ledger-cli behaviour).
///
/// The `{{total}}` double-brace form is recognised and recorded by setting
/// [`LotAnnotation::cost_is_total`]; the elaborator divides the captured
/// total by the posting's unit count to derive per-unit basis.
fn parse_lot_annotation_into(pair: Pair<Rule>, acc: &mut LotAnnotation) {
    let child = pair.into_inner().next().unwrap();
    match child.as_rule() {
        Rule::lot_cost => {
            // The `{{total}}` form: the grammar matches it before
            // `{cost}` so the raw token text starts with `{{`. The
            // elaborator divides by the posting's unit count when
            // applying the cost.
            if child.as_str().starts_with("{{") {
                acc.cost_is_total = true;
            }
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
/// are accepted. If the expression carries no commodity (e.g. `D 1000.00` -- a
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
    // commodity_format is the first child -- the raw format string.
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

// ---
// Value expression parser (Pratt)
// ---
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

// ---
// Convenience function
// ---
/// Parse hledger source with no `include` support.
///
/// Any `include` directives in the input are silently resolved to empty (no
/// entries included). Useful for unit tests and standalone parsing.
#[cfg(test)]
pub(crate) fn parse_hledger(input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
    Parser {
        opener: |_| Ok(String::new()),
        base_path: PathBuf::new(),
    }
    .parse(input)
}

// ---
// Frontend impl
// ---
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
    ) -> Result<crate::resolution::HIR, Box<dyn std::error::Error>> {
        let ast_journal = Parser {
            opener: |path: &str| opener(path),
            base_path: base_path.to_path_buf(),
        }
        .parse(input)?;
        let mut hir: crate::resolution::HIR = ast_journal.try_into()?;
        // hledger uses `@price` for transaction balance when present
        // and does not require an explicit gain/loss posting. After
        // the @price-driven balance check, the elaborator synthesizes
        // a posting on the gains account so the elaborated form is
        // cost-basis-balanced -- giving `.dop` files a uniform shape
        // across frontends. See #210.
        hir.global_context.balance_mode = crate::resolution::BalanceMode::AtPriceWithSynthesis {
            gains_account: "Income:Capital Gains".to_string(),
        };
        Ok(hir)
    }
}

// ---
// Unit tests
// ---
#[cfg(test)]
mod tests {
    use rust_decimal::dec;

    use super::*;

    // -- simple transaction ----------------------------------------------------

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

    // -- pending status, code, inline comment, posting note ------------------─

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

    // -- balance assertion (=) ------------------------------------------------─

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

    // -- strict balance assertion (==) ----------------------------------------─

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

    // -- balance assignment (= target, no LHS amount) --------------------------

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

    // -- lot pricing @ --------------------------------------------------------─

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

    // -- lot pricing @@ --------------------------------------------------------

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

    // -- historical price ------------------------------------------------------

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

    // -- account directive ----------------------------------------------------─

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

    // -- commodity directive --------------------------------------------------─

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

    // -- date separators ------------------------------------------------------─

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

    // -- hash comment --------------------------------------------------------─

    #[test]
    fn hash_comment_line() {
        let input = "# This is a hash comment\n";
        let journal = parse_hledger(input).expect("parse");
        assert_eq!(journal.entries.len(), 1);
        assert!(matches!(journal.entries[0], Entry::Comment(_)));
    }

    // -- negative amount ------------------------------------------------------─

    #[test]
    fn negative_prefix_amount() {
        let input = "2024-01-01 Test\n    assets:bank  $-110.00\n    income:salary\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        let p = &tx.postings[0];
        // $-110.00 -> Unary(Sub, Amount($, 110.00))
        assert!(p.amount.is_some());
    }

    // -- amount with comma thousands separator --------------------------------─

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

    // -- arithmetic in posting amount ------------------------------------------

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

    // -- D default-commodity directive ----------------------------------------─

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
        // `D 1000.00` carries no commodity -- the parser must reject it with a
        // message that mentions "commodity" so the user knows what is missing.
        let err = parse_hledger("D 1000.00\n").unwrap_err();
        assert!(
            err.to_string().contains("commodity"),
            "error should mention 'commodity', got: {err}"
        );
    }

    // -- periodic transaction is silently ignored ------------------------------

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

    // -- virtual postings ------------------------------------------------------

    #[test]
    fn virtual_unbalanced_posting_parses_to_correct_kind() {
        // `(Equity:Reservations)` -- parentheses denote a virtual-unbalanced
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
        // `[Equity:Reservations]` -- square brackets denote a virtual-balanced posting.
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

    // -- full sample-journal smoke test ----------------------------------------

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

    // ---
    // Lot annotation grammar tests
    // ---
    #[test]
    fn test_lot_annotation_cost_only() {
        let input = "2024-03-01 Buy\n    assets:brokerage   10 AAPL {$150}\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction")
        };
        let details = tx.postings[0].amount.as_ref().expect("amount present");
        let AmountDetails::Amount { lot_annotation, .. } = details else {
            panic!("expected Amount details")
        };
        let ann = lot_annotation.as_ref().expect("lot annotation present");
        assert!(ann.cost.is_some(), "cost should be set");
        assert!(ann.date.is_none(), "date should be absent");
        assert!(ann.note.is_none(), "note should be absent");
    }

    #[test]
    fn test_lot_annotation_date_only() {
        let input =
            "2024-03-01 Buy\n    assets:brokerage   10 AAPL [2024-01-15]\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction")
        };
        let details = tx.postings[0].amount.as_ref().expect("amount present");
        let AmountDetails::Amount { lot_annotation, .. } = details else {
            panic!("expected Amount details")
        };
        let ann = lot_annotation.as_ref().expect("lot annotation present");
        assert!(ann.cost.is_none(), "cost should be absent");
        assert_eq!(
            ann.date,
            chrono::NaiveDate::from_ymd_opt(2024, 1, 15),
            "date should be 2024-01-15"
        );
        assert!(ann.note.is_none(), "note should be absent");
    }

    #[test]
    fn test_lot_annotation_note_only() {
        let input =
            "2024-03-01 Buy\n    assets:brokerage   10 AAPL ((BUY-2024-01))\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction")
        };
        let details = tx.postings[0].amount.as_ref().expect("amount present");
        let AmountDetails::Amount { lot_annotation, .. } = details else {
            panic!("expected Amount details")
        };
        let ann = lot_annotation.as_ref().expect("lot annotation present");
        assert!(ann.cost.is_none(), "cost should be absent");
        assert!(ann.date.is_none(), "date should be absent");
        assert_eq!(ann.note.as_deref(), Some("BUY-2024-01"));
    }

    #[test]
    fn test_lot_annotation_combined() {
        let input = "2024-03-01 Buy\n    assets:brokerage   10 AAPL {$150} [2024-03-01] ((BUY-2024-01))\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction")
        };
        let details = tx.postings[0].amount.as_ref().expect("amount present");
        let AmountDetails::Amount { lot_annotation, .. } = details else {
            panic!("expected Amount details")
        };
        let ann = lot_annotation.as_ref().expect("lot annotation present");
        assert!(ann.cost.is_some(), "cost should be set");
        assert_eq!(
            ann.date,
            chrono::NaiveDate::from_ymd_opt(2024, 3, 1),
            "date should be 2024-03-01"
        );
        assert_eq!(ann.note.as_deref(), Some("BUY-2024-01"));
    }

    #[test]
    #[test]
    fn block_comment_closed_and_unclosed() {
        // Closed `comment` ... `end comment` block in the middle of a journal.
        let closed = "\
2024-01-01 * Salary
    Assets:Bank      $100
    Income:Salary

comment
this is ignored
    Assets:Fake     $999
end comment

2024-01-02 * Coffee
    Expenses:Food    $5
    Assets:Bank
";
        let journal = parse_hledger(closed).expect("parse closed block-comment");
        let txs: Vec<_> = journal
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Transaction(_)))
            .collect();
        assert_eq!(
            txs.len(),
            2,
            "block comment must not consume the second txn"
        );

        // Unclosed block at EOF: hledger treats it as comment-to-EOF.
        let unclosed = "\
2024-01-01 * Salary
    Assets:Bank      $100
    Income:Salary

comment
trailing notes about the journal
nothing after this matters
";
        let journal = parse_hledger(unclosed).expect("parse unclosed block-comment");
        let txs: Vec<_> = journal
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Transaction(_)))
            .collect();
        assert_eq!(
            txs.len(),
            1,
            "the single transaction before the unclosed block must parse"
        );
    }

    #[test]
    fn assertion_op_recognises_star_qualifier() {
        // The grammar accepts `=`, `==`, `=*`, `==*`. The `*` qualifier
        // routes to a different AmountDetails variant for the elaborator.
        let input = "2024-12-31 retain earnings\n    Income      ==* 0\n    Equity:RE\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!()
        };
        assert!(
            matches!(
                tx.postings[0].amount,
                Some(AmountDetails::BalanceAssignmentAllCommodities(_))
            ),
            "expected BalanceAssignmentAllCommodities, got {:?}",
            tx.postings[0].amount
        );
        // And the `=*` (weak strict-zero) form takes the same variant.
        let input2 = "2024-12-31 retain earnings\n    Income      =* 0\n    Equity:RE\n";
        let j2 = parse_hledger(input2).expect("parse =*");
        let Entry::Transaction(tx2) = &j2.entries[0] else {
            panic!()
        };
        assert!(matches!(
            tx2.postings[0].amount,
            Some(AmountDetails::BalanceAssignmentAllCommodities(_))
        ));
    }

    /// `==*` aggregates the named account's full subtree: the synthesized
    /// posting on the parent must offset the sum across descendants.
    /// See #207.
    #[test]
    fn assertion_star_aggregates_subtree() {
        let input = "\
2024-01-15 * Salary
    Assets:Cash:USD          200.00 USD
    Income:Salary           -200.00 USD

2024-02-01 * Royalty
    Assets:Cash:EUR           30.00 EUR
    Income:Royalties         -30.00 EUR

2024-12-31 * Retain earnings
    Income                    ==* 0
    Equity:Retained-Earnings
";
        let ast = parse_hledger(input).expect("parse");
        let hir: crate::resolution::HIR = ast.try_into().expect("resolution");
        let journal: crate::elaboration::Journal = crate::elaborate(hir).expect("elaboration");
        let retain = journal
            .transactions
            .iter()
            .find(|t| t.description == "Retain earnings")
            .expect("retain earnings tx");
        let income = retain
            .postings
            .iter()
            .find(|p| p.account == "Income")
            .expect("synthesized Income posting");
        assert_eq!(income.amount_in("USD"), Some(dec!(200.00)));
        assert_eq!(income.amount_in("EUR"), Some(dec!(30.00)));
        let re = retain
            .postings
            .iter()
            .find(|p| p.account == "Equity:Retained-Earnings")
            .expect("retained-earnings absorber");
        assert_eq!(re.amount_in("USD"), Some(dec!(-200.00)));
        assert_eq!(re.amount_in("EUR"), Some(dec!(-30.00)));
    }

    /// hledger's `account Foo ; type:R` tag is captured as metadata on
    /// the declared account; the elaborator's existing ancestor walk
    /// then propagates it to undeclared descendants. No tree type, no
    /// new path — generic metadata inheritance already covers the type
    /// tag. Pinning this here so the behaviour can't silently regress.
    #[test]
    fn account_type_tag_inherits_to_undeclared_descendant() {
        let input = "\
account Income          ; type:R

2024-01-15 * Salary
    Income:Salary    -100 USD
    Assets:Cash       100 USD
";
        let ast = parse_hledger(input).expect("parse");
        let hir: crate::resolution::HIR = ast.try_into().expect("resolution");
        let journal: crate::elaboration::Journal = crate::elaborate(hir).expect("elaboration");
        let salary = journal
            .accounts
            .get("Income:Salary")
            .expect("Income:Salary in elaborated accounts");
        assert_eq!(
            salary.metadata.get("type").map(String::as_str),
            Some("R"),
            "type:R declared on parent Income must propagate to undeclared Income:Salary; \
             got metadata = {:?}",
            salary.metadata
        );
    }

    #[test]
    fn test_lot_annotation_double_brace_total_form() {
        // `{{$1500}}` declares the total lot cost. The adapter records
        // `cost_is_total = true` and stores the inner expression
        // verbatim; the elaborator (covered by integration tests in the
        // doppio crate) divides by the unit count to derive per-unit
        // basis. Here we only assert the AST shape.
        let input = "2024-03-01 Buy\n    assets:brokerage   10 AAPL {{$1500}}\n    assets:cash\n";
        let journal = parse_hledger(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction")
        };
        let details = tx.postings[0].amount.as_ref().expect("amount present");
        let AmountDetails::Amount { lot_annotation, .. } = details else {
            panic!("expected Amount details")
        };
        let ann = lot_annotation.as_ref().expect("lot annotation present");
        assert!(ann.cost_is_total, "double-brace form sets cost_is_total");
        assert!(ann.cost.is_some(), "cost expression captured");
    }
}
