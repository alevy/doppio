# doppio-categorize

A counter-account prediction library for the [doppio](https://github.com/alevy/doppio) ledger compiler.

Given a partial transaction (one side known — typically a bank/credit-card account from an import), suggest the most likely counter-account based on patterns in the existing journal.

This is a **classifier**, not a forecaster. It runs at import time, per-transaction, against an index built once from the journal.

## Status

**v0.2 prototype** — lives here in `doppio-research/prototypes/` so the API can shake out before the crate is promoted to a workspace member of doppio and published to crates.io.

## API

```rust
use doppio_categorize::{Index, Query, Config, DefaultNormalizer};

// Build once from a journal:
let index = Index::build(&journal, DefaultNormalizer);

// Query many times:
let query = Query {
    date: chrono::NaiveDate::from_ymd_opt(2024, 4, 27).unwrap(),
    payee: "STARBUCKS #1234 SEATTLE WA".into(),
    amount: rust_decimal::Decimal::new(-758, 2), // -$7.58
    known_account: "Liabilities:Visa".into(),
};
let suggestions = index.suggest(&query, &Config::default());
// → vec![Suggestion { account: "Expenses:Coffee", confidence: 0.83, sample_count: 24, last_seen: ... }, ...]
```

## Algorithm

The pipeline is the same shape across all scoring strategies:

1. Look up the per-known-account bucket. (Bucket missing → empty result.)
2. Pick candidate samples via the configured [`ScoringStrategy`].
3. Apply the **sign filter** (sample's amount sign must match query's — refunds vs charges).
4. Apply **amount-similarity weighting**: each candidate's match-score is multiplied by `1 / (1 + |ln(|query.amount|) - ln(|sample.amount|)|)`. Disable via `Config { use_amount_weighting: false, .. }`.
5. Aggregate `match_score * amount_weight` per `counter_account`.
6. Rank by `weight_sum / total_weight` descending.

What varies across strategies is step 2 — how candidates are picked and how their match-score is computed:

- **`ScoringStrategy::ExactMatch`**: candidates are samples whose normalized payee equals the query's. Match-score = 1.0 for each. This is v0.1 behavior.
- **`ScoringStrategy::TokenIdf { df_threshold }`**: candidates are samples whose tokens overlap with the query's. Match-score for a sample is `Σ ln(N / df(t))` over shared tokens `t`, excluding tokens where `df(t) >= df_threshold`. The threshold filters geographic / structural noise tokens like "seattle", "wa" that appear in too many distinct payees.
- **`ScoringStrategy::Hybrid { df_threshold, .. }`**: try `ExactMatch` first; if it returns no candidates, fall back to `TokenIdf`. **This is the default** since v0.2.

### Normalization (v0.2 default)

`DefaultNormalizer` lowercases alphabetic characters; treats digits, punctuation, and whitespace as word separators; collapses runs of separators. So `"STARBUCKS #1234 SEATTLE WA"` and `"Starbucks Seattle, WA"` both become `"starbucks seattle wa"`.

The `Normalizer` trait is pluggable — a v0.3 implementation can swap in stemming, abbreviation expansion, currency-suffix stripping, etc.

## Evaluation harness

A held-out evaluation harness lives at `examples/eval_holdout.rs`. It:

1. Loads a journal (`.ledger`, `.hledger`, `.journal`, or pre-compiled `.dop`).
2. Filters to 2-posting transactions where exactly one posting matches an "import-side" account regex (default: `Liabilities:.*Visa`, bank/checking/savings/etc).
3. Holds out a fraction of the eligible transactions (10% by default, seeded shuffle with `seed=42`).
4. Builds the index from the remaining transactions.
5. For each held-out transaction, queries with the import-side `(date, payee, amount, known_account)` and checks whether the true counter-account appears at rank 1 / 2-3.
6. Reports top-1 / top-3 accuracy plus stratification by training-cluster size.

```sh
# default: hybrid strategy, df_threshold=50, 10% holdout, amount weighting
cargo run --release --example eval_holdout -- /path/to/journal.ledger

# pin to v0.1 exact-match behavior:
cargo run --release --example eval_holdout -- /path/to/journal.ledger \
    --strategy exact

# pure token-IDF with a tighter df threshold:
cargo run --release --example eval_holdout -- /path/to/journal.ledger \
    --strategy token-idf --df-threshold 20

# different import-side regex for non-default ledger conventions:
cargo run --release --example eval_holdout -- /path/to/journal.ledger \
    --import-regex '^Liabilities:Mercury'
```

## Real-data results

### Better Bytes nonprofit books (188 transactions, 90 eligible)

| Strategy | Top-1 | Top-3 | Cold-start |
|---|---:|---:|---:|
| `exact` | 88.9% | 88.9% | 11.1% |
| `token-idf` (df<50) | 83.3% | 83.3% | 11.1% |
| **`hybrid` (default)** | **88.9%** | **88.9%** | **11.1%** |

Small, consistent corpus. Pure token-IDF *hurts* slightly here because rare tokens cross-match across vendors. Hybrid avoids the regression by preferring exact-match when there's a hit.

### Personal household ledger (3,075 transactions, 2,960 eligible, 20% holdout for stable estimate)

| Strategy | Top-1 | Top-3 |
|---|---:|---:|
| `exact` | 58.8% | 63.9% |
| `token-idf` (df<50) | 68.1% | 78.2% |
| **`hybrid` (default)** | **70.8%** | **78.5%** |

Long-tail corpus where 70% of normalized payees appear exactly once. Hybrid lifts top-1 by **+12 points** over the v0.1 exact-match baseline by recovering cold-start cases through shared rare tokens (e.g. "amazon" matching across `AMZN MKTP US*ABC123` / `AMZN MKTP US*XYZ987` variants). The cold-start *rate* (32.9%) doesn't change — that's a property of the corpus and the holdout — but token-IDF rescues many of those formerly-empty cases.

## v0.3 trajectory

Things deliberately deferred:

- **Recency weighting**: confidence is currently pure frequency × IDF × amount-similarity. v0.3 will weight recent samples higher.
- **Naive Bayes / probabilistic classifier**: real-data evaluation showed cold-start (not classification error) is the dominant failure mode on both bb-org and household corpora. Token-IDF closed most of that gap. NB-style discrimination would help on a corpus with same-payee-different-counter ambiguity (the synthetic PCC $14 vs $200 case), but neither real corpus exhibits much of this. Worth revisiting with more downstream consumers.
- **Multi-posting pattern recognition**: each posting in an N-posting transaction is currently an independent sample. v0.3 may recognize "Starbucks usually splits as Coffee + Tax" and surface structured multi-posting suggestions.
- **Online learning / feedback**: the index is stateless — the caller rebuilds when the journal changes. No `Index::observe(accepted_suggestion)` API yet.
- **Custom normalizers documented and tested**: the `Normalizer` trait is pluggable today, but only `DefaultNormalizer` is shipped. v0.3 will exercise the customization path.

## Notes on the doppio API

See `NOTES.md` for ergonomic friction encountered against the doppio library API during development. Each entry is a candidate for a tracking issue against doppio core.
