//! Grammar implementations for each supported file format.
//!
//! Each sub-module implements [`crate::frontend::Frontend`] for one file
//! format and houses the corresponding pest grammar.
//!
//! Currently available:
//! - [`ledger`] — the ledger-cli plain-text accounting format.
//!
//! Planned (see issue #103):
//! - `hledger` — the hledger dialect.

pub mod ledger;
