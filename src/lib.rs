//! ledger-rs — a compiler and query library for the Ledger plain-text
//! accounting format.
//!
//! # Pipeline
//!
//! Source text is processed through four stages:
//!
//! ```text
//! source text
//!   → [parser]      ast::Journal        (PEG grammar + Pratt expressions)
//!   → [resolution]  resolution::HIR     (dates, aliases, metadata)
//!   → [elaboration] elaboration::Journal (evaluation, balancing)
//!   → serialisation                     (postcard + XZ → .bki)
//! ```
//!
//! The top-level entry point is [`compile`], which runs all three in-memory
//! stages and returns the elaborated [`Journal`]. For CLI usage see the
//! `ledger` binary in `src/main.rs`.
//!
//! # Modules
//!
//! - [`ast`] — abstract syntax tree produced by the parser.
//! - [`parser`] — pest-based parser and `include` directive handling.
//! - [`resolution`] — alias resolution, date normalisation, metadata
//!   extraction.
//! - [`elaboration`] — expression evaluation, transaction balancing, and the
//!   final serialisable [`Journal`] type.

pub mod ast;
pub mod elaboration;
pub mod parser;
pub mod resolution;

pub use elaboration::Journal;

/// Load and concatenate all files matching a glob pattern.
///
/// This is the default file-opener used by the CLI when processing `include`
/// directives. Multiple files matched by a single glob (e.g.
/// `include accounts/*.ledger`) are concatenated in the order that
/// [`glob::glob`] returns them (lexicographic on most platforms).
///
/// Panics if the pattern is invalid or a matched path cannot be read.
pub fn file_opener(pattern: &str) -> String {
    use std::io::Read as _;

    let mut buf = String::new();
    for path in glob::glob(pattern).unwrap() {
        std::fs::File::open(path.unwrap())
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();
    }
    buf
}

/// Compile Ledger source text into a fully elaborated [`Journal`].
///
/// Runs the three in-memory pipeline stages in sequence:
///
/// 1. [`parser::Parser::parse`] — tokenise `input` into an [`ast::Journal`].
/// 2. [`resolution::HIR::try_from`] — resolve dates, aliases, and metadata.
/// 3. [`elaboration::Journal::try_from`] — evaluate amounts and balance
///    transactions.
///
/// The `parser` argument supplies the file-opener for `include` directives and
/// the base path for relative path resolution. For single-file inputs without
/// includes, use [`parser::parse_ledger`] instead.
///
/// # Errors
///
/// Returns a boxed error from the first failing stage (parse error, resolution
/// error, or elaboration error).
pub fn compile<F>(
    input: &String,
    mut parser: parser::Parser<F>,
) -> Result<elaboration::Journal, Box<dyn std::error::Error>>
where
    F: Fn(&str) -> String,
{
    let output = parser.parse(input)?;
    let hir: resolution::HIR = output.try_into()?;
    Ok(hir.try_into()?)
}
