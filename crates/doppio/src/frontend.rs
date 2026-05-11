//! The [`Frontend`] trait -- the extension point for pluggable file-format
//! support.
//!
//! Each frontend recognises one or more file extensions and is responsible
//! for parsing source text into a resolved [`crate::resolution::HIR`] and for
//! serialising an [`HIR`] back to that format's source text via
//! [`Frontend::write_journal`].
//!
//! The ledger-cli format is implemented in [`crate::grammars::ledger`]; the
//! hledger dialect is in [`crate::grammars::hledger`]; Beancount is in
//! [`crate::grammars::beancount`].

use std::io;
use std::path::Path;

use crate::resolution::{ElaborationConfig, HIR};

/// The signature of an `include`-directive file opener.
///
/// An opener accepts a file path (or glob pattern) and returns the
/// concatenated contents of all matching files, or an error.  The type
/// alias keeps trait signatures readable; use `|_| Ok(String::new())`
/// as a no-op.
pub type Opener = dyn Fn(&str) -> Result<String, Box<dyn std::error::Error>>;

/// A file-format frontend that produces a resolved [`HIR`] and can serialise
/// one back to source text.
///
/// Implement this trait to add support for a new input format. The CLI
/// uses [`crate::frontend_for_extension`] to select the appropriate
/// frontend at runtime based on the file extension of the source file
/// being loaded.
///
/// # Example
///
/// ```rust
/// use doppio::frontend::Frontend;
/// use doppio::LedgerFrontend;
///
/// let fe = LedgerFrontend;
/// assert!(fe.extensions().contains(&"ledger"));
/// ```
pub trait Frontend {
    /// File extensions this frontend recognises (lowercase, without the
    /// dot).
    ///
    /// The CLI calls this for each registered frontend and selects the
    /// first one whose extension list contains the source file's
    /// extension.
    fn extensions(&self) -> &'static [&'static str];

    /// The default elaboration semantics for files in this frontend's
    /// syntax — i.e. the [`ElaborationConfig`] that mirrors what the
    /// canonical tool's own elaborator would do.
    ///
    /// This is a *convenience pairing*, not a forced coupling. The
    /// elaborator takes any `ElaborationConfig`; a caller can parse a
    /// file in one frontend's syntax and elaborate it under a different
    /// tool's rules. The associated default is what `dop`-style command
    /// dispatchers use when the user hasn't asked for anything more
    /// specific.
    fn elaboration_defaults(&self) -> ElaborationConfig;

    /// Parse source text into an [`HIR`].
    ///
    /// # Parameters
    ///
    /// - `input` -- the complete source text of the file being parsed.
    /// - `base_path` -- the directory of the file currently being
    ///   parsed. Used to resolve relative paths in `include`
    ///   directives.
    /// - `opener` -- invoked for each `include` directive with the
    ///   resolved path (or glob pattern). Must return the concatenated
    ///   file contents or an error. Pass `|_| Ok(String::new())` to
    ///   silently ignore includes.
    ///
    /// # Errors
    ///
    /// Returns a boxed error if:
    /// - the source text is syntactically invalid,
    /// - resolution fails (e.g. a partial date with no fallback year),
    ///   or
    /// - an `include` directive's `opener` call fails.
    fn parse(
        &self,
        input: &str,
        base_path: &Path,
        opener: &Opener,
    ) -> Result<HIR, Box<dyn std::error::Error>>;

    /// Serialise a resolved [`HIR`] to this frontend's source text format.
    ///
    /// Each frontend writes in its own native syntax:
    ///
    /// - [`crate::LedgerFrontend`] emits ledger-cli source (same format as
    ///   the deprecated [`crate::write_ledger`]).
    /// - [`crate::HledgerFrontend`] emits hledger source.
    /// - [`crate::BeancountFrontend`] emits Beancount source.
    ///
    /// ## Round-trip fidelity
    ///
    /// Parsing a file with `Frontend::parse`, running resolution, then calling
    /// `write_journal` on the same frontend produces output that re-parses and
    /// re-resolves to a semantically equivalent [`HIR`] (modulo whitespace
    /// differences and the information lost at the resolution boundary — see
    /// below).
    ///
    /// ## Cross-frontend transcoding and lossy cases
    ///
    /// When the [`HIR`] was produced by a *different* frontend than the one
    /// performing the write, format-specific constructs that have no equivalent
    /// in the target format are emitted as `; [<source-format>] ...` comment
    /// lines rather than being silently dropped. For example, writing a
    /// Beancount-sourced journal through [`crate::LedgerFrontend`] emits any
    /// `pad` directives as:
    ///
    /// ```text
    /// ; [beancount] pad 2024-01-01 Assets:Bank:Checking Equity:Opening-Balances
    /// ```
    ///
    /// ## What the HIR level cannot preserve
    ///
    /// The resolution stage consumes `account`/`commodity`/`option` directives
    /// and inline comments; they are absent from the [`HIR`] and therefore
    /// absent from the writer's output. A future AST-level writer would recover
    /// them.
    ///
    /// # Errors
    ///
    /// Propagates any [`io::Error`] from `writer`.
    fn write_journal(&self, hir: &HIR, writer: &mut dyn io::Write) -> io::Result<()>;
}
