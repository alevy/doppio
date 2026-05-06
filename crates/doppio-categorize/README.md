# doppio-categorize

A counter-account prediction library for the
[doppio](https://crates.io/crates/doppio) ledger compiler.

Given a partial transaction (one side known -- typically a
bank/credit-card account from an import), suggest the most likely
counter-account based on patterns in the existing journal.

This is a **classifier**, not a forecaster. It runs at import time,
per-transaction, against an index built once from the journal.

## API

```rust
use doppio_categorize::{Index, Query, Config, DefaultNormalizer};

// Build once from a journal.
let index = Index::build(&journal, DefaultNormalizer);

// Query many times.
let query = Query {
    date: chrono::NaiveDate::from_ymd_opt(2024, 4, 27).unwrap(),
    payee: "STARBUCKS #1234 SEATTLE WA".into(),
    amount: rust_decimal::Decimal::new(-758, 2), // -$7.58
    known_account: "Liabilities:Visa".into(),
};
let suggestions = index.suggest(&query, &Config::default());
// -> vec![Suggestion { account: "Expenses:Coffee", confidence: 0.83, sample_count: 24, last_seen: ... }, ...]
```

## Algorithm

The pipeline is the same shape across all scoring strategies:

1. Look up the per-known-account bucket. (Bucket missing -> empty result.)
2. Pick candidate samples via the configured `ScoringStrategy`.
3. Apply the **sign filter** (sample's amount sign must match
   query's -- refunds vs charges).
4. Apply **amount-similarity weighting**: each candidate's match-score
   is multiplied by `1 / (1 + |ln(|query.amount|) - ln(|sample.amount|)|)`.
   Disable via `Config { use_amount_weighting: false, .. }`.
5. Aggregate `match_score * amount_weight` per `counter_account`.
6. Rank by `weight_sum / total_weight` descending.

What varies across strategies is step 2 -- how candidates are picked
and how their match-score is computed:

- **`ScoringStrategy::ExactMatch`**: candidates are samples whose
  normalized payee equals the query's. Match-score = 1.0 for each.
- **`ScoringStrategy::TokenIdf { df_threshold }`**: candidates are
  samples whose tokens overlap with the query's. Match-score for a
  sample is `Sum_t ln(N / df(t))` over shared tokens `t`, excluding
  tokens where `df(t) >= df_threshold`. The threshold filters
  geographic / structural noise tokens like "seattle", "wa" that
  appear in too many distinct payees.
- **`ScoringStrategy::Hybrid { df_threshold, .. }`** *(default)*: try
  `ExactMatch` first; if it returns no candidates, fall back to
  `TokenIdf`.

### Normalization

`DefaultNormalizer` lowercases alphabetic characters; treats digits,
punctuation, and whitespace as word separators; collapses runs of
separators. So `"STARBUCKS #1234 SEATTLE WA"` and `"Starbucks Seattle, WA"`
both become `"starbucks seattle wa"`.

The `Normalizer` trait is pluggable -- a future implementation can
swap in stemming, abbreviation expansion, currency-suffix stripping,
etc.

## Evaluation harness

A held-out evaluation harness lives at
`examples/eval_holdout.rs` in the repository. It:

1. Loads a journal (`.ledger`, `.hledger`, `.journal`, or pre-compiled
   `.dop`).
2. Filters to 2-posting transactions where exactly one posting
   matches an "import-side" account regex (default:
   `Liabilities:.*Visa`, bank/checking/savings/etc).
3. Holds out a fraction of the eligible transactions (10% by
   default, seeded shuffle with `seed=42`).
4. Builds the index from the remaining transactions.
5. For each held-out transaction, queries with the import-side
   `(date, payee, amount, known_account)` and checks whether the
   true counter-account appears at rank 1 / 2-3.
6. Reports top-1 / top-3 accuracy plus stratification by
   training-cluster size.

```sh
# Default: hybrid strategy, df_threshold=50, 10% holdout, amount weighting.
cargo run --release --example eval_holdout -- /path/to/journal.ledger

# Pin to exact-match behavior.
cargo run --release --example eval_holdout -- /path/to/journal.ledger \
    --strategy exact

# Pure token-IDF with a tighter df threshold.
cargo run --release --example eval_holdout -- /path/to/journal.ledger \
    --strategy token-idf --df-threshold 20

# Different import-side regex for non-default ledger conventions.
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

Small, consistent corpus. Pure token-IDF *hurts* slightly because
rare tokens cross-match across vendors. Hybrid avoids the regression
by preferring exact-match when there's a hit.

### Personal household ledger (3,075 transactions, 2,960 eligible, 20% holdout)

| Strategy | Top-1 | Top-3 |
|---|---:|---:|
| `exact` | 58.8% | 63.9% |
| `token-idf` (df<50) | 68.1% | 78.2% |
| **`hybrid` (default)** | **70.8%** | **78.5%** |

Long-tail corpus where 70% of normalized payees appear exactly once.
Hybrid lifts top-1 by **+12 points** over the exact-match baseline by
recovering cold-start cases through shared rare tokens (e.g.
"amazon" matching across `AMZN MKTP US*ABC123` /
`AMZN MKTP US*XYZ987` variants). The cold-start *rate* (32.9%)
doesn't change -- that's a property of the corpus and the holdout --
but token-IDF rescues many of those formerly-empty cases.

## Companion crates

- **[`doppio`](https://crates.io/crates/doppio)** -- the library this
  one is built against. Provides the `Journal` type and the
  compilation pipeline.
- **[`doppio-cli`](https://crates.io/crates/doppio-cli)** -- the
  `dop` binary.

## License

ISC. See [LICENSE](https://github.com/alevy/doppio/blob/main/LICENSE).
