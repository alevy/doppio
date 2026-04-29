//! Ledger-cli frontend: parse `.ledger` source text into an [`ast::Journal`].
//!
//! This module has three layers:
//!
//! 1. **[`LedgerParser`]** — a `pest`-derived parser that tokenises source
//!    text according to the grammar in `ledger.pest`.
//! 2. **[`Parser<F>`]** — the public API that wraps `LedgerParser`, walks
//!    the pair tree, handles `include` directives recursively, and builds
//!    the [`ast::Journal`].
//! 3. **[`LedgerFrontend`]** — implements [`crate::frontend::Frontend`] so
//!    that the CLI can select this parser by file extension.
//!
//! Amount expressions (see [`ast::ValueExpr`]) are parsed using a **Pratt
//! parser** ([`PRATT_PARSER`]) to apply operator precedence: `*` and `/`
//! bind tighter than `+` and `-`. The grammar itself treats the token
//! sequence as flat — precedence is applied as a post-processing step.

use crate::ast::*;
use pest::Parser as _;
use pest::iterators::{Pair, Pairs};
use pest::pratt_parser::PrattParser;
use pest_derive::Parser;
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::sync::LazyLock; // Or once_cell

/// The raw pest parser generated from `ledger.pest` via `pest_derive`.
///
/// This type is only used internally by [`Parser<F>::parse`]. Callers
/// should use [`Parser`] or the convenience function [`parse_ledger`].
#[derive(Parser)]
#[grammar = "grammars/ledger/ledger.pest"]
pub struct LedgerParser;

/// A stateful parser that resolves `include` directives.
///
/// The generic parameter `F` is the file-opener: a callable that accepts a
/// file-system path (potentially a glob pattern) and returns the concatenated
/// contents of all matching files as a `String`, or an error. This design
/// makes the parser testable without touching the file system — pass
/// `|_| Ok(String::new())` for a no-op opener.
///
/// The opener receives the fully joined path (base directory + include
/// argument). For glob patterns (containing `*`, `?`, or `[`) the opener is
/// responsible for expanding and sorting matches; [`crate::file_opener`] does
/// this correctly and is the default for CLI use.
pub struct Parser<F: Fn(&str) -> Result<String, Box<dyn std::error::Error>>> {
    /// Called with the joined include path (or glob pattern) to load included
    /// files. The path may be relative if `base_path` is relative — callers
    /// who need absolute paths should canonicalise `base_path` before parsing.
    ///
    /// Returns the concatenated file contents on success, or a boxed error
    /// if the path does not exist, the glob matches nothing, or a file cannot
    /// be read. The error message is surfaced to the caller of
    /// [`Parser::parse`].
    pub opener: F,
    /// The directory of the file currently being parsed. Used to resolve
    /// relative paths in `include` directives.
    pub base_path: PathBuf,
}

impl<F: Fn(&str) -> Result<String, Box<dyn std::error::Error>>> Parser<F> {
    /// Parse `input` (Ledger-format source text) into an [`ast::Journal`].
    ///
    /// `include` directives are expanded inline: the included file's entries
    /// are inserted at the position of the directive in the parent journal.
    /// The `base_path` and `opener` fields are used to locate included files.
    ///
    /// # Errors
    ///
    /// Returns a boxed error if:
    /// - the source text is syntactically invalid (pest parse error), or
    /// - an `include` directive's opener call fails (e.g. file not found,
    ///   glob with no matches, I/O error).
    pub fn parse(&mut self, input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
        let pairs = LedgerParser::parse(Rule::journal, input)?;
        let mut entries = Vec::new();

        for pair in pairs.into_iter().next().unwrap().into_inner() {
            match pair.as_rule() {
                Rule::transaction => {
                    entries.push(Entry::Transaction(parse_transaction(pair)?));
                }
                Rule::comment_line => {
                    entries.push(Entry::Comment(pair.as_str().to_string()));
                }
                Rule::commodity_directive => {
                    entries.push(Entry::Directive(parse_commodity_directive(pair)));
                }
                Rule::account_directive => {
                    entries.push(Entry::Directive(parse_account_directive(pair)));
                }
                Rule::alias_directive => {
                    entries.push(Entry::Directive(parse_alias_directive(pair)));
                }
                Rule::define_directive => {
                    entries.push(Entry::Directive(parse_define_directive(pair)));
                }
                Rule::tag_directive => {
                    entries.push(Entry::Directive(parse_tag_directive(pair)));
                }
                Rule::default_directive => {
                    entries.push(Entry::Directive(parse_default_directive(pair)?));
                }
                Rule::historical_price => {
                    entries.push(Entry::HistoricalPrice(parse_historical_price(pair)));
                }
                Rule::assertion_directive => {
                    entries.push(Entry::Assertion(parse_assertion_directive(pair)));
                }
                Rule::include_directive => {
                    // Join the included path with the current base directory so
                    // relative paths (e.g. "include accounts/*.ledger") work
                    // regardless of the process working directory.
                    let include_path = self.base_path.join(pair.into_inner().as_str());
                    let new_input = (self.opener)(include_path.as_os_str().to_str().unwrap())?;
                    // Temporarily update base_path to the included file's directory
                    // so that any further includes within it are resolved correctly.
                    let new_base_path = include_path
                        .parent()
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| self.base_path.clone());
                    let old_base_path = std::mem::replace(&mut self.base_path, new_base_path);
                    entries.append(&mut self.parse(&new_input)?.entries);
                    // Restore the original base_path for subsequent entries in
                    // the parent file.
                    let _ = std::mem::replace(&mut self.base_path, old_base_path);
                }
                Rule::budget => {
                    // Budget entries (`~ monthly ...`) are intentionally not
                    // modelled in this implementation. They represent
                    // planned/expected amounts in ledger-cli but have no effect
                    // on balances or reports. See GitHub issue #13.
                }
                _ => {}
            }
        }

        let journal = Journal { entries };
        validate_regexes(&journal)?;
        Ok(journal)
    }
}

