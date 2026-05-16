//! Query API: given a partial transaction, return ranked counter-account suggestions.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::collections::{HashMap, HashSet};

use crate::index::Index;

/// A partial transaction to classify.
#[derive(Debug, Clone)]
pub struct Query {
    /// Date of the transaction (currently informational; future recency
    /// weighting will use this).
    pub date: NaiveDate,
    /// Raw payee string from the import source. Will be normalized by the
    /// index's normalizer.
    pub payee: String,
    /// The known-side amount. Sign matters: a refund (positive on the
    /// credit-card side) only matches historical refund samples, not charges.
    pub amount: Decimal,
    /// The known-side account (typically the bank/credit-card account that
    /// originated the import). Retained for forward compatibility and caller
    /// inspection; not consulted during scoring.
    pub known_account: String,
}

/// A ranked suggestion for the counter-account.
#[derive(Debug, Clone)]
pub struct Suggestion {
    /// The suggested counter-account.
    pub account: String,
    /// `weighted_votes_for_account / total_weighted_votes`. Higher is better;
    /// always in `[0.0, 1.0]`. Not a probability -- purely relative ranking
    /// among the surviving candidates.
    pub confidence: f64,
    /// Unweighted count of historical samples backing this suggestion.
    /// Useful for distinguishing "1 sample at high amount-similarity weight"
    /// from "50 samples at low weight".
    pub sample_count: u32,
    /// The most recent date this counter-account was observed for the
    /// query payee.
    pub last_seen: NaiveDate,
}

/// Strategy used to find candidate samples for a query.
#[derive(Debug, Clone)]
pub enum ScoringStrategy {
    /// Exact match on the normalized payee. Fast and precise when the exact
    /// normalized form has been seen before; useless for payee variants that
    /// differ in noise tokens (different store numbers, different city codes).
    ExactMatch,
    /// Tokenize the normalized payee on whitespace, weight each token by
    /// IDF (`ln(N / df(token))`) over the corpus of distinct normalized
    /// payees, exclude tokens that appear in `df_threshold` or more
    /// distinct payees. The relevance score for a sample is the sum of
    /// IDF-weights for tokens shared with the query.
    ///
    /// `df_threshold` filters out common geographic / structural tokens
    /// like "seattle", "wa", "ca" that would over-collapse unrelated
    /// payees. A value around 50 is a reasonable starting point on a
    /// multi-thousand-transaction journal.
    TokenIdf { df_threshold: u32 },
    /// Try [`ScoringStrategy::ExactMatch`] first; if no candidates are
    /// found, fall back to [`ScoringStrategy::TokenIdf`]. This is the
    /// recommended default -- exact matches are still the strongest signal
    /// when available, and IDF rescues the cold-start cases.
    Hybrid {
        /// Currently always `true`. Reserved for future extension.
        exact_first: bool,
        df_threshold: u32,
    },
}

impl Default for ScoringStrategy {
    fn default() -> Self {
        ScoringStrategy::Hybrid {
            exact_first: true,
            df_threshold: 50,
        }
    }
}

/// Tunables for the suggest algorithm.
#[derive(Debug, Clone)]
pub struct Config {
    /// If true (default), each surviving sample contributes a weight
    /// `1 / (1 + |ln(query.amount) - ln(sample.amount)|)`. If false, every
    /// surviving sample contributes weight 1.0 from amount-similarity
    /// (the per-strategy match weight is unaffected).
    pub use_amount_weighting: bool,
    /// Strategy for finding candidate samples. Default is the hybrid strategy.
    pub strategy: ScoringStrategy,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            use_amount_weighting: true,
            strategy: ScoringStrategy::default(),
        }
    }
}

