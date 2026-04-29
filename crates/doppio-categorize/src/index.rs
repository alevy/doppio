//! Index construction: build sample buckets from a journal.

use chrono::NaiveDate;
use doppio::elaboration::Journal;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};

use crate::normalize::Normalizer;

/// A historical observation: when payee X was charged via known_account A,
/// the counter-balance went to `counter_account` for `amount` on `date`.
///
/// Internal type. Public API uses [`crate::Suggestion`].
#[derive(Debug, Clone)]
pub(crate) struct Sample {
    pub counter_account: String,
    pub amount: Decimal,
    pub date: NaiveDate,
    /// Tokens of the normalized payee (used by the token-IDF scorer).
    /// The full normalized string isn't stored on each sample — exact-match
    /// lookups go through [`KnownAccountBucket::by_payee`] which keys by
    /// the full normalized string.
    pub payee_tokens: Vec<String>,
}

/// Per-known-account storage. Holds a flat sample list (for token-IDF
/// scanning) plus a `(normalized_payee → sample-indices)` map (for the
/// exact-match fast path).
#[derive(Debug, Default)]
pub(crate) struct KnownAccountBucket {
    pub samples: Vec<Sample>,
    pub by_payee: HashMap<String, Vec<usize>>,
}

/// A built index over historical transactions, ready to answer suggest
/// queries.
///
/// Build once per journal; query many times.
pub struct Index {
    pub(crate) by_known: HashMap<String, KnownAccountBucket>,
    /// `token_df[t]` = number of distinct normalized payees that contain `t`.
    pub(crate) token_df: HashMap<String, u32>,
    /// Total distinct normalized payees observed.
    pub(crate) total_payees: u32,
    pub(crate) normalizer: Box<dyn Normalizer>,
}

impl Index {
    /// Build an index from a journal.
    ///
    /// For each transaction with N ≥ 2 postings, every ordered pair of
    /// postings (i, j) where i ≠ j produces a sample associating posting i's
    /// account (the "known" side) with a counter of posting j's account,
    /// posting i's amount, and the transaction's date. Postings with zero
    /// or empty amounts are skipped, as are postings whose payee normalizes
    /// to the empty string.
    ///
    /// Also computes per-token document frequency over the set of distinct
    /// normalized payees in the journal — needed by the token-IDF scoring
    /// strategy.
    pub fn build<N: Normalizer + 'static>(journal: &Journal, normalizer: N) -> Self {
        let mut by_known: HashMap<String, KnownAccountBucket> = HashMap::new();
        let mut all_payees: HashSet<String> = HashSet::new();
        let mut token_df: HashMap<String, u32> = HashMap::new();

        // Pass 1: collect distinct normalized payees and per-token DF.
        for txn in &journal.transactions {
            for posting in &txn.postings {
                let normalized = normalizer.normalize(&posting.payee);
                if normalized.is_empty() {
                    continue;
                }
                if all_payees.insert(normalized.clone()) {
                    let unique_toks: HashSet<&str> = normalized.split_whitespace().collect();
                    for tok in unique_toks {
                        *token_df.entry(tok.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }

        // Pass 2: build per-known-account sample buckets.
        for txn in &journal.transactions {
            let date = txn.date_naive();
            for (i, known) in txn.postings.iter().enumerate() {
                let normalized = normalizer.normalize(&known.payee);
                if normalized.is_empty() {
                    continue;
                }
                let payee_tokens: Vec<String> =
                    normalized.split_whitespace().map(String::from).collect();

                // Materialize the known posting's amounts once per posting
                // since we iterate them for every counter posting below.
                let known_amounts: Vec<Decimal> = known.amounts().map(|(_, d)| d).collect();

                for (j, counter) in txn.postings.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    for &dec in &known_amounts {
                        if dec.is_zero() {
                            continue;
                        }
                        let bucket = by_known.entry(known.account.clone()).or_default();
                        let idx = bucket.samples.len();
                        bucket.samples.push(Sample {
                            counter_account: counter.account.clone(),
                            amount: dec,
                            date,
                            payee_tokens: payee_tokens.clone(),
                        });
                        bucket
                            .by_payee
                            .entry(normalized.clone())
                            .or_default()
                            .push(idx);
                    }
                }
            }
        }

        Index {
            by_known,
            token_df,
            total_payees: all_payees.len() as u32,
            normalizer: Box::new(normalizer),
        }
    }

    /// Number of historical samples in the exact-match bucket for
    /// `(normalize(raw_payee), known_account)`. Returns 0 if the payee
    /// normalizes to a key not in the index, or if no sample for that key
    /// has the given known_account.
    ///
    /// A query whose bucket has zero samples is a cold-start case for
    /// [`crate::ScoringStrategy::ExactMatch`]; under
    /// [`crate::ScoringStrategy::TokenIdf`] or
    /// [`crate::ScoringStrategy::Hybrid`], some of those cases may still be
    /// recoverable via shared rare tokens.
    pub fn bucket_size(&self, raw_payee: &str, known_account: &str) -> usize {
        let normalized = self.normalizer.normalize(raw_payee);
        self.by_known
            .get(known_account)
            .and_then(|b| b.by_payee.get(&normalized))
            .map(|v| v.len())
            .unwrap_or(0)
    }
}