/// Walk the parsed [`Journal`] and verify every regex literal compiles.
///
/// Regex patterns are stored as raw strings in the AST so the AST itself
/// stays free of `regex` types. Validation here surfaces invalid patterns at
/// parse time — without it, a typo like `assert tag("X") =~ /[unclosed/`
/// would only fail much later during elaboration, when the failing posting
/// is encountered.
fn validate_regexes(journal: &Journal) -> Result<(), Box<dyn std::error::Error>> {
    for entry in &journal.entries {
        match entry {
            Entry::Directive(Directive::Account { items, .. }) => {
                for item in items {
                    match item {
                        AccountItem::Assert(e) | AccountItem::Check(e) => {
                            validate_bool_expr_regexes(e)?;
                        }
                        _ => {}
                    }
                }
            }
            Entry::Directive(Directive::Tag {
                asserts, checks, ..
            }) => {
                for e in asserts.iter().chain(checks.iter()) {
                    validate_bool_expr_regexes(e)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_bool_expr_regexes(expr: &BoolExpr) -> Result<(), Box<dyn std::error::Error>> {
    if let Some((_, ValueExpr::Regex(pattern))) = &expr.cmp {
        regex::Regex::new(pattern).map_err(|e| format!("invalid regex /{pattern}/: {e}"))?;
    }
    if let Some((_, cont)) = &expr.chain {
        validate_bool_expr_regexes(cont)?;
    }
    Ok(())
}

/// Convenience function: parse Ledger source with no `include` support.
///
/// Useful in tests and benchmarks where a self-contained string is parsed
/// and no file I/O is needed. Any `include` directives in the input are
/// silently resolved to an empty string (no entries are included).
pub fn parse_ledger(input: &str) -> Result<Journal, Box<dyn std::error::Error>> {
    Parser {
        opener: |_| Ok(String::new()),
        base_path: PathBuf::new(),
    }
    .parse(input)
}

// ──────────────────────────────────────────────────────────────────────────────
// Frontend impl
// ──────────────────────────────────────────────────────────────────────────────

/// The ledger-cli file-format frontend.
///
/// Recognises `.ledger` files and delegates to [`Parser`] / [`parse_ledger`]
/// for the actual parsing work.  Pass this (or a `Box<dyn Frontend>` wrapping
/// it) to code that needs a type-erased frontend handle.
///
/// # Example
///
/// ```rust
/// use doppio::frontend::Frontend as _;
/// use doppio::LedgerFrontend;
/// use std::path::Path;
///
/// let journal = LedgerFrontend
///     .parse(
///         "2024-01-01 Test\n  Expenses:Food  $10\n  Assets:Cash\n",
///         Path::new(""),
///         &|_| Ok(String::new()),
///     )
///     .unwrap();
/// assert_eq!(journal.entries.len(), 1);
/// ```
pub struct LedgerFrontend;

impl crate::frontend::Frontend for LedgerFrontend {
    fn extensions(&self) -> &'static [&'static str] {
        &["ledger"]
    }

    fn parse(
        &self,
        input: &str,
        base_path: &std::path::Path,
        opener: &crate::frontend::Opener,
    ) -> Result<crate::ast::Journal, Box<dyn std::error::Error>> {
        // Wrap the dyn-Fn opener into a concrete closure so we can store it in
        // the generic Parser<F>. This adds one indirection per include call,
        // which is acceptable because includes are rare compared to parse work.
        Parser {
            opener: |path: &str| opener(path),
            base_path: base_path.to_path_buf(),
        }
        .parse(input)
    }
}

fn parse_assertion_directive(pair: Pair<Rule>) -> AssertionDirective {
    let mut inner = pair.into_inner();
    let date = parse_date(&mut inner.next().unwrap().into_inner());
    let op_pair = inner.next().unwrap();
    let strict = op_pair.as_str() == "==";
    let account = inner.next().unwrap().as_str().trim().to_string();
    let amount = parse_expr(inner.next().unwrap());
    AssertionDirective {
        date,
        account,
        amount,
        strict,
    }
}

fn parse_historical_price(pair: Pair<Rule>) -> HistoricalPrice {
    let mut inner = pair.into_inner();
    let date = parse_date(&mut inner.next().unwrap().into_inner());
    let mut time = None;
    let mut commodity = String::new();
    let mut price_pair = None;

    for p in inner {
        match p.as_rule() {
            Rule::time => time = Some(p.as_str().to_string()),
            Rule::commodity => commodity = p.as_str().to_string(),
            Rule::value_expr => price_pair = Some(p),
            _ => {}
        }
    }

    HistoricalPrice {
        date,
        time,
        commodity,
        price: parse_expr(price_pair.expect("historical_price must have a price")),
    }
}

fn parse_alias_directive(pair: Pair<Rule>) -> Directive {
    let mut pairs = pair.into_inner();
    let alias = pairs.next().unwrap().as_str().trim().to_string();
    let account = pairs.next().unwrap().as_str().trim().to_string();
    Directive::Alias { alias, account }
}

fn parse_define_directive(pair: Pair<Rule>) -> Directive {
    let mut pairs = pair.into_inner().peekable();

    // First child is always the macro name.
    let name = pairs.next().unwrap().as_str().to_string();

    // Collect any parameter identifiers that precede the define_body.
    // All inner pairs before the final `define_body` are parameter identifiers.
    let mut params = Vec::new();
    let mut body_pair = None;
    for p in pairs {
        match p.as_rule() {
            Rule::identifier => params.push(p.as_str().to_string()),
            Rule::define_body => {
                body_pair = Some(p);
            }
            _ => {}
        }
    }

    let body_pair = body_pair.expect("define_directive must have a define_body");
    // define_body = ${ bool_expr | value_expr }
    // Because `bool_expr` is `value_expr ~ (cmp ~ rhs)? ~ (bool_op ~ bool_expr)?`,
    // the `bool_expr` alternative always matches when `value_expr` does. We
    // distinguish the two cases by checking whether the parsed `BoolExpr` actually
    // carries a comparison or a chain:
    //   - If it has a `cmp` or `chain`, it is a genuine boolean expression.
    //   - If neither, it is a plain value expression wrapped in a trivial BoolExpr;
    //     we unwrap it into `DefineBody::Value` so the evaluator handles it correctly.
    let inner = body_pair
        .into_inner()
        .next()
        .expect("define_body must have one child");

    let body = match inner.as_rule() {
        Rule::bool_expr => {
            let bool_expr = parse_bool_expr(inner);
            // Unwrap a trivial bool_expr (no comparison, no chain) → Value.
            if bool_expr.cmp.is_none() && bool_expr.chain.is_none() {
                DefineBody::Value(bool_expr.lhs)
            } else {
                DefineBody::Bool(bool_expr)
            }
        }
        Rule::value_expr => DefineBody::Value(parse_expr(inner)),
        r => unreachable!("unexpected rule in define_body: {r:?}"),
    };

    Directive::Define { name, params, body }
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
            Rule::account_assert => {
                let expr = parse_bool_expr(p.into_inner().next().unwrap());
                items.push(AccountItem::Assert(expr));
            }
            Rule::account_check => {
                let expr = parse_bool_expr(p.into_inner().next().unwrap());
                items.push(AccountItem::Check(expr));
            }
            _ => {}
        }
    }

    Directive::Account { name, notes, items }
}

fn parse_tag_directive(pair: Pair<Rule>) -> Directive {
    let mut inner = pair.into_inner();
    let name = inner.next().unwrap().as_str().to_string();
    let mut asserts = Vec::new();
    let mut checks = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::note => {
                // Header-level note on the tag directive line — discarded.
            }
            Rule::tag_assert => {
                let expr = parse_bool_expr(p.into_inner().next().unwrap());
                asserts.push(expr);
            }
            Rule::tag_check => {
                let expr = parse_bool_expr(p.into_inner().next().unwrap());
                checks.push(expr);
            }
            _ => {}
        }
    }

    Directive::Tag {
        name,
        asserts,
        checks,
    }
}