impl Index {
    /// Rank candidate counter-accounts for a partial transaction.
    ///
    /// Algorithm:
    /// 1. Normalize the query's payee.
    /// 2. Use the configured [`ScoringStrategy`] to produce
    ///    `(sample_index, match_score)` candidates from the global sample pool.
    /// 3. For each candidate, apply the sign filter (sample sign must match
    ///    query sign), then the amount-similarity weight (if enabled).
    /// 4. Aggregate `match_score * amount_weight` per counter_account.
    /// 5. Rank by `weight_sum / total_weight` desc.
    ///
    /// `query.known_account` is not consulted during scoring. All samples
    /// across every known-side account are eligible candidates, subject to
    /// the sign filter and the configured strategy.
    pub fn suggest(&self, query: &Query, config: &Config) -> Vec<Suggestion> {
        let normalized = self.normalizer.normalize(&query.payee);
        let candidates = self.candidates(&normalized, &config.strategy);
        if candidates.is_empty() {
            return Vec::new();
        }
        self.rank_candidates(&candidates, query, config)
    }

    fn candidates(&self, normalized: &str, strategy: &ScoringStrategy) -> Vec<(usize, f64)> {
        match strategy {
            ScoringStrategy::ExactMatch => exact_match_candidates(self, normalized),
            ScoringStrategy::TokenIdf { df_threshold } => {
                token_idf_candidates(self, normalized, *df_threshold)
            }
            ScoringStrategy::Hybrid {
                exact_first: _,
                df_threshold,
            } => {
                // exact_first is reserved for future extension; today we always
                // try exact first and fall back to token-IDF.
                let exact = exact_match_candidates(self, normalized);
                if !exact.is_empty() {
                    exact
                } else {
                    token_idf_candidates(self, normalized, *df_threshold)
                }
            }
        }
    }

    fn rank_candidates(
        &self,
        candidates: &[(usize, f64)],
        query: &Query,
        config: &Config,
    ) -> Vec<Suggestion> {
        let query_sign = query.amount.is_sign_negative();
        let query_log = log_abs(query.amount);

        struct Acc {
            weight_sum: f64,
            count: u32,
            last_seen: NaiveDate,
        }
        let mut by_account: HashMap<String, Acc> = HashMap::new();

        for &(idx, match_score) in candidates {
            let sample = &self.samples[idx];
            if sample.amount.is_sign_negative() != query_sign {
                continue;
            }
            let amount_weight = if config.use_amount_weighting {
                let sample_log = log_abs(sample.amount);
                match (query_log, sample_log) {
                    (Some(q), Some(s)) => 1.0 / (1.0 + (q - s).abs()),
                    _ => 0.0,
                }
            } else {
                1.0
            };
            let weight = match_score * amount_weight;
            if weight <= 0.0 {
                continue;
            }
            let entry = by_account
                .entry(sample.counter_account.clone())
                .or_insert(Acc {
                    weight_sum: 0.0,
                    count: 0,
                    last_seen: sample.date,
                });
            entry.weight_sum += weight;
            entry.count += 1;
            if sample.date > entry.last_seen {
                entry.last_seen = sample.date;
            }
        }

        let total_weight: f64 = by_account.values().map(|a| a.weight_sum).sum();
        if total_weight <= 0.0 {
            return Vec::new();
        }
        let mut suggestions: Vec<Suggestion> = by_account
            .into_iter()
            .map(|(account, acc)| Suggestion {
                account,
                confidence: acc.weight_sum / total_weight,
                sample_count: acc.count,
                last_seen: acc.last_seen,
            })
            .collect();
        suggestions.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        suggestions
    }
}

fn exact_match_candidates(index: &Index, normalized: &str) -> Vec<(usize, f64)> {
    index
        .by_payee
        .get(normalized)
        .map(|idxs| idxs.iter().map(|&i| (i, 1.0)).collect())
        .unwrap_or_default()
}

