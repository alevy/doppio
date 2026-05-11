//! Beancount source writer.
//!
//! [`write`] serialises a [`resolution::HIR`] to Beancount source text.
//! Beancount syntax differs from ledger-cli in several ways:
//!
//! - Transaction descriptions are always double-quoted strings.
//! - Amounts are in `number COMMODITY` form (space-separated, commodity after
//!   amount).
//! - `*` marks cleared transactions; `!` marks flagged (pending) transactions;
//!   `txn` is the uncleared keyword (emitted as `*` for simplicity here).
//! - Balance assertions use `balance YYYY-MM-DD account amount` syntax.
//! - Price directives use `YYYY-MM-DD price COMMODITY amount` syntax.
//! - Pad directives use `YYYY-MM-DD pad target_account source_account`.
//! - Tags use `#tag` prefix in the transaction header.
//! - Metadata is written as `key: "value"` on indented lines within a block.
//!
//! ## What is preserved
//!
//! - Transactions (date, state, description, tags, metadata, postings).
//! - Historical price directives.
//! - Balance assertions.
//! - Pad directives (if present in the HIR from a Beancount source).
//!
//! ## What is lost vs. the AST
//!
//! The resolution stage discards `open`/`close`/`commodity`/`option`/`event`
//! and other Beancount-specific directives (they become comments in the AST and
//! are then dropped). A future AST-level writer could recover them.
//!
//! ## Cross-frontend transcoding
//!
//! Constructs that have no Beancount equivalent are emitted as `; [<format>]`
//! comment lines. For example, a ledger `apply tag` scope has no Beancount
//! equivalent and would appear as a comment if it were preserved in the HIR
//! (currently it is not).

use std::io;

use crate::resolution::{Entry, HIR};

/// Write `hir` to `writer` in Beancount source text format.
///
/// Transactions use double-quoted descriptions and `#tag` syntax. Amounts
/// use the `number COMMODITY` form. Balance assertions and price directives
/// follow Beancount's own syntax.
///
/// # Errors
///
/// Propagates any [`io::Error`] from `writer`.
pub fn write(hir: &HIR, writer: &mut dyn io::Write) -> io::Result<()> {
    let mut first = true;

    // Historical prices: `YYYY-MM-DD price COMMODITY price_amount`
    for price in &hir.prices {
        if !first {
            writeln!(writer)?;
        }
        first = false;
        let price_str = format_beancount_amount(&price.price);
        writeln!(
            writer,
            "{} price {} {}",
            price.date, price.commodity, price_str
        )?;
    }

    // Entries in source order.
    for entry in &hir.entries {
        match &entry.data {
            Entry::Transaction(txn) => {
                if !first {
                    writeln!(writer)?;
                }
                first = false;
                write_transaction(txn, writer)?;
            }
            Entry::Assertion(a) => {
                if !first {
                    writeln!(writer)?;
                }
                first = false;
                // Beancount: `YYYY-MM-DD balance account amount`
                let amount_str = format_beancount_amount(&a.amount);
                writeln!(writer, "{} balance {} {}", a.date, a.account, amount_str)?;
            }
            Entry::Pad(p) => {
                if !first {
                    writeln!(writer)?;
                }
                first = false;
                // Beancount: `YYYY-MM-DD pad target_account source_account`
                writeln!(
                    writer,
                    "{} pad {} {}",
                    p.date, p.target_account, p.source_account
                )?;
            }
        }
    }

    Ok(())
}

/// Format a [`crate::ast::ValueExpr`] in Beancount's preferred style.
///
/// Beancount always puts the number first and the commodity second
/// (`100.00 USD` not `$100.00`). For simple `Amount` expressions this
/// produces the canonical form. For complex expressions (arithmetic,
/// etc.) we fall back to the `Display` impl which uses ledger-style
/// formatting — those expressions only appear in source-constructed
/// journals, not in round-tripped Beancount files.
fn format_beancount_amount(expr: &crate::ast::ValueExpr) -> String {
    use crate::ast::ValueExpr;
    match expr {
        ValueExpr::Amount { value, commodity } => {
            if let Some(c) = commodity {
                format!("{value} {c}")
            } else {
                format!("{value}")
            }
        }
        other => format!("{other}"),
    }
}

/// Escape a string for use in a Beancount double-quoted context.
///
/// Replaces `\` with `\\` and `"` with `\"`.
fn beancount_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_transaction(
    txn: &crate::resolution::Transaction,
    writer: &mut dyn io::Write,
) -> io::Result<()> {
    // Flag: Beancount uses `*` (cleared), `!` (flagged/pending), or `txn` (uncleared).
    let flag = match txn.state {
        crate::ast::TransactionState::Cleared => "*",
        crate::ast::TransactionState::Pending => "!",
        crate::ast::TransactionState::Uncleared => "txn",
    };

    // Build the tag portion from the tags vec.
    // Beancount tags go in the header: `#vacation ^invoice-0123`
    // Tags that start with `^` are already links; others get `#` prefix.
    let tags_str: String = txn
        .tags
        .iter()
        .map(|t| {
            if t.starts_with('^') {
                format!(" {t}")
            } else {
                format!(" #{t}")
            }
        })
        .collect();

    // Description is always quoted in Beancount.
    let escaped_desc = beancount_escape(&txn.description);
    writeln!(
        writer,
        "{} {} \"{}\"{}",
        txn.date, flag, escaped_desc, tags_str
    )?;

    // Secondary date has no Beancount equivalent — emit as a comment.
    if let Some(sec) = txn.secondary_date {
        writeln!(writer, "  ; [ledger] secondary-date {sec}")?;
    }
    // Code has no Beancount equivalent — emit as a comment.
    if let Some(ref code) = txn.code {
        writeln!(writer, "  ; [ledger] code ({code})")?;
    }

    // Metadata as `key: "value"` lines.
    for (key, value) in &txn.metadata {
        let escaped = beancount_escape(value);
        writeln!(writer, "  {key}: \"{escaped}\"")?;
    }

    // Plain comments.
    for comment in &txn.comments {
        writeln!(writer, "  ; {comment}")?;
    }

    // Postings.
    for posting in &txn.postings {
        write_posting(posting, writer)?;
    }

    Ok(())
}

