//! Grammar implementations for each supported file format.
//!
//! Each sub-module implements [`crate::frontend::Frontend`] for one file
//! format and houses the corresponding pest grammar.
//!
//! Currently available:
//! - [`ledger`] -- the ledger-cli plain-text accounting format (`.ledger`).
//! - [`hledger`] -- the hledger dialect (`.hledger`, `.journal`).
//! - [`beancount`] -- the Beancount format (`.beancount`).
//!   **Experimental**: ships the grammar (#145); the `Frontend` trait
//!   impl (#146) and the `pad` directive evaluator (#147) are
//!   in-flight.

pub(crate) mod beancount;
pub mod hledger;
pub mod ledger;