fn token_idf_candidates(index: &Index, normalized: &str, df_threshold: u32) -> Vec<(usize, f64)> {
    let q_tokens: HashSet<&str> = normalized.split_whitespace().collect();
    if q_tokens.is_empty() {
        return Vec::new();
    }
    let total_payees = index.total_payees as f64;
    let mut out = Vec::new();
    for (i, sample) in index.samples.iter().enumerate() {
        let mut score = 0.0;
        let s_tokens: HashSet<&str> = sample.payee_tokens.iter().map(String::as_str).collect();
        for tok in q_tokens.intersection(&s_tokens) {
            let df = index.token_df.get(*tok).copied().unwrap_or(0);
            if df == 0 || df >= df_threshold {
                continue;
            }
            score += (total_payees / df as f64).ln();
        }
        if score > 0.0 {
            out.push((i, score));
        }
    }
    out
}

fn log_abs(d: Decimal) -> Option<f64> {
    let f = d.abs().to_f64()?;
    if f > 0.0 { Some(f.ln()) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Sample;
    use crate::normalize::DefaultNormalizer;
    use chrono::NaiveDate;
    use rust_decimal_macros::dec;
    use std::collections::HashMap;

    fn d(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn sample(counter: &str, amount: Decimal, date: NaiveDate, tokens: &[&str]) -> Sample {
        Sample {
            counter_account: counter.to_string(),
            amount,
            date,
            known_account: "Liabilities:Visa".to_string(),
            payee_tokens: tokens.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// A synthetic Index built without going through `Index::build`. Useful
    /// for testing `rank_candidates` and the candidate functions in isolation,
    /// without depending on Journal-fixture plumbing.
    fn build_synthetic_index(
        all_samples: Vec<Sample>,
        by_payee: HashMap<String, Vec<usize>>,
        token_df: HashMap<String, u32>,
        total_payees: u32,
    ) -> Index {
        Index {
            samples: all_samples,
            by_payee,
            token_df,
            total_payees,
            normalizer: Box::new(DefaultNormalizer),
        }
    }

    // ------------------------------------------------------------------
    // log_abs
    // ------------------------------------------------------------------

    #[test]
    fn log_abs_positive_matches_ln() {
        let e: Decimal = dec!(2.71828182845904523536);
        let v = log_abs(e).unwrap();
        assert!((v - 1.0).abs() < 1e-9, "log_abs(e) ≈ 1.0, got {v}");
    }

    #[test]
    fn log_abs_negative_uses_absolute_value() {
        let pos = log_abs(dec!(7.58)).unwrap();
        let neg = log_abs(dec!(-7.58)).unwrap();
        assert!((pos - neg).abs() < 1e-12);
    }

    #[test]
    fn log_abs_zero_returns_none() {
        assert!(log_abs(Decimal::ZERO).is_none());
    }

    // ------------------------------------------------------------------
    // exact_match_candidates
    // ------------------------------------------------------------------

    #[test]
    fn exact_match_miss_returns_empty() {
        let index = build_synthetic_index(Vec::new(), HashMap::new(), HashMap::new(), 0);
        assert!(exact_match_candidates(&index, "starbucks").is_empty());
    }

    #[test]
    fn exact_match_hit_returns_unit_weights() {
        let mut by_payee = HashMap::new();
        by_payee.insert("starbucks".to_string(), vec![0usize, 2, 5]);
        let index = build_synthetic_index(Vec::new(), by_payee, HashMap::new(), 1);
        let cands = exact_match_candidates(&index, "starbucks");
        assert_eq!(cands, vec![(0, 1.0), (2, 1.0), (5, 1.0)]);
    }

    // ------------------------------------------------------------------
    // token_idf_candidates
    // ------------------------------------------------------------------

    #[test]
    fn token_idf_empty_query_returns_empty() {
        let index = build_synthetic_index(
            vec![sample(
                "Expenses:Coffee",
                dec!(7.58),
                d(2024, 1, 1),
                &["starbucks"],
            )],
            HashMap::new(),
            HashMap::from([("starbucks".to_string(), 1u32)]),
            1,
        );
        assert!(token_idf_candidates(&index, "", 50).is_empty());
        assert!(token_idf_candidates(&index, "   ", 50).is_empty());
    }

    #[test]
    fn token_idf_skips_tokens_at_or_above_df_threshold() {
        // 2 distinct payees; "seattle" appears in both (df=2), "starbucks"
        // only in one (df=1). With df_threshold=2, "seattle" is excluded;
        // only "starbucks" contributes.
        let samples = vec![
            sample(
                "Expenses:Coffee",
                dec!(7.58),
                d(2024, 1, 1),
                &["starbucks", "seattle"],
            ),
            sample(
                "Expenses:Dining",
                dec!(30.00),
                d(2024, 1, 2),
                &["bistro", "seattle"],
            ),
        ];
        let token_df = HashMap::from([
            ("starbucks".to_string(), 1u32),
            ("bistro".to_string(), 1u32),
            ("seattle".to_string(), 2u32),
        ]);
        let index = build_synthetic_index(samples, HashMap::new(), token_df, 2);
        let cands = token_idf_candidates(&index, "starbucks seattle", 2);
        // Only sample 0 ("starbucks seattle") shares a non-filtered token.
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].0, 0);
        // Score = ln(N / df("starbucks")) = ln(2/1) = ln(2)
        let expected = (2.0_f64 / 1.0).ln();
        assert!((cands[0].1 - expected).abs() < 1e-9);
    }

    #[test]
    fn token_idf_no_shared_tokens_yields_no_candidates() {
        let samples = vec![sample(
            "Expenses:Coffee",
            dec!(7.58),
            d(2024, 1, 1),
            &["starbucks"],
        )];
        let token_df = HashMap::from([("starbucks".to_string(), 1u32)]);
        let index = build_synthetic_index(samples, HashMap::new(), token_df, 1);
        assert!(token_idf_candidates(&index, "comcast", 50).is_empty());
    }

    #[test]
    fn token_idf_unknown_token_in_df_table_skipped() {
        let samples = vec![sample(
            "Expenses:Coffee",
            dec!(7.58),
            d(2024, 1, 1),
            &["starbucks"],
        )];
        let index = build_synthetic_index(samples, HashMap::new(), HashMap::new(), 1);
        let cands = token_idf_candidates(&index, "starbucks", 50);
        assert!(
            cands.is_empty(),
            "df=0 token should not contribute (would divide by zero)"
        );
    }

    // ------------------------------------------------------------------
    // suggest end-to-end (rank_candidates aggregation)
    // ------------------------------------------------------------------

    #[test]
    fn sign_filter_excludes_opposite_sign_samples() {
        // Two samples for the same exact-match key, opposite signs.
        let mut by_payee = HashMap::new();
        by_payee.insert("starbucks".to_string(), vec![0usize, 1]);
        let index = build_synthetic_index(
            vec![
                sample(
                    "Expenses:Coffee",
                    dec!(-7.58),
                    d(2024, 1, 1),
                    &["starbucks"],
                ),
                sample("Income:Refunds", dec!(7.58), d(2024, 1, 2), &["starbucks"]),
            ],
            by_payee,
            HashMap::from([("starbucks".to_string(), 1u32)]),
            1,
        );
        // Negative query: charges only.
        let q = Query {
            date: d(2024, 2, 1),
            payee: "Starbucks".into(),
            amount: dec!(-7.58),
            known_account: "Liabilities:Visa".into(),
        };
        let s = index.suggest(&q, &Config::default());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].account, "Expenses:Coffee");
    }

    #[test]
    fn confidence_sums_to_one_across_returned_suggestions() {
        // Three samples, three different counter accounts, all exact-match.
        let mut by_payee = HashMap::new();
        by_payee.insert("acme".to_string(), vec![0usize, 1, 2]);
        let index = build_synthetic_index(
            vec![
                sample("Expenses:A", dec!(-10.00), d(2024, 1, 1), &["acme"]),
                sample("Expenses:B", dec!(-10.00), d(2024, 1, 2), &["acme"]),
                sample("Expenses:C", dec!(-10.00), d(2024, 1, 3), &["acme"]),
            ],
            by_payee,
            HashMap::from([("acme".to_string(), 1u32)]),
            1,
        );
        let q = Query {
            date: d(2024, 2, 1),
            payee: "Acme".into(),
            amount: dec!(-10.00),
            known_account: "Liabilities:Visa".into(),
        };
        let cfg = Config {
            use_amount_weighting: false,
            strategy: ScoringStrategy::ExactMatch,
        };
        let s = index.suggest(&q, &cfg);
        assert_eq!(s.len(), 3);
        let total: f64 = s.iter().map(|x| x.confidence).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "confidence sum = {total}, expected ~1.0"
        );
        for sug in &s {
            assert!((sug.confidence - 1.0 / 3.0).abs() < 1e-9);
        }
    }

    #[test]
    fn rank_orders_by_confidence_descending() {
        // 3 samples -> Expenses:Common, 1 sample -> Expenses:Rare.
        let mut by_payee = HashMap::new();
        by_payee.insert("acme".to_string(), vec![0usize, 1, 2, 3]);
        let index = build_synthetic_index(
            vec![
                sample("Expenses:Common", dec!(-10.00), d(2024, 1, 1), &["acme"]),
                sample("Expenses:Common", dec!(-10.00), d(2024, 1, 2), &["acme"]),
                sample("Expenses:Common", dec!(-10.00), d(2024, 1, 3), &["acme"]),
                sample("Expenses:Rare", dec!(-10.00), d(2024, 1, 4), &["acme"]),
            ],
            by_payee,
            HashMap::from([("acme".to_string(), 1u32)]),
            1,
        );
        let q = Query {
            date: d(2024, 2, 1),
            payee: "Acme".into(),
            amount: dec!(-10.00),
            known_account: "Liabilities:Visa".into(),
        };
        let s = index.suggest(&q, &Config::default());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].account, "Expenses:Common");
        assert_eq!(s[0].sample_count, 3);
        assert_eq!(s[1].account, "Expenses:Rare");
        assert_eq!(s[1].sample_count, 1);
        assert!(s[0].confidence > s[1].confidence);
    }

    #[test]
    fn last_seen_tracks_most_recent_sample_date() {
        let mut by_payee = HashMap::new();
        by_payee.insert("acme".to_string(), vec![0usize, 1, 2]);
        let index = build_synthetic_index(
            vec![
                sample("Expenses:A", dec!(-10.00), d(2024, 1, 5), &["acme"]),
                sample("Expenses:A", dec!(-10.00), d(2024, 3, 1), &["acme"]),
                sample("Expenses:A", dec!(-10.00), d(2024, 2, 14), &["acme"]),
            ],
            by_payee,
            HashMap::from([("acme".to_string(), 1u32)]),
            1,
        );
        let q = Query {
            date: d(2024, 4, 1),
            payee: "Acme".into(),
            amount: dec!(-10.00),
            known_account: "Liabilities:Visa".into(),
        };
        let s = index.suggest(&q, &Config::default());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].last_seen, d(2024, 3, 1));
        assert_eq!(s[0].sample_count, 3);
    }

    #[test]
    fn amount_weighting_decays_with_log_distance() {
        let mut by_payee = HashMap::new();
        by_payee.insert("acme".to_string(), vec![0usize, 1]);
        let index = build_synthetic_index(
            vec![
                sample("Expenses:Match", dec!(-10.00), d(2024, 1, 1), &["acme"]),
                sample("Expenses:Other", dec!(-1000.00), d(2024, 1, 2), &["acme"]),
            ],
            by_payee,
            HashMap::from([("acme".to_string(), 1u32)]),
            1,
        );
        let q = Query {
            date: d(2024, 2, 1),
            payee: "Acme".into(),
            amount: dec!(-10.00),
            known_account: "Liabilities:Visa".into(),
        };

        // Weighting on: Match >> Other.
        let s_on = index.suggest(&q, &Config::default());
        assert_eq!(s_on.len(), 2);
        assert_eq!(s_on[0].account, "Expenses:Match");
        let ratio = s_on[0].confidence / s_on[1].confidence;
        assert!(
            ratio > 4.0,
            "amount weighting should heavily favor the close sample, ratio={ratio}"
        );

        // Weighting off: equal confidences.
        let s_off = index.suggest(
            &q,
            &Config {
                use_amount_weighting: false,
                ..Config::default()
            },
        );
        assert_eq!(s_off.len(), 2);
        assert!((s_off[0].confidence - s_off[1].confidence).abs() < 1e-9);
    }

    #[test]
    fn zero_query_amount_returns_empty_when_weighted() {
        // log_abs(0) = None, so when amount weighting is on, every sample
        // gets weight 0 and the result is empty. Use a positive-amount sample
        // to isolate the log_abs(0) -> 0-weight path (sign filter doesn't
        // interfere).
        let mut by_payee = HashMap::new();
        by_payee.insert("acme".to_string(), vec![0usize]);
        let index = build_synthetic_index(
            vec![sample("Income:A", dec!(10.00), d(2024, 1, 1), &["acme"])],
            by_payee,
            HashMap::from([("acme".to_string(), 1u32)]),
            1,
        );
        let q = Query {
            date: d(2024, 2, 1),
            payee: "Acme".into(),
            amount: Decimal::ZERO,
            known_account: "Liabilities:Visa".into(),
        };
        let s = index.suggest(&q, &Config::default());
        assert!(
            s.is_empty(),
            "zero query amount with weighting on should yield no suggestions"
        );
    }

    #[test]
    fn unknown_known_account_still_gets_suggestions() {
        // Under v0.2, known_account is not consulted. Samples indexed from
        // "Liabilities:Visa" are candidates even when the query specifies
        // "Liabilities:Nope" (which has no history of its own).
        let mut by_payee = HashMap::new();
        by_payee.insert("starbucks".to_string(), vec![0usize]);
        let index = build_synthetic_index(
            vec![sample(
                "Expenses:Coffee",
                dec!(-7.58),
                d(2024, 1, 1),
                &["starbucks"],
            )],
            by_payee,
            HashMap::from([("starbucks".to_string(), 1u32)]),
            1,
        );
        let q = Query {
            date: d(2024, 2, 1),
            payee: "Starbucks".into(),
            amount: dec!(-7.58),
            known_account: "Liabilities:Nope".into(),
        };
        let s = index.suggest(&q, &Config::default());
        assert_eq!(
            s.len(),
            1,
            "unknown known_account must not suppress suggestions"
        );
        assert_eq!(s[0].account, "Expenses:Coffee");
    }

    #[test]
    fn hybrid_falls_through_to_token_idf_when_exact_misses() {
        // No exact-match key, but the sample shares a rare token with the
        // query. Hybrid should fall back to TokenIdf and recover.
        let index = build_synthetic_index(
            vec![sample(
                "Expenses:Coffee",
                dec!(-7.58),
                d(2024, 1, 1),
                &["starbucks", "seattle"],
            )],
            // by_payee is empty: no exact-match candidates available.
            HashMap::new(),
            HashMap::from([
                ("starbucks".to_string(), 1u32),
                ("seattle".to_string(), 1u32),
            ]),
            2,
        );
        let q = Query {
            date: d(2024, 2, 1),
            payee: "starbucks portland".into(),
            amount: dec!(-6.00),
            known_account: "Liabilities:Visa".into(),
        };
        let s = index.suggest(&q, &Config::default());
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].account, "Expenses:Coffee");
    }
}
