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
//! | `#tag` / `^link` on a transaction | collected into `Transaction.tags`. `#tag` becomes a bare tag (`vacation`); `^link` keeps its prefix (`^trip-001`), so a Beancount link is a doppio tag whose name starts with `^`. |
//!
//! ## Known limitations / stubs
//!
//! - The lot syntax `{cost, date, label}` is parsed best-effort (TODO #185):
//!   comma-split parts are classified as cost (first amount-looking part),
//!   date (ISO), or quoted label. The wildcard `{*}` form is accepted but
//!   the wildcard is silently dropped, and the cost commodity vs held
//!   commodity distinction is collapsed.
//! - The total-cost `{{total}}` form is supported: the adapter records
//!   `cost_is_total = true` on the lot annotation and the elaborator
//!   divides by the posting's unit count to derive per-unit basis.
//!   The proto wire format always carries the canonical per-unit form.
//! - `pad` is preserved as an [`Entry::Pad`] marker but the elaborator does
//!   not yet act on it; the algorithm is the subject of #147.
//! - Org-mode outline headings (`*`, `**`, `***`, ... at column 0)
//!   are accepted and silently dropped, matching Beancount itself.
//! - Shebang lines (`#!/usr/bin/env bean-check`) and Org-mode
//!   file-level startup directives (`#+TITLE: ...`, `#+STARTUP:
//!   showall`, etc.) are accepted and silently dropped.
//!
//! ## String escape sequences
//!
//! Recognised inside double-quoted strings: `\\`, `\"`, `\n`, `\t`,
//! `\r`. Unrecognised escapes (e.g. `\q`) pass through verbatim with
//! the leading backslash preserved, rather than erroring. An escaped
//! quote inside a string literal does not terminate the string.
//!
//! ## Lexically-scoped tag and metadata directives
//!
//! `pushtag #foo` / `poptag #foo` and `pushmeta key: value` /
//! `popmeta key:` are supported. The active set persists across
//! `include` directives (matching Beancount). Mismatched pops are
//! silently ignored. Active pushtags are unioned with each
//! transaction's own `#tag`/`^link` set; active pushmetas are added
//! to the transaction's metadata map (where most-recent-pushed wins
//! per key).

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
    /// Lexically-scoped tags pushed by `pushtag` and removed by
    /// `poptag`. Stored as a stack so multiple pushes of the same tag
    /// are independent (each pop removes the most recent occurrence).
    /// Persists across recursive `include` resolution.
    active_tags: Vec<String>,
    /// Lexically-scoped metadata key/value pairs pushed by `pushmeta`
    /// and removed by `popmeta`. Stack semantics mirror `active_tags`
    /// so the most-recent value wins for a given key.
    active_meta: Vec<(String, String)>,
    /// `option "key" "value"` directives encountered during the parse.
    /// Captured here (rather than just dropped as `Entry::Comment`) so
    /// the frontend can interpret options like
    /// `inferred_tolerance_default` after the parse.
    options: Vec<(String, String)>,
}