fn parse_commodity_directive(pair: Pair<Rule>) -> Directive {
    let mut inner = pair.into_inner();
    let name = inner.next().unwrap().as_str().to_string();
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

    Directive::Commodity { name, notes, items }
}

/// Parse a bare `D <amount>` directive into a [`Directive::Commodity`] AST node.
///
/// The `D` directive is a compact form that declares the default commodity and
/// its display format simultaneously. `D $1000.00` lowers to the same internal
/// representation as:
///
/// ```ledger
/// commodity $
///     default
///     format $1,000.00
/// ```
///
/// The commodity symbol is extracted from the value expression. If the expression
/// carries no commodity (e.g. `D 1000.00` — a bare number), an error is returned
/// because there is no symbol to register as the default.
fn parse_default_directive(pair: Pair<Rule>) -> Result<Directive, Box<dyn std::error::Error>> {
    // The single inner child of `default_directive` is the `value_expr`.
    let value_expr_pair = pair
        .into_inner()
        .next()
        .expect("default_directive must contain a value_expr");

    // Capture the raw source text *before* consuming the pair — this becomes the
    // format string. Trim trailing whitespace and newline.
    let format_str = value_expr_pair.as_str().trim().to_string();

    let parsed = parse_expr(value_expr_pair);

    // Extract the commodity symbol from the parsed expression.
    // A `D $1000.00` parses as `Amount { value: 1000.00, commodity: Some("$") }`.
    // A `D 1,000.00 USD` parses as `Amount { value: 1000.00, commodity: Some("USD") }`.
    let commodity = match &parsed {
        ValueExpr::Amount {
            commodity: Some(c), ..
        } => c.clone(),
        _ => {
            return Err(format!(
                "bare `D` directive requires an amount with an explicit commodity symbol \
                 (e.g. `D $1000.00`); got `{format_str}` which carries no commodity"
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

/// Parse a `bool_expr` grammar node into a [`BoolExpr`] AST node.
///
/// The grammar rule is:
/// ```text
/// bool_expr = ${
///     value_expr ~ (
///         (regex_cmp_op ~ regex_literal)
///         | (cmp_op ~ value_expr)
///     )? ~ (bool_op ~ bool_expr)?
/// }
/// ```
fn parse_bool_expr(pair: Pair<Rule>) -> BoolExpr {
    let mut inner = pair.into_inner().peekable();

    // First child is always the LHS value_expr.
    let lhs = parse_expr(inner.next().expect("bool_expr must have lhs"));

    // Next child (if any) is either cmp_op or regex_cmp_op.
    let cmp = match inner.peek().map(|p| p.as_rule()) {
        Some(Rule::cmp_op) => {
            let op_pair = inner.next().unwrap();
            let op = match op_pair.as_str() {
                "==" => CmpOp::Eq,
                "!=" => CmpOp::Ne,
                "<=" => CmpOp::Le,
                ">=" => CmpOp::Ge,
                "<" => CmpOp::Lt,
                ">" => CmpOp::Gt,
                _ => unreachable!("unknown cmp_op: {}", op_pair.as_str()),
            };
            let rhs = parse_expr(inner.next().expect("cmp_op must be followed by rhs"));
            Some((op, rhs))
        }
        Some(Rule::regex_cmp_op) => {
            let op_pair = inner.next().unwrap();
            let op = match op_pair.as_str() {
                "=~" => CmpOp::RegexMatch,
                "!~" => CmpOp::RegexNotMatch,
                _ => unreachable!("unknown regex_cmp_op: {}", op_pair.as_str()),
            };
            // The next token must be a regex_literal.
            let regex_pair = inner
                .next()
                .expect("regex_cmp_op must be followed by regex_literal");
            // regex_literal = ${ "/" ~ regex_body ~ "/" }
            // Its single inner child is the regex_body @-rule carrying the raw pattern.
            let pattern = regex_pair
                .into_inner()
                .next()
                .expect("regex_literal must have regex_body")
                .as_str()
                .to_string();
            Some((op, ValueExpr::Regex(pattern)))
        }
        _ => None,
    };

    // Remaining child (if any) is bool_op + bool_expr continuation.
    let chain = if inner.peek().map(|p| p.as_rule()) == Some(Rule::bool_op) {
        let op_pair = inner.next().unwrap();
        let op = match op_pair.as_str() {
            "and" => BoolOp::And,
            "or" => BoolOp::Or,
            _ => unreachable!("unknown bool_op: {}", op_pair.as_str()),
        };
        let cont = parse_bool_expr(inner.next().expect("bool_op must be followed by bool_expr"));
        Some((op, Box::new(cont)))
    } else {
        None
    };

    BoolExpr { lhs, cmp, chain }
}

fn parse_account_item(pair: Pair<Rule>) -> AccountItem {
    let mut inner = pair.into_inner();
    let key_pair = inner.next().unwrap();
    let key = key_pair.as_str();
    // Look for value and trailing note
    let mut val = None;

    for p in inner {
        if p.as_rule() == Rule::account_val {
            val = Some(p.as_str().trim().to_string())
        }
    }

    match key {
        "alias" => AccountItem::Alias(val.unwrap_or_default()),
        "note" => AccountItem::Note(val.unwrap_or_default()),
        _ => AccountItem::Unknown(key.to_string(), val),
    }
}

fn parse_commodity_item(pair: Pair<Rule>) -> CommodityItem {
    let mut inner = pair.into_inner();
    let key_pair = inner.next().unwrap();
    let key = key_pair.as_str();
    // Look for value and trailing note
    let mut val = None;

    for p in inner {
        if p.as_rule() == Rule::commodity_val {
            val = Some(p.as_str().trim().to_string())
        }
    }

    match key {
        "alias" => CommodityItem::Alias(val.unwrap_or_default()),
        "format" => CommodityItem::Format(val.unwrap_or_default()),
        "nomarket" => CommodityItem::NoMarket,
        "default" => CommodityItem::Default,
        "note" => CommodityItem::Note(val.unwrap_or_default()),
        _ => CommodityItem::Unknown(key.to_string(), val),
    }
}

fn parse_date(pairs: &mut Pairs<Rule>) -> Date {
    let mut year: Option<i32> = None;

    let mut p = pairs.next().unwrap();
    if let Rule::year = p.as_rule() {
        year = Some(p.as_str().parse().unwrap());
        p = pairs.next().unwrap();
    }

    let month = p.as_str().parse().unwrap();
    let date = pairs.next().unwrap().as_str().parse().unwrap();

    Date { year, month, date }
}

fn parse_transaction(pair: Pair<Rule>) -> Result<Transaction, Box<dyn std::error::Error>> {
    let mut inner = pair.into_inner();
    let header_pair = inner.next().unwrap();
    let mut postings = Vec::new();
    let mut notes = Vec::new();

    // Process remainder of transaction
    for p in inner {
        match p.as_rule() {
            Rule::transaction_note => {
                // Get the inner note rule
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
    let mut header = header_pair.into_inner();
    let date = parse_date(&mut header.next().unwrap().into_inner());

    let mut secondary_date = None;
    let mut state = TransactionState::Uncleared;
    let mut code = None;
    let mut description = String::new();

    for p in header {
        match p.as_rule() {
            Rule::date => secondary_date = Some(parse_date(&mut p.into_inner())),
            Rule::state => state = parse_state(p.as_str()),
            Rule::code => {
                // Remove parentheses from code
                let s = p.as_str();
                code = Some(s[1..s.len() - 1].to_string());
            }
            Rule::description => description = p.as_str().trim().to_string(),
            Rule::note => notes.push(p.as_str().trim().to_string()),
            _ => {}
        }
    }

    Ok(Transaction {
        date,
        secondary_date,
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
            Rule::state => state = parse_state(p.as_str()),
            Rule::posting_account => {
                // posting_account = virtual_unbalanced_account | virtual_balanced_account | account
                let inner_pair = p
                    .into_inner()
                    .next()
                    .expect("posting_account must have one child");
                match inner_pair.as_rule() {
                    Rule::virtual_unbalanced_account => {
                        kind = PostingKind::VirtualUnbalanced;
                        // Inner child is virtual_account_inner — the bare account name.
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
            Rule::note => notes.push(p.as_str().trim().to_string()),
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
                                parse_lot_annotation_into(child, &mut lot_annotation)?;
                            }
                            _ => unreachable!(),
                        }
                    }
                    Rule::assertion => {
                        let inner_expr_pair = p.into_inner().next().unwrap();
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
            let inner_expr_pair = p.into_inner().next().unwrap();
            Ok(AmountDetails::BalanceAssignment(parse_expr(
                inner_expr_pair,
            )))
        }
        _ => unreachable!(),
    }
}

/// Merge a single `lot_annotation` grammar node into an accumulating
/// [`LotAnnotation`] struct.  Duplicate annotations of the same kind take the
/// last value (matching ledger-cli behaviour).
///
/// # Errors
///
/// Returns an error if a `{{total}}` double-brace lot cost is encountered.
/// The double-brace form is syntactically accepted by the grammar but its
/// per-lot total-cost semantics are not yet implemented.  Treating it silently
/// as a per-unit cost would produce wrong balances (e.g. `10 AAPL {{$1500}}`
/// would yield a cash contribution of $15 000 instead of $1 500).  Use
/// `{cost}` for per-unit cost or `@@ total` for transient total cost instead.
fn parse_lot_annotation_into(
    pair: Pair<Rule>,
    acc: &mut LotAnnotation,
) -> Result<(), Box<dyn std::error::Error>> {
    let child = pair.into_inner().next().unwrap();
    match child.as_rule() {
        Rule::lot_cost => {
            // Reject the `{{expr}}` double-brace form. The grammar matches it
            // before `{expr}` so the raw token text starts with "{{".
            if child.as_str().starts_with("{{") {
                return Err(
                    "double-brace `{{total}}` lot syntax is not yet implemented; \
                     use `{cost}` for per-unit cost or `@@ total` for transient total cost"
                        .into(),
                );
            }
            // lot_cost inner: value_expr — per-unit cost.
            let expr_pair = child.into_inner().next().unwrap();
            acc.cost = Some(parse_expr(expr_pair));
        }
        Rule::lot_date => {
            // lot_date inner: date
            let date_pair = child.into_inner().next().unwrap();
            let date = parse_date_pair(date_pair);
            if let (Some(year), month, day) = (date.year, date.month, date.date) {
                acc.date = chrono::NaiveDate::from_ymd_opt(year, month, day);
            }
        }
        Rule::lot_note => {
            // lot_note inner: lot_note_inner (atomic string)
            let note_str = child.into_inner().next().unwrap().as_str().to_string();
            acc.note = Some(note_str);
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// Parse a `date` grammar pair from the ledger grammar into an [`ast::Date`].
/// Used for lot_date parsing (separate from the transaction-header date parsing).
fn parse_date_pair(pair: Pair<Rule>) -> Date {
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

// The Pratt parser is constructed once at program start via LazyLock.
//
// Building a PrattParser allocates and organises a precedence table.
// Since this is called for every value expression in every posting, and
// the table never changes, it is far cheaper to build it once and share
// the reference across all calls than to rebuild it on each invocation.
static PRATT_PARSER: LazyLock<PrattParser<Rule>> = LazyLock::new(|| {
    use Rule::*;
    use pest::pratt_parser::{Assoc::*, Op};

    PrattParser::new()
        .op(Op::infix(add, Left) | Op::infix(sub, Left))
        .op(Op::infix(mul, Left) | Op::infix(div, Left))
        .op(Op::prefix(prefix_op))
});

/// Parse a `value_expr` pair into a [`ValueExpr`] AST node.
///
/// The grammar's `value_expr` rule is `expr (ws+ commodity)?`. The Pratt
/// parser handles the `expr` part (operator precedence), and then we check
/// for a trailing commodity annotation *after* Pratt parsing completes.
/// This two-step approach is necessary because the trailing commodity is
/// outside the `expr` rule and therefore invisible to the Pratt parser —
/// it needs to be lifted into a `ValueExpr::Typed` wrapper here.
pub(crate) fn parse_expr(pair: Pair<Rule>) -> ValueExpr {
    let mut inner = pair.into_inner();
    let expr_pair = inner.next().expect("Empty value_expr");
    let mut ast = run_pratt(expr_pair.into_inner());

    // Check for trailing commodity (e.g., '(1+2) USD')
    if let Some(comm_pair) = inner.next() {
        ast = ValueExpr::Typed {
            expr: Box::new(ast),
            commodity: comm_pair.as_str().to_string(),
        };
    }
    ast
}

fn run_pratt(pairs: pest::iterators::Pairs<Rule>) -> ValueExpr {
    PRATT_PARSER
        .map_primary(|pair| match pair.as_rule() {
            Rule::term => run_pratt(pair.into_inner()),
            Rule::primary => {
                let mut inner = pair.into_inner();
                // base_primary is a silent rule, so its child arrives directly
                // as the first item in `inner`. We re-enter run_pratt for the
                // base atom so that all the match arms below can be reused.
                let base_pair = inner.next().expect("Primary must have a base");

                // Wrap base_pair in a single-item Pairs so we can pass it to
                // run_pratt, which expects a Pairs iterator.
                let mut ast = run_pratt(pest::iterators::Pairs::single(base_pair));

                // Fold dot-access chains left-to-right into nested Access nodes.
                // Iterative rather than recursive because chains are arbitrary
                // length and we want left-associativity without stack growth.
                for access in inner {
                    if access.as_rule() == Rule::access {
                        let field = access.into_inner().next().unwrap().as_str().to_string();
                        ast = ValueExpr::Access {
                            expr: Box::new(ast),
                            field,
                        };
                    }
                }
                ast
            }
            Rule::amount => {
                let mut inner = pair.into_inner();
                let first = inner.next().unwrap();
                match first.as_rule() {
                    // Prefix-commodity form: "$100" — commodity comes first
                    Rule::commodity => {
                        let comm = first.as_str().to_string();
                        let val_str = inner.next().unwrap().as_str();
                        ValueExpr::Amount {
                            value: clean_parse_decimal(val_str),
                            commodity: Some(comm),
                        }
                    }
                    // Number-first form: "100 USD" or bare "100"
                    Rule::number => {
                        let val = clean_parse_decimal(first.as_str());
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
            Rule::function_call => {
                let mut inner = pair.into_inner();
                let name = inner.next().unwrap().as_str().to_string();
                // Each argument is a full `expr` — recurse via run_pratt so
                // arithmetic inside function arguments is also parsed correctly.
                let args = inner.map(|p| run_pratt(p.into_inner())).collect();
                ValueExpr::Function { name, args }
            }
            Rule::expr => run_pratt(pair.into_inner()),
            Rule::string => {
                let s = pair.as_str();
                // Strip the first and last characters (the quotes)
                ValueExpr::Str(s[1..s.len() - 1].to_string())
            }
            // A parenthesised bool_expr in a value-expression context.
            // base_primary tries bool_expr before expr, so comparisons/chains
            // inside parens land here rather than as mis-parsed arithmetic.
            Rule::bool_expr => ValueExpr::Group(Box::new(parse_bool_expr(pair))),
            _ => unreachable!("{:?}", pair.as_rule()),
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

/// Parse a decimal number, stripping comma thousand-separators first.
///
/// The Ledger format allows numbers like `1,234.56`. Rust's `Decimal::parse`
/// does not accept commas, so we remove them before parsing. Falls back to
/// zero if parsing still fails (which should not happen for well-formed grammar
/// output, but avoids a panic in error paths).
fn clean_parse_decimal(s: &str) -> Decimal {
    let cleaned = s.replace(',', "");
    cleaned.parse().unwrap_or(Decimal::ZERO)
}

fn parse_state(s: &str) -> TransactionState {
    match s {
        "*" => TransactionState::Cleared,
        "!" => TransactionState::Pending,
        _ => TransactionState::Uncleared,
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::dec;

    use super::*;

    #[test]
    fn test_simple_transaction() {
        let input =
            "2023-01-01 * (123) Grocery Store\n  Expenses:Food  $50.00\n  Assets:Checking\n";
        let journal = parse_ledger(input).unwrap();

        assert_eq!(journal.entries.len(), 1);
        if let Entry::Transaction(tx) = &journal.entries[0] {
            assert_eq!(tx.description, "Grocery Store");
            assert_eq!(tx.code, Some("123".to_string()));
            assert!(matches!(tx.state, TransactionState::Cleared));
            assert_eq!(tx.postings.len(), 2);
            assert_eq!(tx.postings[0].account, "Expenses:Food");
            assert_eq!(
                tx.postings[0].amount,
                Some(AmountDetails::Amount {
                    value: ValueExpr::Amount {
                        value: dec!(50.00),
                        commodity: Some("$".into()),
                    },
                    lot_annotation: None,
                    lot_pricing: None,
                    balance_assertion: None,
                })
            );
            assert_eq!(tx.postings[1].account, "Assets:Checking");
            assert!(tx.postings[1].amount.is_none());
        } else {
            panic!("Expected a transaction");
        }
    }

    #[test]
    fn test_lot_and_assertion() {
        let input = "2023-01-01 * Stock Purchase\n  Assets:Brokerage  10 AAPL @ $150.00 = $1500.00\n  Assets:Checking\n";
        let journal = parse_ledger(input).expect("Should parse successfully");

        if let Entry::Transaction(ref tx) = journal.entries[0] {
            let p = &tx.postings[0];
            let details = p.amount.as_ref().expect("Should have amount details");

            assert_eq!(
                details,
                &AmountDetails::Amount {
                    value: ValueExpr::Amount {
                        value: dec!(10),
                        commodity: Some("AAPL".into()),
                    },
                    lot_annotation: None,
                    lot_pricing: Some(LotPricing::Unit(ValueExpr::Amount {
                        commodity: Some("$".into()),
                        value: dec!(150.00)
                    })),
                    balance_assertion: Some(ValueExpr::Amount {
                        value: dec!(1500.00),
                        commodity: Some("$".to_string()),
                    })
                }
            );
        }
    }

    #[test]
    fn test_notes_and_comments() {
        let input = "
; Top level comment
2023-01-01 Transaction with notes
  ; Header note
  Expenses:Rent  $1000
  ; Posting note
  Assets:Checking
";
        let journal = parse_ledger(input).unwrap();

        // Entry 0 is an empty line (optional depending on grammar strictness)
        // Entry 1 is the comment
        // Entry 2 is the transaction
        let tx = journal
            .entries
            .iter()
            .find_map(|e| {
                if let Entry::Transaction(t) = e {
                    Some(t)
                } else {
                    None
                }
            })
            .expect("Transaction not found");

        assert_eq!(tx.notes[0], "Header note");
        assert_eq!(tx.postings[0].notes[0], "Posting note");
    }

    #[test]
    fn test_invalid_date() {
        let input = "23-01-01 * Missing Year Century\n  Expenses:Food  $10.00\n  Assets:Cash\n";
        let result = parse_ledger(input);
        assert!(result.is_err(), "Should fail due to strict date format");
    }

    #[test]
    fn test_complex_math_and_commas() {
        // 1. Thousand separators
        // 2. Nested parentheses
        // 3. Precedence: (1,000 + 200) * 2 = 2,400
        let input = "2023-01-01 * Math Test
    Expenses:Food  (1,000.00 + 200) * 2 USD
    Assets:Cash    $-1,234.56
";
        let journal = parse_ledger(input).unwrap();
        let tx = match &journal.entries[0] {
            Entry::Transaction(t) => t,
            _ => panic!("Expected transaction"),
        };

        // Verify first posting (Complex Math)
        let p1 = &tx.postings[0];
        if let Some(details) = &p1.amount {
            // We expect a Binary expression at the top level
            assert!(matches!(
                details,
                AmountDetails::Amount {
                    value: ValueExpr::Binary { .. },
                    ..
                }
            ));
        }

        // Verify second posting (Negative with Commas)
        let p2 = &tx.postings[1];
        if let Some(details) = &p2.amount {
            // This is interesting: depending on whether $-1234 or -$1234 is used,
            // it might be a Unary(Amount) or an Amount with a negative number.
            // Current grammar for `amount` + `prefix_op` makes this a Unary(Amount).
            assert!(matches!(
                details,
                AmountDetails::Amount {
                    value: ValueExpr::Binary { .. },
                    ..
                }
            ));
        }
    }

    #[test]
    fn test_function_calls() {
        let input = "2023-01-01 * Func Test
    Expenses:Travel  market(100, 2023-01-01)
    Assets:Checking
";
        let journal = parse_ledger(input).unwrap();
        let tx = match &journal.entries[0] {
            Entry::Transaction(t) => t,
            _ => panic!("Expected transaction"),
        };

        let p1 = &tx.postings[0];
        match &p1.amount {
            Some(AmountDetails::Amount {
                value: ValueExpr::Function { name, args },
                ..
            }) => {
                assert_eq!(name, "market");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected a function call, got {:?}", p1.amount),
        }
    }

    #[test]
    fn test_just_math() {
        let input = "(100 + 20) * 5";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        // next() returns the Pair<Rule::value_expr>
        let expr = parse_expr(pairs.next().unwrap());
        println!("{:?}", expr);
    }

    #[test]
    fn test_math_with_commodity() {
        let input = "(100 + 20) USD";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        let expr = parse_expr(pairs.next().unwrap());
        assert!(matches!(expr, ValueExpr::Typed { .. }));
    }

    #[test]
    fn test_comma_number() {
        let input = "1,234.56";
        let pairs = LedgerParser::parse(Rule::number, input).unwrap();
        assert_eq!(clean_parse_decimal(pairs.as_str()), dec!(1234.56));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Lot annotation grammar tests
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn test_lot_annotation_cost_only() {
        let input = "2024-03-01 Buy\n    Assets:Brokerage   10 AAPL {$150}\n    Assets:Cash\n";
        let journal = parse_ledger(input).expect("parse");
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
        assert!(matches!(
            ann.cost.as_ref().unwrap(),
            ValueExpr::Amount {
                commodity: Some(c),
                ..
            } if c == "$"
        ));
    }

    #[test]
    fn test_lot_annotation_date_only() {
        let input =
            "2024-03-01 Buy\n    Assets:Brokerage   10 AAPL [2024-01-15]\n    Assets:Cash\n";
        let journal = parse_ledger(input).expect("parse");
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
            "2024-03-01 Buy\n    Assets:Brokerage   10 AAPL ((BUY-2024-01))\n    Assets:Cash\n";
        let journal = parse_ledger(input).expect("parse");
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
        let input = "2024-03-01 Buy\n    Assets:Brokerage   10 AAPL {$150} [2024-03-01] ((BUY-2024-01))\n    Assets:Cash\n";
        let journal = parse_ledger(input).expect("parse");
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
    fn test_lot_annotation_double_brace_rejected() {
        let input = "2024-03-01 Buy\n    Assets:Brokerage   10 AAPL {{$1500}}\n    Assets:Cash\n";
        let result = parse_ledger(input);
        assert!(
            result.is_err(),
            "double-brace `{{total}}` should be rejected at parse time"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("double-brace"),
            "error message should mention double-brace, got: {msg}"
        );
    }
}

#[cfg(test)]
mod directed_tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn test_number_commodity_variants() {
        let cases = vec![
            ("$1000", dec!(1000), Some("$")),
            ("1000 USD", dec!(1000), Some("USD")),
        ];

        for (input, expected_val, expected_comm) in cases {
            let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
            let expr = parse_expr(pairs.next().unwrap());
            if let ValueExpr::Amount { value, commodity } = expr {
                assert_eq!(value, expected_val);
                assert_eq!(commodity, expected_comm.map(|s| s.to_string()));
            } else {
                panic!("Expected Amount, got {:?}", expr);
            }
        }

        // Handle the negative case separately as it's a Unary tree
        let input = "-1,234.56 BTC";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        let expr = parse_expr(pairs.next().unwrap());
        assert!(matches!(expr, ValueExpr::Unary { op: Op::Sub, .. }));
    }

    #[test]
    fn test_posting_variants() {
        let input = "2023-01-01 Transaction
    Expenses:NoAmount
    Expenses:SimpleAmount  $100
    Expenses:Expression    (100 + 100) USD";

        // Parse specifically as a transaction
        let mut pairs = LedgerParser::parse(Rule::transaction, input).unwrap();
        let tx_pair = pairs.next().unwrap();
        let tx = parse_transaction(tx_pair).unwrap();

        assert_eq!(tx.postings.len(), 3);
    }

    #[test]
    fn test_balance_assignment() {
        let input = "2024-12-17 Opening Balance
        Assets:Bank:Checking    =$21,966.08
        Equity:Opening Balances";

        let journal = parse_ledger(input).expect("Should parse balance assignment");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!()
        };

        let p = &tx.postings[0];
        assert_eq!(p.account, "Assets:Bank:Checking");

        let details = p.amount.as_ref().expect("Should have amount details");
        assert_eq!(
            *details,
            AmountDetails::BalanceAssignment(ValueExpr::Amount {
                value: dec!(21966.08),
                commodity: Some("$".into())
            })
        );
    }

    #[test]
    fn test_commodity_directive_block() {
        let input = "commodity BTC
    ; The primary crypto
    alias Bitcoin
    format 1,000.00000000 BTC
    nomarket
    default
";
        let journal = parse_ledger(input).expect("Should parse commodity directive");

        if let Entry::Directive(Directive::Commodity { name, notes, items }) = &journal.entries[0] {
            assert_eq!(name, "BTC");
            assert_eq!(notes[0], "The primary crypto");
            assert_eq!(items.len(), 4);

            assert!(matches!(items[0], CommodityItem::Alias(_)));
            assert!(matches!(items[1], CommodityItem::Format(_)));
            assert!(matches!(items[2], CommodityItem::NoMarket));
            assert!(matches!(items[3], CommodityItem::Default));
        } else {
            panic!("Expected a Commodity Directive");
        }
    }

    #[test]
    fn test_commodity_note_parses_to_note_item() {
        // Regression test for issue #91: `note` sub-key was falling through to
        // `CommodityItem::Unknown`, causing a spurious "unrecognised" warning.
        let input = "commodity $\n    note American Dollars\n    format $1,000.00\n";
        let journal = parse_ledger(input).expect("Should parse commodity with note");

        let Entry::Directive(Directive::Commodity { name, items, .. }) = &journal.entries[0] else {
            panic!("Expected Commodity directive");
        };
        assert_eq!(name, "$");
        // Verify that `note` produces CommodityItem::Note, not Unknown.
        let note_item = items.iter().find(|i| matches!(i, CommodityItem::Note(_)));
        assert!(
            note_item.is_some(),
            "expected CommodityItem::Note, got: {items:?}"
        );
        let CommodityItem::Note(text) = note_item.unwrap() else {
            unreachable!()
        };
        assert_eq!(text, "American Dollars");
        // Also confirm no Unknown item snuck through.
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, CommodityItem::Unknown(..))),
            "unexpected Unknown item in: {items:?}"
        );
    }

    #[test]
    fn test_string_in_function() {
        let input = "account(\"Assets:Bank:Checking\")";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        let expr = parse_expr(pairs.next().unwrap());

        if let ValueExpr::Function { name, args } = expr {
            assert_eq!(name, "account");
            match &args[0] {
                ValueExpr::Str(s) => assert_eq!(s, "Assets:Bank:Checking"),
                _ => panic!("Expected string argument"),
            }
        } else {
            panic!("Expected function call");
        }
    }

    #[test]
    fn test_field_access() {
        let input = "account(\"Assets:Bank\").total.quantity";
        let mut pairs = LedgerParser::parse(Rule::value_expr, input).unwrap();
        let expr = parse_expr(pairs.next().unwrap());

        if let ValueExpr::Access { expr: inner, field } = expr {
            assert_eq!(field, "quantity");
            if let ValueExpr::Access {
                field: inner_field, ..
            } = *inner
            {
                assert_eq!(inner_field, "total");
            } else {
                panic!("Expected nested access");
            }
        } else {
            panic!("Expected field access, got {:?}", expr);
        }
    }

    #[test]
    fn test_historical_price_with_time() {
        let input = "P 2024-06-15 14:30:00 AAPL $182.50\n";
        let journal = parse_ledger(input).unwrap();
        assert_eq!(journal.entries.len(), 1);
        let Entry::HistoricalPrice(ref hp) = journal.entries[0] else {
            panic!("Expected HistoricalPrice");
        };
        assert_eq!(hp.date.year, Some(2024));
        assert_eq!(hp.date.month, 6);
        assert_eq!(hp.date.date, 15);
        assert_eq!(hp.time.as_deref(), Some("14:30:00"));
        assert_eq!(hp.commodity, "AAPL");
        assert!(matches!(
            hp.price,
            ValueExpr::Amount {
                commodity: Some(ref c),
                ..
            } if c == "$"
        ));
    }

    #[test]
    fn test_historical_price_without_time() {
        let input = "P 2024-01-01 BTC $42000\n";
        let journal = parse_ledger(input).unwrap();
        let Entry::HistoricalPrice(ref hp) = journal.entries[0] else {
            panic!("Expected HistoricalPrice");
        };
        assert_eq!(hp.date.month, 1);
        assert_eq!(hp.date.date, 1);
        assert!(hp.time.is_none());
        assert_eq!(hp.commodity, "BTC");
    }

    #[test]
    fn test_date_parsing_day_differs_from_month() {
        // Regression test for a parse_date bug where day was not read from the
        // third token — both month and date were set to the same monthdate pair.
        let input = "P 2024-03-17 AAPL $100\n";
        let journal = parse_ledger(input).unwrap();
        let Entry::HistoricalPrice(ref hp) = journal.entries[0] else {
            panic!()
        };
        assert_eq!(hp.date.month, 3);
        assert_eq!(hp.date.date, 17);
    }

    #[test]
    fn test_assertion_directive_weak() {
        let input = "2024-01-15 = Assets:Checking  $1000.00\n";
        let journal = parse_ledger(input).unwrap();
        assert_eq!(journal.entries.len(), 1);
        let Entry::Assertion(ref a) = journal.entries[0] else {
            panic!("expected Assertion, got {:?}", journal.entries[0]);
        };
        assert_eq!(a.date.year, Some(2024));
        assert_eq!(a.date.month, 1);
        assert_eq!(a.date.date, 15);
        assert_eq!(a.account, "Assets:Checking");
        assert!(!a.strict, "= should be non-strict");
        assert!(matches!(
            a.amount,
            ValueExpr::Amount { commodity: Some(ref c), .. } if c == "$"
        ));
    }

    #[test]
    fn test_assertion_directive_strict() {
        let input = "2024-06-30 == Liabilities:CreditCard  $-500.00\n";
        let journal = parse_ledger(input).unwrap();
        assert_eq!(journal.entries.len(), 1);
        let Entry::Assertion(ref a) = journal.entries[0] else {
            panic!("expected Assertion");
        };
        assert_eq!(a.account, "Liabilities:CreditCard");
        assert!(a.strict, "== should be strict");
    }

    #[test]
    fn test_bool_expr_and_chain_parses() {
        // Regression test for issue #78: `and` was being consumed as a commodity
        // by value_expr, causing the bool chain to be silently dropped.
        // After the grammar fix, the chain must survive round-trip through the parser.
        let input = "\
account Assets:Savings
    assert amount > 0 and amount < 0
";
        let journal = parse_ledger(input).unwrap();
        assert_eq!(journal.entries.len(), 1);
        let Entry::Directive(Directive::Account { items, .. }) = &journal.entries[0] else {
            panic!("expected Account directive");
        };
        let assert_item = items
            .iter()
            .find(|item| matches!(item, AccountItem::Assert(_)))
            .expect("assert item not found");
        let AccountItem::Assert(bool_expr) = assert_item else {
            unreachable!()
        };
        // The chain must be present — if it's None, the grammar still drops `and`.
        assert!(
            bool_expr.chain.is_some(),
            "bool_expr.chain should be Some(And, ...), got None — grammar fix may not be applied"
        );
        assert!(
            matches!(bool_expr.chain.as_ref().unwrap().0, BoolOp::And),
            "expected BoolOp::And in chain"
        );
    }

    #[test]
    fn test_paren_bool_expr_simple() {
        // `(amt > 0)` must parse as a Group wrapping a bool_expr comparison.
        let input = "account Assets:Savings\n    assert (amount > 0)\n";
        let journal = parse_ledger(input).expect("should parse");
        let Entry::Directive(Directive::Account { items, .. }) = &journal.entries[0] else {
            panic!("expected Account directive");
        };
        let AccountItem::Assert(expr) = items
            .iter()
            .find(|i| matches!(i, AccountItem::Assert(_)))
            .unwrap()
        else {
            unreachable!()
        };
        // The lhs of the outer bool_expr is a Group — the paren bool.
        assert!(
            matches!(expr.lhs, ValueExpr::Group(_)),
            "expected ValueExpr::Group, got {:?}",
            expr.lhs
        );
    }

    #[test]
    fn test_paren_bool_expr_or_chain_inside_parens() {
        // `(amt > 0 or amt < -10)` must parse without error.
        let input = "account Assets:Savings\n    assert (amount > 0 or amount < -10)\n";
        let journal = parse_ledger(input).expect("should parse");
        let Entry::Directive(Directive::Account { items, .. }) = &journal.entries[0] else {
            panic!("expected Account directive");
        };
        assert!(items.iter().any(|i| matches!(i, AccountItem::Assert(_))));
    }

    #[test]
    fn test_paren_bool_nested_parens() {
        // Nested parenthesised bool: `(a > 0 and (tag("X") =~ /a/ or tag("Y") =~ /b/))`
        let input =
            "account Test\n    assert (amount > 0 and (tag(\"X\") =~ /a/ or tag(\"Y\") =~ /b/))\n";
        parse_ledger(input).expect("nested parens should parse");
    }

    #[test]
    fn test_plain_arithmetic_paren_still_works() {
        // Arithmetic `(100 + 200) USD` must still parse as a Typed node,
        // not as a Group, because bool_expr backtracks for plain arithmetic.
        let input = "2024-01-01 Test\n    Expenses:Food  (100 + 200) USD\n    Assets:Cash\n";
        let journal = parse_ledger(input).expect("should parse");
        let Entry::Transaction(tx) = &journal.entries[0] else {
            panic!("expected transaction");
        };
        // The amount should be a Typed (or Binary) expr, not a Group.
        let details = tx.postings[0].amount.as_ref().unwrap();
        assert!(
            matches!(
                details,
                AmountDetails::Amount {
                    value: ValueExpr::Typed { .. } | ValueExpr::Binary { .. },
                    ..
                }
            ),
            "expected Typed or Binary, got {details:?}"
        );
    }

    #[test]
    fn test_define_body_paren_bool_expr() {
        // `define inRange(x) = (x > 0 and x < 100)` must parse and produce a
        // define body that carries the boolean logic wrapped in a Group.
        //
        // The outer bool_expr has `lhs = Group(...)`, `cmp = None`, `chain = None`,
        // so `parse_define_directive` unwraps it as a trivial bool_expr and
        // yields `DefineBody::Value(ValueExpr::Group(...))`. The Group variant
        // holds the full inner BoolExpr, which the evaluator handles correctly.
        let input = "define inRange(x) = (x > 0 and x < 100)\n";
        let journal = parse_ledger(input).expect("should parse define with paren bool body");
        let Entry::Directive(Directive::Define { name, params, body }) = &journal.entries[0] else {
            panic!("expected Define directive");
        };
        assert_eq!(name, "inRange");
        assert_eq!(params, &["x"]);
        // The body may be Value(Group(...)) or Bool(...) — either carries the
        // boolean logic correctly. Just verify it parsed without error and that
        // a Group is present somewhere in the body.
        match body {
            DefineBody::Value(ValueExpr::Group(_)) => {}
            DefineBody::Bool(_) => {}
            other => panic!("unexpected define body: {other:?}"),
        }
    }

    #[test]
    fn d_directive_rejects_bare_number() {
        // `D 1000.00` carries no commodity — the parser must reject it with a
        // message that mentions "commodity" so the user knows what is missing.
        let err = parse_ledger("D 1000.00\n").unwrap_err();
        assert!(
            err.to_string().contains("commodity"),
            "error should mention 'commodity', got: {err}"
        );
    }

    #[test]
    fn test_issue_89_failing_input() {
        // Regression test for issue #89: the exact failing input from the bug report.
        let input = "define assetChecker(amt) = (amt > -100.00 or (tag(\"TaxImplication\") !~ /^\\s*$/ and tag(\"Entity\") !~ /^\\s*$/))\n";
        parse_ledger(input).expect("issue #89 input should parse");
    }
}