fn write_posting(
    posting: &crate::resolution::Posting,
    writer: &mut dyn io::Write,
) -> io::Result<()> {
    // Virtual postings have no Beancount equivalent; emit with a marker comment.
    match posting.kind {
        crate::ast::PostingKind::Real => {}
        crate::ast::PostingKind::VirtualUnbalanced => {
            writeln!(writer, "  ; [ledger] virtual-unbalanced posting follows")?;
        }
        crate::ast::PostingKind::VirtualBalanced => {
            writeln!(writer, "  ; [ledger] virtual-balanced posting follows")?;
        }
    }

    // Per-posting state flag in Beancount (`!` flag on the posting line).
    let flag = match posting.state {
        crate::ast::TransactionState::Uncleared => None,
        crate::ast::TransactionState::Pending => Some("!"),
        crate::ast::TransactionState::Cleared => Some("*"),
    };
    if let Some(f) = flag {
        write!(writer, "  {f} {}", posting.account)?;
    } else {
        write!(writer, "  {}", posting.account)?;
    }

    if let Some(ref amount) = posting.amount {
        let amount_str = format_beancount_posting_amount(amount);
        write!(writer, "  {amount_str}")?;
    }
    writeln!(writer)?;

    // Posting metadata as `key: "value"` lines.
    for (key, value) in &posting.metadata {
        let escaped = beancount_escape(value);
        writeln!(writer, "    {key}: \"{escaped}\"")?;
    }

    // Per-posting comments.
    for comment in &posting.comments {
        writeln!(writer, "    ; {comment}")?;
    }

    Ok(())
}

/// Format an [`crate::ast::AmountDetails`] in Beancount's preferred style.
///
/// The core difference from ledger: amounts are `number COMMODITY` (not
/// `$number` prefix-symbol style). Balance assertions become inline
/// `= amount` suffixes (Beancount doesn't have them on postings, so we
/// drop them with a comment marker). The `==*` all-commodities form is
/// a pure hledger extension — emit it as a comment.
fn format_beancount_posting_amount(amount: &crate::ast::AmountDetails) -> String {
    use crate::ast::{AmountDetails, LotPricing};

    // `AmountDetails` is `#[non_exhaustive]`; the wildcard arm is required for
    // forward compatibility and kept even though it is currently unreachable.
    #[allow(unreachable_patterns)]
    match amount {
        AmountDetails::Amount {
            value,
            lot_annotation,
            lot_pricing,
            balance_assertion,
        } => {
            let mut s = format_beancount_amount(value);

            // Lot annotation: `{cost [, date] [, "note"]}` — Beancount style.
            if let Some(ann) = lot_annotation {
                let mut parts = Vec::new();
                if let Some(cost) = &ann.cost {
                    parts.push(format_beancount_amount(cost));
                }
                if let Some(date) = ann.date {
                    parts.push(format!("{date}"));
                }
                if let Some(note) = &ann.note {
                    let escaped = beancount_escape(note);
                    parts.push(format!("\"{escaped}\""));
                }
                if !parts.is_empty() {
                    s.push_str(" {");
                    s.push_str(&parts.join(", "));
                    s.push('}');
                }
            }

            // Lot pricing: `@ unit_price` or `@@ total_price`.
            if let Some(pricing) = lot_pricing {
                match pricing {
                    LotPricing::Unit(p) => {
                        s.push_str(&format!(" @ {}", format_beancount_amount(p)));
                    }
                    LotPricing::Total(p) => {
                        s.push_str(&format!(" @@ {}", format_beancount_amount(p)));
                    }
                }
            }

            // Balance assertion on a posting has no Beancount parallel;
            // Beancount uses top-level `balance` directives. Emit as a comment
            // suffix so the intent is visible.
            if let Some(ba) = balance_assertion {
                s.push_str(&format!(" ; [hledger] = {}", format_beancount_amount(ba)));
            }

            s
        }
        AmountDetails::BalanceAssignment(target) => {
            // hledger-only form — no Beancount parallel.
            format!("; [hledger] = {}", format_beancount_amount(target))
        }
        AmountDetails::BalanceAssignmentAllCommodities(target) => {
            // hledger-only `==*` form — no Beancount parallel.
            format!("; [hledger] ==* {}", format_beancount_amount(target))
        }
        _ => String::new(),
    }
}
