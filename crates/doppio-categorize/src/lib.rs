//! Counter-account prediction for the doppio ledger compiler.
//!
//! Given a partial transaction (one side known -- typically a bank/credit-card
//! account from an import), suggest the most likely counter-account from
//! patterns in an existing journal.
//!
//! # Example
//!
//! ```ignore
//! use doppio_categorize::{Index, Query, Config, RichNormalizer};
//!
//! let index = Index::build(&journal, RichNormalizer);
//! let query = Query {
//!     date: today,
//!     payee: "STARBUCKS #1234 SEATTLE WA".into(),
//!     amount: rust_decimal::Decimal::new(-758, 2),
//!     known_account: "Liabilities:Visa".into(),
//! };
//! let suggestions = index.suggest(&query, &Config::default());
//! ```

mod index;
mod normalize;
mod query;

pub use index::Index;
pub use normalize::{DefaultNormalizer, Normalizer, RichNormalizer};
pub use query::{Config, Query, ScoringStrategy, Suggestion};
