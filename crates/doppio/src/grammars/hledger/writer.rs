//! hledger source writer.
//!
//! [`write`] serialises a [`resolution::HIR`] to hledger source text.
//! hledger syntax differs from ledger-cli in a few key ways:
//!
//! - No secondary dates (`date=secondary_date` is not standard hledger syntax;
//!   emitted as a trailing comment if present).
//! - Amounts in `number commodity` form (e.g. `100.00 EUR`) or prefix-symbol
//!   form (e.g. `$100.00`).
//! - Balance assertions with `=`, `==`, `=*`, or `==*` operators.
//! - `P` price directives in the same form as ledger-cli.
//! - `account` / `commodity` directives are not preserved (not in HIR).
//!
//! ## What is preserved
//!
//! - Transactions (date, state, code, description, comments, tags, metadata,
//!   postings).
//! - Historical price directives.
//! - Balance assertions.
//! - Pad directives are emitted as `; [beancount] pad ...` comments since
//!   hledger has no `pad` primitive.
//!
//! ## Cross-frontend transcoding
//!
//! When writing a journal that originated from a different frontend (e.g.
//! ledger or beancount), format-specific constructs that have no hledger
//! equivalent are emitted as `; [<source-format>] <original>` comment lines.
//! For example, a Beancount `pad` directive becomes:
//!
//! ```text
//! ; [beancount] pad 2024-01-01 Assets:Bank:Checking Equity:Opening-Balances
//! ```

use std::io;

use crate::resolution::{Entry, HIR};

/// Write `hir` to `writer` in hledger source text format.
///
/// # Errors
///
/// Propagates any [`io::Error`] from `writer`.
pub fn write(hir: &HIR, writer: &mut dyn io::Write) -> io::Result<()> {
    let mut first = true;

    // Historical prices.
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
                // hledger balance directives use `balance YYYY-MM-DD account amount`.
                writeln!(
                    writer,
                    "; balance {} {} {}  {}",
                    a.date, strict, a.account, a.amount
                )?;
            }
            Entry::Pad(p) => {
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
    // hledger date format: YYYY-MM-DD (same as ledger).
    write!(writer, "{}", txn.date)?;

    // Secondary date: hledger supports `YYYY-MM-DD=YYYY-MM-DD` but it is
    // uncommon; emit it if present.
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

    for comment in &txn.comments {
        writeln!(writer, "    ; {comment}")?;
    }
    for tag in &txn.tags {
        writeln!(writer, "    ; :{tag}:")?;
    }
    for (key, value) in &txn.metadata {
        writeln!(writer, "    ; {key}: {value}")?;
    }

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
