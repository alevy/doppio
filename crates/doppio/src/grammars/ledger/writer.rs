//! Ledger-cli source writer.
//!
//! [`write`] serialises a [`resolution::HIR`] to ledger-cli source text.
//! The output is idiomatic ledger-cli: standard `YYYY-MM-DD` dates, `;`
//! comment lines, `*` / `!` state markers, and commodity-after-amount
//! formatting.
//!
//! ## What is preserved
//!
//! Everything present in the [`resolution::HIR`] is preserved:
//!
//! - Transactions (date, state, code, description, comments, tags, metadata,
//!   postings with amounts and balance assertions).
//! - Historical price directives (`P date commodity price`).
//! - Balance assertions and `pad` directives (emitted as comments since
//!   ledger-cli has no direct `pad` syntax).
//!
//! ## What is lost vs. the AST
//!
//! The resolution stage discards `account` / `commodity` directives and inline
//! comments, so those are absent from the HIR and therefore absent from the
//! output. A future writer working at the AST level could recover them.

use std::io;

use crate::resolution::{Entry, HIR};

/// Write `hir` to `writer` in ledger-cli source text format.
///
/// Transactions are emitted in source order with blank-line separators.
/// Historical prices appear in `P YYYY-MM-DD commodity price` form.
/// Balance assertions appear as standalone `YYYY-MM-DD balance account amount`.
/// Pad directives have no direct ledger-cli equivalent and are emitted as
/// comments: `; pad YYYY-MM-DD target_account source_account`.
///
/// # Errors
///
/// Propagates any [`io::Error`] from `writer`.
pub fn write(hir: &HIR, writer: &mut dyn io::Write) -> io::Result<()> {
    let mut first = true;

    // Historical prices first (by convention; ledger-cli doesn't enforce order).
    for price in &hir.prices {
        if !first {
            writeln!(writer)?;
        }
        first = false;
        let price_str = format!("{}", price.price);
        writeln!(writer, "P {} {} {}", price.date, price.commodity, price_str)?;
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
                let strict = if a.strict { "==" } else { "=" };
                writeln!(writer, "{} {} {}  {}", a.date, strict, a.account, a.amount)?;
            }
            Entry::Pad(p) => {
                // `pad` has no ledger-cli native syntax; emit as a comment
                // with the `; [beancount]` prefix so it's traceable.
                if !first {
                    writeln!(writer)?;
                }
                first = false;
                writeln!(
                    writer,
                    "; [beancount] pad {} {} {}",
                    p.date, p.target_account, p.source_account
                )?;
            }
        }
    }

    Ok(())
}

fn write_transaction(
    txn: &crate::resolution::Transaction,
    writer: &mut dyn io::Write,
) -> io::Result<()> {
    // Header: date[=secondary_date] [state] [code] description
    write!(writer, "{}", txn.date)?;
    if let Some(sec) = txn.secondary_date {
        write!(writer, "={sec}")?;
    }
    match txn.state {
        crate::ast::TransactionState::Uncleared => {}
        crate::ast::TransactionState::Pending => write!(writer, " !")?,
        crate::ast::TransactionState::Cleared => write!(writer, " *")?,
    }
    if let Some(ref code) = txn.code {
        write!(writer, " ({code})")?;
    }
    writeln!(writer, " {}", txn.description)?;

    // Transaction-level comments, tags, and metadata.
    for comment in &txn.comments {
        writeln!(writer, "    ; {comment}")?;
    }
    for tag in &txn.tags {
        writeln!(writer, "    ; :{tag}:")?;
    }
    for (key, value) in &txn.metadata {
        writeln!(writer, "    ; {key}: {value}")?;
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
    write!(writer, "    ")?;
    match posting.state {
        crate::ast::TransactionState::Uncleared => {}
        crate::ast::TransactionState::Pending => write!(writer, "! ")?,
        crate::ast::TransactionState::Cleared => write!(writer, "* ")?,
    }
    match posting.kind {
        crate::ast::PostingKind::Real => write!(writer, "{}", posting.account)?,
        crate::ast::PostingKind::VirtualUnbalanced => write!(writer, "({})", posting.account)?,
        crate::ast::PostingKind::VirtualBalanced => write!(writer, "[{}]", posting.account)?,
    }
    if let Some(ref amount) = posting.amount {
        write!(writer, "  {amount}")?;
    }
    writeln!(writer)?;
    for comment in &posting.comments {
        writeln!(writer, "        ; {comment}")?;
    }
    for tag in &posting.tags {
        writeln!(writer, "        ; :{tag}:")?;
    }
    for (key, value) in &posting.metadata {
        writeln!(writer, "        ; {key}: {value}")?;
    }
    Ok(())
}
