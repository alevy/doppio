//! Grammar implementations for each supported file format.
//!
//! Each sub-module implements [`crate::frontend::Frontend`] for one file
//! format and houses the corresponding pest grammar.
//!
//! Currently available:
//! - [`ledger`] -- the ledger-cli plain-text accounting format (`.ledger`).
//! - [`hledger`] -- the hledger dialect (`.hledger`, `.journal`).
//! - [`beancount`] -- the Beancount format (`.beancount`).
//!   **Experimental**: the grammar (#145) and Frontend trait impl
//!   (#146) ship; the `pad` directive evaluator (#147) is still
//!   in-flight, so pads are preserved as markers but produce no
//!   balancing transaction during elaboration.

pub mod beancount;
pub mod hledger;
pub mod ledger;