impl<F: Fn(&str) -> Result<String, Box<dyn std::error::Error>>> Parser<F> {
    fn parse(&mut self, input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
        let pairs = BeancountParser::parse(Rule::journal, input)?;
        let mut entries = Vec::new();

        for pair in pairs.into_iter().next().unwrap().into_inner() {
            match pair.as_rule() {
                Rule::transaction => {
                    let mut tx = parse_transaction(pair)?;
                    // Apply active pushtags as an additional bare-tag note
                    // (resolve_metadata splits each `:a:b:` note independently
                    // so multiple tag-bearing notes union cleanly).
                    if !self.active_tags.is_empty() {
                        tx.notes.push(format!(":{}:", self.active_tags.join(":")));
                    }
                    // Apply active pushmetas as additional `key: value` notes.
                    // Iterate in stack order so the most recently pushed value
                    // for any given key wins (resolve_metadata's BTreeMap
                    // overwrites on duplicate insert).
                    for (k, v) in &self.active_meta {
                        tx.notes.push(format!("{k}: {v}"));
                    }
                    entries.push(Entry::Transaction(tx));
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
                Rule::option_directive => {
                    // Capture the (key, value) for later interpretation
                    // by BeancountFrontend (e.g. inferred_tolerance_default).
                    // Also push as Comment so the source line survives the
                    // round-trip.
                    let raw = pair.as_str().trim().to_string();
                    let mut inner = pair.into_inner();
                    let key = string_inner(inner.next().expect("option key"));
                    let value = string_inner(inner.next().expect("option value"));
                    self.options.push((key, value));
                    entries.push(Entry::Comment(raw));
                }
                Rule::note_directive
                | Rule::document_directive
                | Rule::event_directive
                | Rule::query_directive
                | Rule::custom_directive
                | Rule::plugin_directive => {
                    entries.push(Entry::Comment(pair.as_str().trim().to_string()));
                }
                Rule::pushtag_directive => {
                    // `pushtag #name`: capture the tag (skipping the `#`).
                    let tag_pair = pair
                        .into_inner()
                        .find(|p| p.as_rule() == Rule::tag)
                        .expect("pushtag must contain a tag");
                    self.active_tags.push(tag_pair.as_str()[1..].to_string());
                }
                Rule::poptag_directive => {
                    // `poptag #name`: remove the most recent matching tag.
                    // Mismatched pops are silently ignored (Beancount itself
                    // emits a warning, but we don't have a warning channel).
                    let tag_pair = pair
                        .into_inner()
                        .find(|p| p.as_rule() == Rule::tag)
                        .expect("poptag must contain a tag");
                    let name = &tag_pair.as_str()[1..];
                    if let Some(idx) = self.active_tags.iter().rposition(|t| t == name) {
                        self.active_tags.remove(idx);
                    }
                }
                Rule::pushmeta_directive => {
                    // `pushmeta key: value`: capture the key and the
                    // verbatim value text.
                    let mut inner = pair.into_inner();
                    let key = inner.next().unwrap().as_str().to_string();
                    let value = inner
                        .next()
                        .map(|v| v.as_str().trim().to_string())
                        .unwrap_or_default();
                    self.active_meta.push((key, value));
                }
                Rule::popmeta_directive => {
                    // `popmeta key:`: remove the most recent value pushed
                    // for that key.
                    let mut inner = pair.into_inner();
                    let key = inner.next().unwrap().as_str();
                    if let Some(idx) = self.active_meta.iter().rposition(|(k, _)| k == key) {
                        self.active_meta.remove(idx);
                    }
                }
                Rule::comment_line => {
                    entries.push(Entry::Comment(pair.as_str().to_string()));
                }
                Rule::org_mode_heading | Rule::shebang_line | Rule::org_mode_meta => {
                    // Beancount accepts these as top-level no-ops:
                    //   - `*` / `**` / ... outline headings (#190)
                    //   - `#!/usr/bin/env bean-web` shebangs (#199)
                    //   - `#+STARTUP: showall` org-mode startup (#199)
                    // Preserve as comments so the source line survives
                    // the round-trip; resolution discards them.
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

/// Extract and decode the inner text of a `string` pair: strip the
/// surrounding quotes and interpret recognised backslash escape
/// sequences.
///
/// Recognised: `\\`, `\"`, `\n`, `\t`, `\r`. Unrecognised escapes
/// (e.g. `\q`) pass through verbatim with the leading backslash
/// preserved -- this matches Python's "be lenient on unknown escapes
/// in string literals" stance and avoids losing source bytes.
fn string_inner(pair: Pair<Rule>) -> String {
    // string = ${ "\"" ~ string_inner ~ "\"" }
    let inner_pair = pair
        .into_inner()
        .next()
        .expect("string must contain string_inner");
    decode_string_escapes(inner_pair.as_str())
}

fn decode_string_escapes(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            // Unknown escape: keep the backslash and the following
            // character verbatim. This is intentionally lenient.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            // Trailing lone backslash (only possible if the grammar
            // somehow let one through; preserve it).
            None => out.push('\\'),
        }
    }
    out
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
    let mut tags: Vec<String> = Vec::new();

    for p in header_fields {
        match p.as_rule() {
            Rule::flag => state = parse_flag(p.as_str()),
            Rule::string => payee_or_desc.push(string_inner(p)),
            // Both `#tag` and `^link` land in Transaction.tags, but
            // the `^` prefix is preserved so the Beancount
            // tag-vs-link distinction can be recovered downstream:
            // a Beancount link is encoded as a doppio tag that
            // starts with `^`. The `#` prefix is stripped because
            // every other ledger-cli-style tag is bare.
            Rule::tag => tags.push(p.as_str()[1..].to_string()),
            Rule::link => tags.push(p.as_str().to_string()),
            Rule::note => {
                notes.push(p.into_inner().as_str().trim().to_string());
            }
            _ => {}
        }
    }

    // Emit collected tags/links as a single ledger-cli-style bare-tag
    // note. resolve_metadata splits on `:` and pushes each into
    // Transaction.tags, so multiple tags survive (they would otherwise
    // clobber each other if encoded as `key: value` metadata).
    if !tags.is_empty() {
        notes.push(format!(":{}:", tags.join(":")));
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
                // The `{{total}}` form: the grammar matches it before
                // `{cost}` so the raw token text starts with `{{`. The
                // elaborator divides by the posting's unit count when
                // applying the cost (see #193).
                if p.as_str().starts_with("{{") {
                    lot_annotation.cost_is_total = true;
                }
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
/// # Known gaps (TODO #185)
///
/// - The wildcard `{*}` / `{*, ...}` form ("automatic cost") is silently
///   dropped: there is no AST representation for "infer this lot's cost
///   from the inventory."
/// - The cost commodity is not distinguished from the held commodity:
///   `10 AAPL {150 USD}` should let downstream consumers tell which
///   commodity is the basis vs the held lot, but today the inner cost
///   expression is captured as a single `ValueExpr` with no extra
///   structural hint.
///
/// The total-cost `{{total}}` form is rejected at the call site
/// (`parse_amount_logic`) before this function is invoked; full
/// support is tracked cross-frontend in #193.
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
// Date-order normalisation
// ---
/// Beancount evaluates entries in **date order**, not source order. We
/// stable-sort the AST entries here so the resolver and elaborator can
/// stay source-order-agnostic. Undated entries (directives, comments)
/// keep their original relative position and sort before any dated
/// entry.
///
/// Within the same date, Beancount applies a fixed ordering that
/// matters for the pad/balance interaction (#147) and for balance
/// assertions that should fire BEFORE any same-date transaction
/// touching the asserted account (#212):
///
/// 1. **Pad** -- registers a "pending pad" for the next balance.
/// 2. **Balance** -- assertion fires at the *beginning* of the date,
///    consuming any pending pad. Per Beancount's documentation:
///    "The balance directive applies for the *beginning* of the date."
/// 3. **Transactions / HistoricalPrice / other dated** -- happen
///    during the date, in source order (stable sort tiebreak).
fn sort_entries_by_date(entries: &mut [Entry]) {
    entries.sort_by_key(entry_sort_key);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SameDayOrder {
    /// Pad registers a pending fill before any same-date balance.
    Pad,
    /// Balance asserts at the start of the date, after pad.
    Balance,
    /// Transactions, prices, etc. -- "during" the date.
    During,
}

fn entry_sort_key(entry: &Entry) -> (Option<Date>, SameDayOrder) {
    (entry_date_key(entry), entry_sub_order(entry))
}

fn entry_date_key(entry: &Entry) -> Option<Date> {
    match entry {
        Entry::Transaction(t) => Some(t.date.clone()),
        Entry::Assertion(a) => Some(a.date.clone()),
        Entry::HistoricalPrice(hp) => Some(hp.date.clone()),
        Entry::Pad(p) => Some(p.date.clone()),
        Entry::Directive(_) | Entry::Comment(_) => None,
    }
}

fn entry_sub_order(entry: &Entry) -> SameDayOrder {
    match entry {
        Entry::Pad(_) => SameDayOrder::Pad,
        Entry::Assertion(_) => SameDayOrder::Balance,
        _ => SameDayOrder::During,
    }
}

// ---
// Convenience function (test-only)
// ---
/// Parse Beancount source with no `include` support.
///
/// Any `include` directive in the input is silently resolved to an empty
/// string (no entries pulled in). Useful for unit tests and standalone
/// parsing. Entries are date-sorted before return (matching the
/// Beancount language semantics).
#[cfg(test)]
pub(crate) fn parse_beancount(input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
    Ok(parse_beancount_with_options(input)?.0)
}

/// Like [`parse_beancount`] but also returns any `option` directives
/// captured during the parse. Used by the elaboration test helper to
/// honour `inferred_tolerance_default`.
#[cfg(test)]
pub(crate) fn parse_beancount_with_options(
    input: &str,
) -> Result<(Journal, Vec<(String, String)>), Box<dyn std::error::Error>> {
    let mut parser = Parser {
        opener: |_| Ok(String::new()),
        base_path: PathBuf::new(),
        active_tags: Vec::new(),
        active_meta: Vec::new(),
        options: Vec::new(),
    };
    let mut journal = parser.parse(input)?;
    sort_entries_by_date(&mut journal.entries);
    Ok((journal, parser.options))
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
        let mut parser = Parser {
            opener: |path: &str| opener(path),
            base_path: base_path.to_path_buf(),
            active_tags: Vec::new(),
            active_meta: Vec::new(),
            options: Vec::new(),
        };
        let mut ast_journal = parser.parse(input)?;
        // Beancount is date-ordered, not source-ordered. Sort once at the
        // outermost call (after include resolution has flattened the tree)
        // so resolver + elaborator can treat the entries as already in
        // chronological order.
        sort_entries_by_date(&mut ast_journal.entries);
        let mut hir: crate::resolution::HIR = ast_journal.try_into()?;
        // Beancount applies a per-transaction balance tolerance by
        // default (#198). Default fraction is 0.5 -- half the
        // least-precise posting's decimal place -- matching
        // bean-check's behaviour.
        hir.global_context.tolerance_mode =
            crate::resolution::ToleranceMode::FractionOfSmallestPrecision(
                rust_decimal::Decimal::new(5, 1),
            );
        // Honour `option "inferred_tolerance_default" "COMMODITY:VALUE"`
        // directives. Each occurrence overrides any previous value for
        // the same commodity (last write wins). The directive is
        // file-level; nested includes contribute to the same map.
        for (k, v) in &parser.options {
            if k == "inferred_tolerance_default"
                && let Some((commodity, decimal)) = v.split_once(':')
                && let Ok(d) = decimal.trim().parse::<rust_decimal::Decimal>()
            {
                hir.global_context
                    .tolerance_overrides
                    .insert(commodity.trim().to_string(), d);
            }
        }
        Ok(hir)
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
    fn string_with_escape_sequences() {
        // Escaped quote inside the literal must not terminate the string.
        parse_one(Rule::string, "\"hello \\\" world\"");
        // Common escapes the grammar must accept.
        parse_one(Rule::string, "\"line1\\nline2\"");
        parse_one(Rule::string, "\"col1\\tcol2\"");
        parse_one(Rule::string, "\"path C:\\\\Users\\\\me\"");
    }

    #[test]
    fn decode_string_escapes_recognises_known_sequences() {
        assert_eq!(decode_string_escapes("hello"), "hello");
        assert_eq!(decode_string_escapes(r"line1\nline2"), "line1\nline2");
        assert_eq!(decode_string_escapes(r"col1\tcol2"), "col1\tcol2");
        assert_eq!(decode_string_escapes(r"x\rclear"), "x\rclear");
        assert_eq!(decode_string_escapes(r#"say \"hi\""#), r#"say "hi""#);
        assert_eq!(decode_string_escapes(r"a\\b"), r"a\b");
    }

    #[test]
    fn decode_string_escapes_passes_unknown_through() {
        // \q is not a recognised escape; the backslash + char survive.
        assert_eq!(decode_string_escapes(r"a\qb"), r"a\qb");
        // Trailing lone backslash also survives.
        assert_eq!(decode_string_escapes(r"a\"), r"a\");
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

    #[test]
    fn org_mode_heading_grammar_rule() {
        parse_one(Rule::org_mode_heading, "* Personal");
        parse_one(Rule::org_mode_heading, "** Bank accounts");
        parse_one(Rule::org_mode_heading, "*** Deeply nested");
    }

    #[test]
    fn shebang_and_org_mode_meta_grammar_rules() {
        parse_one(Rule::shebang_line, "#!/usr/bin/env bean-check");
        parse_one(Rule::shebang_line, "#!/usr/bin/python3");
        parse_one(Rule::org_mode_meta, "#+STARTUP: showall");
        parse_one(Rule::org_mode_meta, "#+TITLE: My Personal Journal");
        parse_one(Rule::org_mode_meta, "#+OPTIONS: toc:nil");
    }

    #[test]
    fn beancount_file_with_shebang_and_org_meta_prelude() {
        // Real upstream Beancount files (e.g. the bean-web examples)
        // open with a shebang and one or more org-mode #+ directives;
        // both must be accepted as top-level no-ops.
        let input = "\
#!/usr/bin/env bean-check
#+TITLE: Test journal
#+STARTUP: showall

2024-01-01 open Assets:Cash USD
2024-01-15 * \"Test\"
  Assets:Cash      100.00 USD
  Income:Initial  -100.00 USD
";
        let journal = parse_beancount(input).expect("parse with prelude");
        let txn_count = journal
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Transaction(_)))
            .count();
        assert_eq!(txn_count, 1, "transaction should still be picked up");
        // Shebang + two #+ lines preserved as comments.
        let comment_count = journal
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Comment(_)))
            .count();
        assert!(
            comment_count >= 3,
            "shebang + 2 #+ lines should each become Entry::Comment, got {} comments",
            comment_count
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
    fn tags_and_links_collected_as_bare_tag_note() {
        let input = "\
2024-01-12 * \"Trip\" #vacation #beach ^trip-001
  Expenses:Travel    100.00 USD
  Assets:Bank:Checking
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!();
        };
        // The AST stores them as a single `:a:b:c:` note that resolve_metadata
        // lifts into Transaction.tags.
        assert!(
            tx.notes.iter().any(|n| n == ":vacation:beach:^trip-001:"),
            "expected a bare-tag note (link prefix preserved), got notes={:?}",
            tx.notes
        );
    }

    #[test]
    fn tags_and_links_land_in_resolved_transaction_tags() {
        // End-to-end: parse a Beancount transaction with #tag and ^link, run
        // it through resolution, and verify both kinds end up in
        // Transaction.tags (links collapse onto tags). This guards against the
        // earlier `tag:NAME` encoding which clobbered itself in the metadata
        // map for multi-tag transactions.
        let input = "\
2024-01-01 open Expenses:Travel
2024-01-01 open Assets:Bank:Checking USD

2024-01-12 * \"Trip\" #vacation #beach ^trip-001
  Expenses:Travel        100.00 USD
  Assets:Bank:Checking  -100.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let tx_entry = hir
            .entries
            .iter()
            .find(|e| matches!(e.data, crate::resolution::Entry::Transaction(_)))
            .expect("a transaction entry should be present");
        let crate::resolution::Entry::Transaction(ref tx) = tx_entry.data else {
            unreachable!();
        };
        let tags: std::collections::HashSet<&str> = tx.tags.iter().map(String::as_str).collect();
        assert!(tags.contains("vacation"), "tags={:?}", tx.tags);
        assert!(tags.contains("beach"), "tags={:?}", tx.tags);
        // Beancount `^link`s land in tags too, but with the `^` prefix
        // preserved so consumers can recover the link-vs-tag distinction.
        assert!(tags.contains("^trip-001"), "tags={:?}", tx.tags);
        // Nothing should have leaked into the metadata map.
        assert!(
            !tx.metadata.contains_key("tag"),
            "metadata leak: {:?}",
            tx.metadata
        );
        assert!(
            !tx.metadata.contains_key("link"),
            "metadata leak: {:?}",
            tx.metadata
        );
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
    fn double_brace_total_cost_lot_elaborates_to_per_unit() {
        // `{{1825 USD}}` declares the *total* lot cost; the elaborator
        // divides by 10 (the unit count) to derive a per-unit basis of
        // 182.50, which is what flows through the rest of the pipeline.
        // The transaction balances at $1825 cash either way.
        let total_form = "\
2024-01-01 open Assets:Brokerage USD
2024-01-01 open Assets:Bank:Checking USD

2024-02-15 * \"Apple lot purchase (total-cost form)\"
  Assets:Brokerage    10 AAPL {{1825.00 USD}}
  Assets:Bank:Checking      -1825.00 USD
";
        let per_unit_form = "\
2024-01-01 open Assets:Brokerage USD
2024-01-01 open Assets:Bank:Checking USD

2024-02-15 * \"Apple lot purchase (per-unit form)\"
  Assets:Brokerage    10 AAPL {182.50 USD}
  Assets:Bank:Checking      -1825.00 USD
";
        let total_elab = elaborate(total_form).expect("`{{total}}` should elaborate");
        let per_unit_elab = elaborate(per_unit_form).expect("`{cost}` should elaborate");

        // Both forms should produce identical elaborated lot cost on the
        // brokerage posting (the canonical per-unit form).
        let total_lot = total_elab.transactions[0].postings[0]
            .lot
            .as_ref()
            .expect("lot present (total form)");
        let per_unit_lot = per_unit_elab.transactions[0].postings[0]
            .lot
            .as_ref()
            .expect("lot present (per-unit form)");
        assert_eq!(
            total_lot, per_unit_lot,
            "total and per-unit forms should produce identical elaborated lots"
        );
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
        // without errors.
        let journal = parse_beancount(SAMPLE).expect("parse sample.beancount");
        assert!(!journal.entries.is_empty());
        let _hir: crate::resolution::HIR = journal.try_into().expect("resolve sample.beancount");
    }

    #[test]
    fn sample_fixture_round_trips_through_elaboration() {
        // End-to-end: parse + resolve + elaborate. The pad+balance pair
        // in the fixture (`pad Assets:Bank:Checking Equity:Opening-Balances`
        // followed by `balance Assets:Bank:Checking 5000.00 USD` after a
        // 3400 USD deposit on Jan 12) should reconcile cleanly: the pad
        // synthesizes a 1600 USD balancing transaction so the assertion
        // passes.
        let journal = parse_beancount(SAMPLE).expect("parse sample.beancount");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve sample.beancount");
        let elab: crate::elaboration::Journal = hir
            .try_into()
            .expect("elaborate sample.beancount (pad+balance must reconcile)");
        // The synthesized pad transaction is tagged with metadata `pad: <source>`.
        let pad_tx_count = elab
            .transactions
            .iter()
            .filter(|t| t.metadata.contains_key("pad"))
            .count();
        assert_eq!(
            pad_tx_count, 1,
            "exactly one synthesized pad transaction expected"
        );
    }

    #[test]
    fn frontend_extensions_lists_beancount() {
        use crate::frontend::Frontend;
        assert!(BeancountFrontend.extensions().contains(&"beancount"));
    }

    #[test]
    fn org_mode_outline_journal_parses_and_resolves() {
        // Beancount accepts journals edited as Org-mode outlines (Emacs
        // org-mode is the canonical case but the language itself permits
        // these lines anywhere -- they are top-level no-ops). The
        // headings round-trip through parse + resolution without
        // affecting the directives that follow.
        let input = "\
* Personal
** Bank accounts
2024-01-01 open Assets:Bank:Checking USD
*** Transactions
2024-01-12 * \"Salary\"
  Assets:Bank:Checking      3400.00 USD
  Income:Salary            -3400.00 USD
";
        let journal = parse_beancount(input).expect("parse with org-mode headings");

        // Three headings preserved as Entry::Comment (alongside the
        // open and the transaction).
        let comment_count = journal
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Comment(_)))
            .count();
        assert_eq!(
            comment_count, 3,
            "three org-mode headings preserved as comments, got entries={:?}",
            journal.entries
        );

        let open_count = journal
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Directive(Directive::Account { .. })))
            .count();
        assert_eq!(open_count, 1);
        let txn_count = journal
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Transaction(_)))
            .count();
        assert_eq!(txn_count, 1);

        // Resolution discards the headings (they are comments).
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let resolved_txn_count = hir
            .entries
            .iter()
            .filter(|e| matches!(e.data, crate::resolution::Entry::Transaction(_)))
            .count();
        assert_eq!(resolved_txn_count, 1);
    }

    #[test]
    fn transaction_string_with_embedded_escapes() {
        // The narration contains an escaped quote and an explicit newline
        // sequence. Both must (a) parse without terminating the string
        // early, and (b) be decoded into their actual characters in the
        // resulting Transaction.description.
        let input = "\
2024-01-01 open Expenses:Food
2024-01-01 open Assets:Cash USD

2024-03-10 * \"order #42 \\\"weekly\\\" delivery\\nnote: replace tomatoes\"
  Expenses:Food      12.34 USD
  Assets:Cash       -12.34 USD
";
        let journal = parse_beancount(input).expect("parse");
        let Entry::Transaction(tx) = journal
            .entries
            .iter()
            .find(|e| matches!(e, Entry::Transaction(_)))
            .expect("transaction present")
        else {
            unreachable!()
        };
        assert_eq!(
            tx.description, "order #42 \"weekly\" delivery\nnote: replace tomatoes",
            "string escapes should be decoded into actual characters"
        );
    }

    // -- pushtag/poptag, pushmeta/popmeta (#188 / #189) ---------------------

    fn resolved_tx<'a>(hir: &'a crate::resolution::HIR) -> Vec<&'a crate::resolution::Transaction> {
        hir.entries
            .iter()
            .filter_map(|e| match &e.data {
                crate::resolution::Entry::Transaction(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn pushtag_scopes_tag_to_subsequent_transactions() {
        let input = "\
2024-01-01 open Expenses:Travel
2024-01-01 open Assets:Cash USD

pushtag #trip-2024

2024-06-15 * \"Hotel\"
  Expenses:Travel    100.00 USD
  Assets:Cash       -100.00 USD

2024-06-16 * \"Dinner\"
  Expenses:Travel     40.00 USD
  Assets:Cash        -40.00 USD

poptag #trip-2024

2024-07-01 * \"Outside the trip\"
  Expenses:Travel     20.00 USD
  Assets:Cash        -20.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let txs = resolved_tx(&hir);
        assert_eq!(txs.len(), 3);
        // First two transactions inherit the pushtag, third does not.
        assert!(
            txs[0].tags.iter().any(|t| t == "trip-2024"),
            "txs[0]={:?}",
            txs[0].tags
        );
        assert!(
            txs[1].tags.iter().any(|t| t == "trip-2024"),
            "txs[1]={:?}",
            txs[1].tags
        );
        assert!(
            !txs[2].tags.iter().any(|t| t == "trip-2024"),
            "third transaction must NOT have the popped tag, got: {:?}",
            txs[2].tags
        );
    }

    #[test]
    fn nested_pushtags_compose() {
        let input = "\
2024-01-01 open Expenses:Travel
2024-01-01 open Assets:Cash USD

pushtag #project-acme
pushtag #travel

2024-02-15 * \"Site visit\"
  Expenses:Travel    300.00 USD
  Assets:Cash       -300.00 USD

poptag #travel

2024-02-20 * \"Project meeting (no travel)\"
  Expenses:Travel     50.00 USD
  Assets:Cash        -50.00 USD

poptag #project-acme
";
        let journal = parse_beancount(input).expect("parse");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let txs = resolved_tx(&hir);
        assert_eq!(txs.len(), 2);
        // First transaction has BOTH active pushtags.
        let t0_tags: std::collections::HashSet<&str> =
            txs[0].tags.iter().map(String::as_str).collect();
        assert!(
            t0_tags.contains("project-acme"),
            "tx0 tags={:?}",
            txs[0].tags
        );
        assert!(t0_tags.contains("travel"), "tx0 tags={:?}", txs[0].tags);
        // Second has only `project-acme` -- `travel` was popped.
        let t1_tags: std::collections::HashSet<&str> =
            txs[1].tags.iter().map(String::as_str).collect();
        assert!(t1_tags.contains("project-acme"));
        assert!(!t1_tags.contains("travel"));
    }

    #[test]
    fn pushtag_unions_with_transaction_local_tags() {
        // The transaction's own #tags coexist with pushtags.
        let input = "\
2024-01-01 open Expenses:Food
2024-01-01 open Assets:Cash USD

pushtag #household

2024-03-10 * \"Groceries\" #weekly
  Expenses:Food      50.00 USD
  Assets:Cash       -50.00 USD

poptag #household
";
        let journal = parse_beancount(input).expect("parse");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let txs = resolved_tx(&hir);
        let tags: std::collections::HashSet<&str> =
            txs[0].tags.iter().map(String::as_str).collect();
        assert!(
            tags.contains("household"),
            "pushtag missing: {:?}",
            txs[0].tags
        );
        assert!(
            tags.contains("weekly"),
            "transaction-local tag missing: {:?}",
            txs[0].tags
        );
    }

    #[test]
    fn poptag_with_no_matching_push_is_silently_ignored() {
        // Beancount itself emits a warning on a mismatched pop; we just
        // ignore it (no warning channel today). The journal must still
        // parse and resolve cleanly.
        let input = "\
2024-01-01 open Expenses:Food
2024-01-01 open Assets:Cash USD

poptag #never-pushed

2024-03-10 * \"Lunch\"
  Expenses:Food      10.00 USD
  Assets:Cash       -10.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let txs = resolved_tx(&hir);
        assert_eq!(txs.len(), 1);
        assert!(
            txs[0].tags.is_empty(),
            "transaction should have no tags, got {:?}",
            txs[0].tags
        );
    }

    #[test]
    fn pushmeta_scopes_metadata_to_subsequent_transactions() {
        let input = "\
2024-01-01 open Expenses:Consulting
2024-01-01 open Assets:Cash USD

pushmeta project: \"acme-rebrand\"

2024-06-15 * \"Design review\"
  Expenses:Consulting   500.00 USD
  Assets:Cash          -500.00 USD

popmeta project:

2024-07-01 * \"Outside the project\"
  Expenses:Consulting   100.00 USD
  Assets:Cash          -100.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let txs = resolved_tx(&hir);
        assert_eq!(txs.len(), 2);
        // First inherits the pushmeta. Resolution captures metadata values
        // verbatim from the note text, so the value retains the surrounding
        // quotes.
        assert_eq!(
            txs[0].metadata.get("project").map(String::as_str),
            Some("\"acme-rebrand\""),
            "pushmeta missing on tx0: {:?}",
            txs[0].metadata
        );
        // Second does NOT have the popped metadata key.
        assert!(
            !txs[1].metadata.contains_key("project"),
            "popped metadata leaked: {:?}",
            txs[1].metadata
        );
    }

    #[test]
    fn pushmeta_most_recent_value_wins_for_same_key() {
        // Two pushes of the same key without an intervening pop -- the
        // later push wins.
        let input = "\
2024-01-01 open Expenses:Consulting
2024-01-01 open Assets:Cash USD

pushmeta phase: \"discovery\"
pushmeta phase: \"delivery\"

2024-08-15 * \"Sprint\"
  Expenses:Consulting   1000.00 USD
  Assets:Cash         -1000.00 USD
";
        let journal = parse_beancount(input).expect("parse");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let txs = resolved_tx(&hir);
        assert_eq!(
            txs[0].metadata.get("phase").map(String::as_str),
            Some("\"delivery\""),
            "later pushmeta should win: {:?}",
            txs[0].metadata
        );
    }

    #[test]
    fn pushmeta_does_not_leak_into_transactions_before_the_push() {
        // Date-sort puts a Jan 5 transaction before a Mar 1 pushmeta even
        // when source order has the push first. Pushmeta is a *parse-time*
        // construct and must be applied at parse time -- the active set at
        // the moment a transaction is parsed is what the transaction inherits.
        // (Source order: open, push, txn -- so the txn DOES inherit.)
        let input = "\
2024-01-01 open Expenses:Food
2024-01-01 open Assets:Cash USD

pushmeta tag: \"taxable\"

2024-01-05 * \"Lunch\"
  Expenses:Food     10.00 USD
  Assets:Cash      -10.00 USD

popmeta tag:
";
        let journal = parse_beancount(input).expect("parse");
        let hir: crate::resolution::HIR = journal.try_into().expect("resolve");
        let txs = resolved_tx(&hir);
        assert_eq!(
            txs[0].metadata.get("tag").map(String::as_str),
            Some("\"taxable\"")
        );
    }

    // -- balance tolerance (#198) -------------------------------------------

    #[test]
    fn sub_tolerance_residual_is_absorbed_into_synthetic_account() {
        // 100.00 + (-100.005) = -0.005 USD. Tolerance for the
        // least-precise posting (scale 2) is 0.5 * 10^-2 = 0.005.
        // residual <= tolerance, so the elaborator synthesizes a
        // posting on the empty-string account that absorbs +0.005 USD.
        let input = "\
2024-01-01 open Assets:Cash USD
2024-01-01 open Income:Salary USD

2024-01-15 * \"Sub-tolerance residual\"
  Assets:Cash         100.00 USD
  Income:Salary      -100.005 USD
";
        let elab = elaborate(input).expect("sub-tolerance residual must elaborate");
        // Locate the synthesized rounding posting (account == "").
        let mut found = None;
        for tx in &elab.transactions {
            for p in &tx.postings {
                if p.account.is_empty() {
                    found = Some(p);
                }
            }
        }
        let p = found.expect("a synthesized empty-account posting should exist");
        let amt = p
            .amount
            .as_ref()
            .and_then(|a| a.by_commodity.get("USD"))
            .expect("rounding posting in USD");
        // 0.005 -> mantissa 5, scale 3, mantissa_high 0
        assert_eq!(
            (amt.mantissa_low, amt.mantissa_high, amt.scale),
            (5, 0, 3),
            "rounding posting should absorb +0.005 USD"
        );
    }

    #[test]
    fn over_tolerance_residual_still_errors() {
        // 100.00 + (-100.05) = -0.05 USD. Tolerance for scale-2 postings
        // is 0.005. 0.05 > 0.005 -> reject.
        let input = "\
2024-01-01 open Assets:Cash USD
2024-01-01 open Income:Salary USD

2024-01-15 * \"Over-tolerance residual\"
  Assets:Cash         100.00 USD
  Income:Salary      -100.05 USD
";
        let err = elaborate(input).expect_err("over-tolerance residual must reject");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("TransactionDoesNotBalance"),
            "expected TransactionDoesNotBalance, got: {msg}"
        );
    }

    #[test]
    fn inferred_tolerance_default_option_overrides_per_commodity() {
        // Without the option directive, this 0.05 USD residual is over
        // tolerance and rejects (covered by `over_tolerance_residual_still_errors`).
        // With `option \"inferred_tolerance_default\" \"USD:0.1\"`, the
        // per-commodity override raises USD's tolerance to 0.1 absolute,
        // so the same 0.05 residual is now within tolerance.
        let input = "\
option \"inferred_tolerance_default\" \"USD:0.1\"

2024-01-01 open Assets:Cash USD
2024-01-01 open Income:Salary USD

2024-01-15 * \"Larger residual within override\"
  Assets:Cash         100.00 USD
  Income:Salary      -100.05 USD
";
        let elab = elaborate(input).expect("USD override should accept the 0.05 residual");
        // Synthesized empty-account posting should absorb +0.05 USD.
        let mut found_amt: Option<rust_decimal::Decimal> = None;
        for tx in &elab.transactions {
            for p in &tx.postings {
                if p.account.is_empty() {
                    if let Some(amt) = p.amount.as_ref().and_then(|a| a.by_commodity.get("USD")) {
                        // Reconstruct decimal: mantissa / 10^scale (sign in mantissa_high).
                        let mantissa = (amt.mantissa_high as i128) << 64 | amt.mantissa_low as i128;
                        let d = rust_decimal::Decimal::from_i128_with_scale(mantissa, amt.scale);
                        found_amt = Some(d);
                    }
                }
            }
        }
        assert_eq!(
            found_amt,
            Some(rust_decimal::Decimal::new(5, 2)),
            "rounding posting should absorb +0.05 USD when override permits it"
        );
    }

    #[test]
    fn inferred_tolerance_default_does_not_apply_to_other_commodities() {
        // The override is per-commodity. A USD override doesn't widen
        // EUR tolerance.
        let input = "\
option \"inferred_tolerance_default\" \"USD:0.1\"

2024-01-01 open Assets:Cash:EUR EUR
2024-01-01 open Income:Salary:EUR EUR

2024-01-15 * \"EUR over default tolerance\"
  Assets:Cash:EUR         100.00 EUR
  Income:Salary:EUR      -100.05 EUR
";
        let err = elaborate(input).expect_err("USD override must not affect EUR");
        assert!(format!("{err:?}").contains("TransactionDoesNotBalance"));
    }

    // -- pad evaluator (#147) -----------------------------------------------

    fn elaborate(input: &str) -> Result<crate::elaboration::Journal, Box<dyn std::error::Error>> {
        let (journal, options) = parse_beancount_with_options(input)?;
        let mut hir: crate::resolution::HIR = journal.try_into()?;
        // Match BeancountFrontend's default (fraction = 0.5) so the
        // elaborator applies Beancount-style tolerance.
        hir.global_context.tolerance_mode =
            crate::resolution::ToleranceMode::FractionOfSmallestPrecision(
                rust_decimal::Decimal::new(5, 1),
            );
        for (k, v) in &options {
            if k == "inferred_tolerance_default"
                && let Some((commodity, decimal)) = v.split_once(':')
                && let Ok(d) = decimal.trim().parse::<rust_decimal::Decimal>()
            {
                hir.global_context
                    .tolerance_overrides
                    .insert(commodity.trim().to_string(), d);
            }
        }
        Ok(hir.try_into()?)
    }

    #[test]
    fn pad_fills_gap_to_next_balance_assertion() {
        // Pad on Jan 1, 3400 USD deposit on Jan 12, balance assertion on
        // Jan 15 expects 5000 USD. The pad must synthesize a 1600 USD
        // (= 5000 - 3400) balancing transaction back-dated to Jan 1.
        let input = "\
2024-01-01 open Assets:Bank:Checking USD
2024-01-01 open Income:Salary USD
2024-01-01 open Equity:Opening-Balances

2024-01-01 pad Assets:Bank:Checking Equity:Opening-Balances
2024-01-15 balance Assets:Bank:Checking 5000.00 USD

2024-01-12 *
  Assets:Bank:Checking      3400.00 USD
  Income:Salary            -3400.00 USD
";
        let elab = elaborate(input).expect("elaborate (pad+balance must reconcile)");
        let pad_txs: Vec<_> = elab
            .transactions
            .iter()
            .filter(|t| t.metadata.contains_key("pad"))
            .collect();
        assert_eq!(pad_txs.len(), 1, "expected exactly one synthesized pad txn");
        let pad_tx = pad_txs[0];
        assert_eq!(
            pad_tx.metadata.get("pad").map(String::as_str),
            Some("Equity:Opening-Balances"),
            "pad provenance metadata should record the source account"
        );
        // Date is Jan 1 (pad's date), epoch days. NaiveDate::from_ymd(2024,1,1) -> 19723
        assert_eq!(
            pad_tx.date,
            chrono::NaiveDate::from_ymd_opt(2024, 1, 1)
                .unwrap()
                .signed_duration_since(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
                .num_days() as i32,
            "synthesized pad txn should be back-dated to the pad directive"
        );
        assert_eq!(pad_tx.postings.len(), 2);
        // Target gets +1600, source gets -1600.
        let target = pad_tx
            .postings
            .iter()
            .find(|p| p.account == "Assets:Bank:Checking")
            .expect("target posting present");
        let source = pad_tx
            .postings
            .iter()
            .find(|p| p.account == "Equity:Opening-Balances")
            .expect("source posting present");
        let target_usd = target
            .amount
            .as_ref()
            .and_then(|a| a.by_commodity.get("USD"))
            .expect("target amount in USD");
        let source_usd = source
            .amount
            .as_ref()
            .and_then(|a| a.by_commodity.get("USD"))
            .expect("source amount in USD");
        // 1600.00 -> mantissa 160000, scale 2; mantissa_high == 0 (positive)
        assert_eq!(
            (
                target_usd.mantissa_low,
                target_usd.mantissa_high,
                target_usd.scale
            ),
            (160000, 0, 2),
            "target should receive +1600.00"
        );
        // -1600.00 -> sign-extended high half is -1
        assert_eq!(
            source_usd.mantissa_high, -1,
            "source amount should be negative"
        );
        assert_eq!(source_usd.scale, 2);
    }

    #[test]
    fn pad_without_following_balance_is_silently_dropped() {
        // No `balance` directive on Assets:Bank:Checking after the pad ->
        // pad is silently discarded; no synthesized transaction.
        let input = "\
2024-01-01 open Assets:Bank:Checking USD
2024-01-01 open Equity:Opening-Balances

2024-01-01 pad Assets:Bank:Checking Equity:Opening-Balances
";
        let elab = elaborate(input).expect("elaborate");
        let pad_txs = elab
            .transactions
            .iter()
            .filter(|t| t.metadata.contains_key("pad"))
            .count();
        assert_eq!(
            pad_txs, 0,
            "pad without a follow-up balance assertion must be dropped, not error"
        );
    }

    #[test]
    fn most_recent_pad_wins_when_multiple_precede_one_balance() {
        // Two pads on the same target account before any balance assertion;
        // Beancount keeps only the most recent. The Jan 5 pad's source
        // (Equity:Late-Init) should be the synthesized txn's source, not
        // the Jan 1 pad's (Equity:Opening-Balances).
        let input = "\
2024-01-01 open Assets:Bank:Checking USD
2024-01-01 open Equity:Opening-Balances
2024-01-01 open Equity:Late-Init

2024-01-01 pad Assets:Bank:Checking Equity:Opening-Balances
2024-01-05 pad Assets:Bank:Checking Equity:Late-Init
2024-01-15 balance Assets:Bank:Checking 1000.00 USD
";
        let elab = elaborate(input).expect("elaborate");
        let pad_txs: Vec<_> = elab
            .transactions
            .iter()
            .filter(|t| t.metadata.contains_key("pad"))
            .collect();
        assert_eq!(pad_txs.len(), 1, "exactly one synthesized pad txn");
        let pad_tx = pad_txs[0];
        assert_eq!(
            pad_tx.metadata.get("pad").map(String::as_str),
            Some("Equity:Late-Init"),
            "the most recent pad's source must be used"
        );
        // The synthesized txn's date matches the *winning* pad's date (Jan 5).
        assert_eq!(
            pad_tx.date,
            chrono::NaiveDate::from_ymd_opt(2024, 1, 5)
                .unwrap()
                .signed_duration_since(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
                .num_days() as i32,
        );
    }

    #[test]
    fn pad_with_zero_gap_emits_no_transaction() {
        // If the running balance already equals the asserted amount, the
        // pad has nothing to fill -- no synthesized transaction.
        let input = "\
2024-01-01 open Assets:Bank:Checking USD
2024-01-01 open Income:Salary USD
2024-01-01 open Equity:Opening-Balances

2024-01-01 pad Assets:Bank:Checking Equity:Opening-Balances
2024-01-15 balance Assets:Bank:Checking 1000.00 USD

2024-01-10 *
  Assets:Bank:Checking      1000.00 USD
  Income:Salary            -1000.00 USD
";
        let elab = elaborate(input).expect("elaborate");
        let pad_txs = elab
            .transactions
            .iter()
            .filter(|t| t.metadata.contains_key("pad"))
            .count();
        assert_eq!(
            pad_txs, 0,
            "pad whose gap is zero should not emit a synthesized transaction"
        );
    }
}
