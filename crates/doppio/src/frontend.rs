//! The [`Frontend`] trait — the extension point for pluggable file-format
//! support.
//!
//! Each frontend recognises one or more file extensions and is responsible for
//! converting source text into an [`crate::ast::Journal`].  The ledger-cli
//! format is implemented in [`crate::grammars::ledger`]; future frontends
//! (hledger, beancount) will live alongside it under `grammars/`.

use std::path::Path;

/// The signature of an `include`-directive file opener.
///
/// An opener accepts a file path (or glob pattern) and returns the
/// concatenated contents of all matching files, or an error.  The type alias
/// keeps trait signatures readable; use `|_| Ok(String::new())` as a no-op.
pub type Opener = dyn Fn(&str) -> Result<String, Box<dyn std::error::Error>>;

/// A file-format frontend that produces an [`crate::ast::Journal`].
///
/// Implement this trait to add support for a new input format. The CLI uses
/// [`crate::frontend_for_extension`] to select the appropriate frontend at
/// runtime based on the file extension of the source file being loaded.
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
    /// File extensions this frontend recognises (lowercase, without the dot).
    ///
    /// The CLI calls this for each registered frontend and selects the first
    /// one whose extension list contains the source file's extension.
    fn extensions(&self) -> &'static [&'static str];

    /// Parse source text into an [`crate::ast::Journal`].
    ///
    /// # Parameters
    ///
    /// - `input` — the complete source text of the file being parsed.
    /// - `base_path` — the directory of the file currently being parsed.
    ///   Used to resolve relative paths in `include` directives.
    /// - `opener` — invoked for each `include` directive with the resolved
    ///   path (or glob pattern). Must return the concatenated file contents or
    ///   an error. Pass `|_| Ok(String::new())` to silently ignore includes.
    ///
    /// # Errors
    ///
    /// Returns a boxed error if:
    /// - the source text is syntactically invalid, or
    /// - an `include` directive's `opener` call fails.
    fn parse(
        &self,
        input: &str,
        base_path: &Path,
        opener: &Opener,
    ) -> Result<crate::ast::Journal, Box<dyn std::error::Error>>;
}
