//! Index construction: build sample buckets from a journal.

use chrono::NaiveDate;
use doppio::elaboration::Journal;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};

use crate::normalize::Normalizer;

/// A historical observation: a payee was associated with a counter-account
/// for a given amount on a given date, on the known-side account `known_account`.
///
/// `known_account` is retained as data on each sample for caller inspection
/// and potential future feature use, but is not consulted during scoring.
#[derive(Debug, Clone)]
pub(crate) struct Sample {
    pub counter_account: String,
    pub amount: Decimal,
    pub date: NaiveDate,
    /// The known-side account this sample was indexed from. Retained for
    /// caller inspection and potential future feature use; not consulted during
    /// scoring.
    #[allow(dead_code)]
    pub known_account: String,
    /// Tokens of the normalized payee (used by the token-IDF scorer).
    pub payee_tokens: Vec<String>,
}

/// A built index over historical transactions, ready to answer suggest
/// queries.
///
/// Build once per journal; query many times.
pub struct Index {
    /// Flat list of all samples across all accounts and payees.
    pub(crate) samples: Vec<Sample>,
    /// Normalized-payee → sample indices into `samples`.
    pub(crate) by_payee: HashMap<String, Vec<usize>>,
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
    /// normalized payees in the journal -- needed by the token-IDF scoring
    /// strategy.
    pub fn build<N: Normalizer + 'static>(journal: &Journal, normalizer: N) -> Self {
        let mut samples: Vec<Sample> = Vec::new();
        let mut by_payee: HashMap<String, Vec<usize>> = HashMap::new();
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

        // Pass 2: build flat sample list with global by_payee index.
        for txn in &journal.transactions {
            let date = txn.date_naive();
            for (i, known) in txn.postings.iter().enumerate() {
                let normalized = normalizer.normalize(&known.payee);
                if normalized.is_empty() {
                    continue;
                }
                let payee_tokens: Vec<String> =
                    normalized.split_whitespace().map(String::from).collect();

                let known_amounts: Vec<Decimal> = known.amounts().map(|(_, d)| d).collect();

                for (j, counter) in txn.postings.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    for &dec in &known_amounts {
                        if dec.is_zero() {
                            continue;
                        }
                        let idx = samples.len();
                        samples.push(Sample {
                            counter_account: counter.account.clone(),
                            amount: dec,
                            date,
                            known_account: known.account.clone(),
                            payee_tokens: payee_tokens.clone(),
                        });
                        by_payee.entry(normalized.clone()).or_default().push(idx);
                    }
                }
            }
        }

        Index {
            samples,
            by_payee,
            token_df,
            total_payees: all_payees.len() as u32,
            normalizer: Box::new(normalizer),
        }
    }

    /// Number of historical samples whose payee normalizes to the same key
    /// as `raw_payee`. Returns 0 if the payee normalizes to a key not in the
    /// index.
    ///
    /// The "cluster" is the global payee bucket: all samples across every
    /// known-side account that share this normalized payee. A cluster size of
    /// zero means no history for this payee exists anywhere in the corpus.
    pub fn samples_for_payee_count(&self, raw_payee: &str) -> usize {
        let normalized = self.normalizer.normalize(raw_payee);
        self.by_payee.get(&normalized).map(|v| v.len()).unwrap_or(0)
    }
}
