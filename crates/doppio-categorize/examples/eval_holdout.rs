//! Held-out evaluation harness for doppio-categorize.
//!
//! Five mutually-exclusive evaluation modes:
//!
//! 1. **Uniform holdout** (default): holds out a random fraction of eligible
//!    transactions, trains on the rest, evaluates the held-out set.
//!
//! 2. **Single-account cold-start** (`--cold-account ACCOUNT`): holds out
//!    every eligible transaction whose import-side posting matches `ACCOUNT`
//!    by exact string equality; trains on everything else.
//!
//! 3. **Leave-one-account-out** (`--leave-one-account-out [N]`): for each
//!    distinct known_account with ≥ N eligible transactions, holds out that
//!    account, trains on the rest, evaluates. Reports per-account results
//!    plus cohort mean (each account counts once) and size-weighted aggregate.
//!
//! 4. **Account replacement** (`--account-replacement ACCOUNT [--replacement-fraction F]`):
//!    simulates closing `ACCOUNT` and opening a replacement card with the same
//!    spending patterns. Holds out a random fraction `F` (default 0.10) of
//!    that account's eligible transactions. The remaining `1-F` stay in
//!    training. Held-out queries use `ACCOUNT + "-Replacement"` as
//!    `known_account`. Under the v0.2 payee-primary architecture `known_account`
//!    is not used for candidate selection, so the classifier recovers signal
//!    from all accounts sharing the same payees.
//!
//! 5. **Account-replacement cohort** (`--account-replacement-cohort [N]`): for
//!    each distinct known_account with ≥ N eligible transactions, runs the
//!    single-account replacement variant and reports per-account results plus
//!    cohort mean and size-weighted aggregate.
//!
//! Usage:
//!
//! ```sh
//! cargo run --release --example eval_holdout -- \
//!   <journal-path> \
//!   [--import-regex REGEX] \
//!   [--no-amount-weighting] \
//!   [--strategy exact|token-idf|hybrid] \
//!   [--df-threshold N] \
//!   [--holdout 0.10] [--seed N] \
//!   [--cold-account ACCOUNT] \
//!   [--leave-one-account-out [MIN_TXN_COUNT]] \
//!   [--account-replacement ACCOUNT [--replacement-fraction F]] \
//!   [--account-replacement-cohort [MIN_TXN_COUNT]]
//! ```
//!
//! The journal can be a `.ledger`, `.hledger`, `.journal`, or `.dop` file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

use doppio::elaboration::{Journal, Transaction};
use doppio_categorize::{Config, DefaultNormalizer, Index, Query, ScoringStrategy};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use regex::Regex;
use rust_decimal::Decimal;

const DEFAULT_HOLDOUT: f64 = 0.10;
const DEFAULT_SEED: u64 = 42;
const DEFAULT_IMPORT_REGEX: &str = r"(?i)^(Assets:.*Bank|Assets:.*Checking|Assets:.*Saving|Assets:.*Cash|Liabilities:.*Card|Liabilities:.*Visa|Liabilities:.*Mastercard|Liabilities:Credit)";
const DEFAULT_LOAO_MIN: usize = 5;
/// Suffix appended to the original account name to form the synthetic
/// `known_account` used in account-replacement queries.  The suffix must not
/// match any real account in a typical ledger.  Under the v0.2 payee-primary
/// architecture `known_account` is not used in candidate selection, so the
/// suffix has no effect on scoring — the purpose is solely to signal that this
/// is a synthetic replacement scenario, not a warm-start query.
const REPLACEMENT_SUFFIX: &str = "-Replacement";

fn load_journal(path: &Path) -> Result<Journal, Box<dyn std::error::Error>> {
    if path.extension().and_then(|e| e.to_str()) == Some("dop") {
        let mut f = File::open(path)?;
        Ok(doppio::read_dop(&mut f, path)?)
    } else {
        let ext = path.extension().and_then(|e| e.to_str());
        let frontend = doppio::frontend_for_extension(ext);
        let base_path = path.parent().unwrap_or(Path::new(""));
        let mut input = String::new();
        File::open(path)?.read_to_string(&mut input)?;
        let hir = frontend.parse(&input, base_path, &doppio::file_opener)?;
        Ok(doppio::elaborate(hir, &frontend.elaboration_defaults())?)
    }
}

/// Extract a single decimal from a posting's amount: returns the first
/// (and usually only) commodity's value.
fn first_amount(txn: &Transaction, posting_idx: usize) -> Option<Decimal> {
    txn.postings
        .get(posting_idx)?
        .amounts()
        .next()
        .map(|(_, d)| d)
}

/// One row of the evaluation result table.
struct Hit {
    /// 1-indexed rank of the true counter-account in the suggestion list,
    /// or `None` if it was not in the returned suggestions at all.
    rank: Option<usize>,
    /// Number of training samples in the global payee bucket for the query
    /// payee. 0 = cold-start (no corpus history for this payee at all).
    cluster_size: usize,
    /// Confidence of the rank-1 suggestion, only set when rank == Some(1).
    confidence_at_top1: Option<f64>,
}

/// Summary metrics over a set of hits.
struct Metrics {
    total: usize,
    top1: usize,
    top3: usize,
    cold_start: usize,
    avg_conf_top1: f64,
}

impl Metrics {
    fn compute(hits: &[Hit]) -> Self {
        let total = hits.len();
        let top1 = hits.iter().filter(|h| h.rank == Some(1)).count();
        let top3 = hits
            .iter()
            .filter(|h| matches!(h.rank, Some(r) if r <= 3))
            .count();
        let cold_start = hits.iter().filter(|h| h.cluster_size == 0).count();
        let confs: Vec<f64> = hits.iter().filter_map(|h| h.confidence_at_top1).collect();
        let avg_conf_top1 = if confs.is_empty() {
            0.0
        } else {
            confs.iter().sum::<f64>() / confs.len() as f64
        };
        Metrics {
            total,
            top1,
            top3,
            cold_start,
            avg_conf_top1,
        }
    }

    fn top1_pct(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            100.0 * self.top1 as f64 / self.total as f64
        }
    }

    fn top3_pct(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            100.0 * self.top3 as f64 / self.total as f64
        }
    }

    fn cold_start_pct(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            100.0 * self.cold_start as f64 / self.total as f64
        }
    }
}

fn print_usage() {
    eprintln!(
        "usage: eval_holdout <journal-path> [--import-regex REGEX] [--no-amount-weighting] \
         [--strategy exact|token-idf|hybrid] [--df-threshold N] \
         [--holdout FRACTION] [--seed N] \
         [--cold-account ACCOUNT] \
         [--leave-one-account-out [MIN_COUNT]] \
         [--account-replacement ACCOUNT [--replacement-fraction F]] \
         [--account-replacement-cohort [MIN_COUNT]]"
    );
}

fn parse_strategy(name: &str, df_threshold: u32) -> Option<ScoringStrategy> {
    match name {
        "exact" => Some(ScoringStrategy::ExactMatch),
        "token-idf" => Some(ScoringStrategy::TokenIdf { df_threshold }),
        "hybrid" => Some(ScoringStrategy::Hybrid {
            exact_first: true,
            df_threshold,
        }),
        _ => None,
    }
}

/// Evaluate a set of test transactions against a built index, returning Hit records.
///
/// `import_re` identifies the import-side posting within each transaction.
fn evaluate(
    test_txns: &[Transaction],
    index: &Index,
    config: &Config,
    import_re: &Regex,
) -> Vec<Hit> {
    let mut hits = Vec::new();
    for txn in test_txns {
        let (known_idx, counter_idx) = if import_re.is_match(&txn.postings[0].account) {
            (0usize, 1usize)
        } else {
            (1usize, 0usize)
        };
        let known = &txn.postings[known_idx];
        let counter = &txn.postings[counter_idx];

        let amount = match first_amount(txn, known_idx) {
            Some(d) if !d.is_zero() => d,
            _ => continue,
        };

        let query = Query {
            date: txn.date_naive(),
            payee: known.payee.clone(),
            amount,
            known_account: known.account.clone(),
        };

        let cluster_size = index.samples_for_payee_count(&query.payee);
        let suggestions = index.suggest(&query, config);

        let rank = suggestions
            .iter()
            .position(|s| s.account == counter.account)
            .map(|p| p + 1);
        let confidence_at_top1 = if rank == Some(1) {
            suggestions.first().map(|s| s.confidence)
        } else {
            None
        };

        hits.push(Hit {
            rank,
            cluster_size,
            confidence_at_top1,
        });
    }
    hits
}

/// Build a training Journal from a subset of transactions.
fn make_journal(txns: Vec<Transaction>) -> Journal {
    Journal {
        transactions: txns,
        accounts: BTreeMap::new(),
        commodities: BTreeMap::new(),
        prices: Vec::new(),
    }
}

/// Print the standard per-cluster-size breakdown.
fn print_cluster_breakdown(hits: &[Hit]) {
    println!("Top-1 accuracy by training cluster size:");
    let buckets: [(usize, usize, &str); 4] = [
        (10, usize::MAX, "≥ 10 samples"),
        (3, 9, "3-9 samples"),
        (1, 2, "1-2 samples"),
        (0, 0, "0 samples (cold-start)"),
    ];
    for (lo, hi, label) in buckets {
        let in_bucket: Vec<&Hit> = hits
            .iter()
            .filter(|h| h.cluster_size >= lo && h.cluster_size <= hi)
            .collect();
        let n = in_bucket.len();
        let hits1 = in_bucket.iter().filter(|h| h.rank == Some(1)).count();
        let pct = if n == 0 {
            0.0
        } else {
            100.0 * hits1 as f64 / n as f64
        };
        println!("  {:24}  {:>4} / {:<4}  =  {:>5.1}%", label, hits1, n, pct);
    }
}

/// Print the standard metrics block (used by both uniform and cold-account modes).
fn print_metrics(m: &Metrics) {
    println!("Total queries:               {}", m.total);
    println!(
        "Top-1 accuracy:              {} / {} = {:.1}%",
        m.top1,
        m.total,
        m.top1_pct()
    );
    println!(
        "Top-3 accuracy:              {} / {} = {:.1}%",
        m.top3,
        m.total,
        m.top3_pct()
    );
    println!(
        "Cold-start (payee unknown):  {} / {} = {:.1}%",
        m.cold_start,
        m.total,
        m.cold_start_pct()
    );
    println!("Avg confidence on top-1 hits: {:.3}", m.avg_conf_top1);
}

/// Mode 1 (uniform holdout): original behavior.
fn run_uniform_holdout(
    all_txns: Vec<Transaction>,
    import_re: &Regex,
    config: &Config,
    holdout_fraction: f64,
    seed: u64,
) -> ExitCode {
    let mut eligible: Vec<usize> = Vec::new();
    for (i, txn) in all_txns.iter().enumerate() {
        if txn.postings.len() != 2 {
            continue;
        }
        let m0 = import_re.is_match(&txn.postings[0].account);
        let m1 = import_re.is_match(&txn.postings[1].account);
        if m0 ^ m1 {
            eligible.push(i);
        }
    }
    eprintln!(
        "Eligible 2-posting txns with exactly one import-side posting: {}",
        eligible.len()
    );
    if eligible.is_empty() {
        eprintln!(
            "\nNo eligible transactions matched. Check --import-regex against the \
             account names in your journal."
        );
        return ExitCode::from(1);
    }

    let mut rng = StdRng::seed_from_u64(seed);
    eligible.shuffle(&mut rng);

    let holdout_count = ((eligible.len() as f64) * holdout_fraction)
        .round()
        .max(1.0) as usize;
    let holdout: HashSet<usize> = eligible.iter().take(holdout_count).copied().collect();
    eprintln!(
        "Held out {} transactions; training set has {}",
        holdout.len(),
        all_txns.len() - holdout.len()
    );

    let mut train_txns: Vec<Transaction> = Vec::new();
    let mut test_txns: Vec<Transaction> = Vec::new();
    for (i, txn) in all_txns.into_iter().enumerate() {
        if holdout.contains(&i) {
            test_txns.push(txn);
        } else {
            train_txns.push(txn);
        }
    }

    let index = Index::build(&make_journal(train_txns), DefaultNormalizer);
    eprintln!(
        "Index built. Evaluating {} held-out queries...\n",
        test_txns.len()
    );

    let hits = evaluate(&test_txns, &index, config, import_re);
    if hits.is_empty() {
        eprintln!("No queries were evaluable (all held-out transactions had zero amounts).");
        return ExitCode::from(1);
    }

    let m = Metrics::compute(&hits);
    println!("=== Results ===");
    print_metrics(&m);
    println!();
    print_cluster_breakdown(&hits);

    let acc = m.top1 as f64 / m.total as f64;
    println!();
    if acc >= 0.70 {
        println!("Top-1 accuracy {:.1}% meets the >=70% bar.", acc * 100.0);
    } else {
        println!("Top-1 accuracy {:.1}% is BELOW the >=70% bar.", acc * 100.0);
    }

    ExitCode::from(0)
}

/// Mode 2 (single cold-account holdout): hold out all eligible transactions
/// for one specific known_account; train on everything else.
fn run_cold_account(
    all_txns: Vec<Transaction>,
    import_re: &Regex,
    config: &Config,
    account: &str,
) -> ExitCode {
    // Partition into held-out (import side == account) vs. train (everything else).
    // Only eligible (2-posting, exactly-one-import-side) transactions participate.
    let mut test_txns: Vec<Transaction> = Vec::new();
    let mut train_txns: Vec<Transaction> = Vec::new();

    for txn in all_txns {
        if txn.postings.len() != 2 {
            train_txns.push(txn);
            continue;
        }
        let m0 = import_re.is_match(&txn.postings[0].account);
        let m1 = import_re.is_match(&txn.postings[1].account);
        if !(m0 ^ m1) {
            train_txns.push(txn);
            continue;
        }
        // Eligible. Check if import side is the held-out account.
        let import_account = if m0 {
            &txn.postings[0].account
        } else {
            &txn.postings[1].account
        };
        if import_account == account {
            test_txns.push(txn);
        } else {
            train_txns.push(txn);
        }
    }

    if test_txns.is_empty() {
        eprintln!(
            "Error: no eligible transactions found for account '{account}'. \
             Check that the account name is an exact match and that it is \
             captured by --import-regex."
        );
        return ExitCode::from(1);
    }

    eprintln!(
        "Cold-account mode: held out {} eligible transactions for '{}'",
        test_txns.len(),
        account
    );
    eprintln!(
        "Training on {} transactions (all other txns).",
        train_txns.len()
    );

    let index = Index::build(&make_journal(train_txns), DefaultNormalizer);
    eprintln!(
        "Index built. Evaluating {} held-out queries...\n",
        test_txns.len()
    );

    let hits = evaluate(&test_txns, &index, config, import_re);
    if hits.is_empty() {
        eprintln!("No queries were evaluable (all held-out transactions had zero amounts).");
        return ExitCode::from(1);
    }

    let m = Metrics::compute(&hits);
    println!("=== Cold-Account Results: {} ===", account);
    print_metrics(&m);
    println!();
    print_cluster_breakdown(&hits);

    ExitCode::from(0)
}

/// Mode 3 (leave-one-account-out): for each distinct known_account with ≥ min_count
/// eligible transactions, hold it out, train on rest, evaluate.
fn run_loao(
    all_txns: Vec<Transaction>,
    import_re: &Regex,
    config: &Config,
    min_count: usize,
) -> ExitCode {
    // Classify every transaction as eligible (2-posting, exactly-one-import)
    // or non-eligible. Eligible ones are tagged by their import-side account.
    struct EligibleTxn {
        txn: Transaction,
        import_account: String,
    }

    let mut eligible_txns: Vec<EligibleTxn> = Vec::new();
    let mut ineligible_txns: Vec<Transaction> = Vec::new();

    for txn in all_txns {
        if txn.postings.len() != 2 {
            ineligible_txns.push(txn);
            continue;
        }
        let m0 = import_re.is_match(&txn.postings[0].account);
        let m1 = import_re.is_match(&txn.postings[1].account);
        if !(m0 ^ m1) {
            ineligible_txns.push(txn);
            continue;
        }
        let import_account = if m0 {
            txn.postings[0].account.clone()
        } else {
            txn.postings[1].account.clone()
        };
        eligible_txns.push(EligibleTxn {
            txn,
            import_account,
        });
    }

    eprintln!(
        "Total eligible 2-posting transactions: {}",
        eligible_txns.len()
    );

    // Count per account.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for et in &eligible_txns {
        *counts.entry(et.import_account.as_str()).or_insert(0) += 1;
    }

    let mut accounts_to_eval: Vec<&str> = counts
        .iter()
        .filter(|(_, c)| **c >= min_count)
        .map(|(&acct, _)| acct)
        .collect();
    accounts_to_eval.sort_unstable();

    if accounts_to_eval.is_empty() {
        eprintln!(
            "Error: no known_account has ≥ {min_count} eligible transactions. \
             Lower --leave-one-account-out threshold or check your journal."
        );
        return ExitCode::from(1);
    }

    eprintln!(
        "Leave-one-account-out: {} accounts with ≥ {} transactions will be evaluated.",
        accounts_to_eval.len(),
        min_count
    );
    eprintln!();

    // Per-account results accumulator.
    struct AccountResult {
        account: String,
        n_held_out: usize,
        top1: usize,
        top3: usize,
    }

    let mut account_results: Vec<AccountResult> = Vec::new();
    // All hits across every account (for the aggregate weighted total).
    let mut all_hits: Vec<Hit> = Vec::new();

    for (loop_idx, &account) in accounts_to_eval.iter().enumerate() {
        eprint!(
            "  [{}/{}] {} ... ",
            loop_idx + 1,
            accounts_to_eval.len(),
            account
        );

        let mut test_txns: Vec<Transaction> = Vec::new();
        let mut train_txns: Vec<Transaction> = ineligible_txns.clone();

        for et in &eligible_txns {
            if et.import_account == account {
                test_txns.push(et.txn.clone());
            } else {
                train_txns.push(et.txn.clone());
            }
        }

        let index = Index::build(&make_journal(train_txns), DefaultNormalizer);
        let hits = evaluate(&test_txns, &index, config, import_re);

        let n_eval = hits.len();
        let top1 = hits.iter().filter(|h| h.rank == Some(1)).count();
        let top3 = hits
            .iter()
            .filter(|h| matches!(h.rank, Some(r) if r <= 3))
            .count();

        eprintln!(
            "{} queries  top-1: {:.1}%  top-3: {:.1}%",
            n_eval,
            if n_eval == 0 {
                0.0
            } else {
                100.0 * top1 as f64 / n_eval as f64
            },
            if n_eval == 0 {
                0.0
            } else {
                100.0 * top3 as f64 / n_eval as f64
            }
        );

        account_results.push(AccountResult {
            account: account.to_string(),
            n_held_out: n_eval,
            top1,
            top3,
        });
        all_hits.extend(hits);
    }

    eprintln!();

    if account_results.is_empty() || all_hits.is_empty() {
        eprintln!("No evaluable queries across any account.");
        return ExitCode::from(1);
    }

    // Per-account report.
    println!("=== Leave-One-Account-Out Results (min {min_count} txns) ===");
    println!();
    println!(
        "{:<50}  {:>9}  {:>8}  {:>8}",
        "Account", "N held-out", "Top-1", "Top-3"
    );
    println!("{}", "-".repeat(80));
    for ar in &account_results {
        let top1_pct = if ar.n_held_out == 0 {
            0.0
        } else {
            100.0 * ar.top1 as f64 / ar.n_held_out as f64
        };
        let top3_pct = if ar.n_held_out == 0 {
            0.0
        } else {
            100.0 * ar.top3 as f64 / ar.n_held_out as f64
        };
        println!(
            "{:<50}  {:>9}  {:>7.1}%  {:>7.1}%",
            ar.account, ar.n_held_out, top1_pct, top3_pct
        );
    }
    println!();

    // Cohort mean (each account gets one vote, regardless of size).
    let n_accounts = account_results.len() as f64;
    let mean_top1: f64 = account_results
        .iter()
        .map(|ar| {
            if ar.n_held_out == 0 {
                0.0
            } else {
                ar.top1 as f64 / ar.n_held_out as f64
            }
        })
        .sum::<f64>()
        / n_accounts;
    let mean_top3: f64 = account_results
        .iter()
        .map(|ar| {
            if ar.n_held_out == 0 {
                0.0
            } else {
                ar.top3 as f64 / ar.n_held_out as f64
            }
        })
        .sum::<f64>()
        / n_accounts;

    println!(
        "Cohort mean (unweighted, {} accounts):  top-1 = {:.1}%  top-3 = {:.1}%",
        account_results.len(),
        mean_top1 * 100.0,
        mean_top3 * 100.0
    );

    // Size-weighted aggregate over all held-out queries.
    let agg = Metrics::compute(&all_hits);
    println!(
        "Aggregate total ({} queries, size-weighted):  top-1 = {:.1}%  top-3 = {:.1}%",
        agg.total,
        agg.top1_pct(),
        agg.top3_pct()
    );

    ExitCode::from(0)
}

/// Like [`evaluate`], but overrides `query.known_account` with
/// `synthetic_account` for every held-out transaction.  The ground-truth
/// counter-account comparison is unchanged — it always uses the real posting
/// account from the journal.
fn evaluate_with_replacement(
    test_txns: &[Transaction],
    index: &Index,
    config: &Config,
    import_re: &Regex,
    synthetic_account: &str,
) -> Vec<Hit> {
    let mut hits = Vec::new();
    for txn in test_txns {
        let (known_idx, counter_idx) = if import_re.is_match(&txn.postings[0].account) {
            (0usize, 1usize)
        } else {
            (1usize, 0usize)
        };
        let known = &txn.postings[known_idx];
        let counter = &txn.postings[counter_idx];

        let amount = match first_amount(txn, known_idx) {
            Some(d) if !d.is_zero() => d,
            _ => continue,
        };

        let query = Query {
            date: txn.date_naive(),
            payee: known.payee.clone(),
            amount,
            // Use the synthetic account name so the per-account bucket lookup
            // misses — this is the replacement scenario.
            known_account: synthetic_account.to_string(),
        };

        let cluster_size = index.samples_for_payee_count(&query.payee);
        let suggestions = index.suggest(&query, config);

        let rank = suggestions
            .iter()
            .position(|s| s.account == counter.account)
            .map(|p| p + 1);
        let confidence_at_top1 = if rank == Some(1) {
            suggestions.first().map(|s| s.confidence)
        } else {
            None
        };

        hits.push(Hit {
            rank,
            cluster_size,
            confidence_at_top1,
        });
    }
    hits
}

/// Mode 4 (account replacement): simulate closing `account` and opening a
/// replacement.  Holds out `fraction` of that account's eligible transactions;
/// the rest stay in training alongside all other-account transactions.
///
/// For each held-out transaction a `Query` is built with
/// `known_account = account + REPLACEMENT_SUFFIX`.  The ground-truth
/// counter-account used for hit/miss comparison is unchanged — it is always
/// the real counter-posting from the journal.
fn run_account_replacement(
    all_txns: Vec<Transaction>,
    import_re: &Regex,
    config: &Config,
    account: &str,
    fraction: f64,
    seed: u64,
) -> ExitCode {
    let synthetic_account = format!("{account}{REPLACEMENT_SUFFIX}");

    let mut target_eligible: Vec<Transaction> = Vec::new();
    let mut train_txns: Vec<Transaction> = Vec::new();

    for txn in all_txns {
        if txn.postings.len() != 2 {
            train_txns.push(txn);
            continue;
        }
        let m0 = import_re.is_match(&txn.postings[0].account);
        let m1 = import_re.is_match(&txn.postings[1].account);
        if !(m0 ^ m1) {
            train_txns.push(txn);
            continue;
        }
        let import_account = if m0 {
            &txn.postings[0].account
        } else {
            &txn.postings[1].account
        };
        if import_account == account {
            target_eligible.push(txn);
        } else {
            train_txns.push(txn);
        }
    }

    if target_eligible.is_empty() {
        eprintln!(
            "Error: no eligible transactions found for account '{account}'. \
             Check that the account name is an exact match and that it is \
             captured by --import-regex."
        );
        return ExitCode::from(1);
    }

    let total_eligible = target_eligible.len();
    let mut rng = StdRng::seed_from_u64(seed);
    target_eligible.shuffle(&mut rng);

    let holdout_count = ((total_eligible as f64) * fraction).round().max(1.0) as usize;

    // The held-out slice becomes test; the remainder joins training.
    let test_txns: Vec<Transaction> = target_eligible.drain(..holdout_count).collect();
    let kept_in_train = target_eligible.len();
    train_txns.extend(target_eligible);

    eprintln!("Account-replacement mode: '{account}' has {total_eligible} eligible transactions.");
    eprintln!(
        "  Held out {} (fraction {fraction:.2}); {kept_in_train} kept in training.",
        test_txns.len(),
    );
    eprintln!("  Synthetic known_account for queries: '{synthetic_account}'");
    eprintln!("  Total training transactions: {}", train_txns.len());

    let index = Index::build(&make_journal(train_txns), DefaultNormalizer);
    eprintln!(
        "Index built. Evaluating {} held-out queries...\n",
        test_txns.len()
    );

    let hits = evaluate_with_replacement(&test_txns, &index, config, import_re, &synthetic_account);
    if hits.is_empty() {
        eprintln!("No queries were evaluable (all held-out transactions had zero amounts).");
        return ExitCode::from(1);
    }

    let m = Metrics::compute(&hits);
    println!("=== Account-Replacement Results: {} ===", account);
    println!("(queries use known_account = '{synthetic_account}')");
    print_metrics(&m);
    println!();
    print_cluster_breakdown(&hits);

    ExitCode::from(0)
}

/// Mode 5 (account-replacement cohort): for each distinct known_account with
/// ≥ min_count eligible transactions, run the single-account replacement
/// variant and aggregate.
fn run_account_replacement_cohort(
    all_txns: Vec<Transaction>,
    import_re: &Regex,
    config: &Config,
    min_count: usize,
    fraction: f64,
    seed: u64,
) -> ExitCode {
    struct EligibleTxn {
        txn: Transaction,
        import_account: String,
    }

    let mut eligible_txns: Vec<EligibleTxn> = Vec::new();
    let mut ineligible_txns: Vec<Transaction> = Vec::new();

    for txn in all_txns {
        if txn.postings.len() != 2 {
            ineligible_txns.push(txn);
            continue;
        }
        let m0 = import_re.is_match(&txn.postings[0].account);
        let m1 = import_re.is_match(&txn.postings[1].account);
        if !(m0 ^ m1) {
            ineligible_txns.push(txn);
            continue;
        }
        let import_account = if m0 {
            txn.postings[0].account.clone()
        } else {
            txn.postings[1].account.clone()
        };
        eligible_txns.push(EligibleTxn {
            txn,
            import_account,
        });
    }

    eprintln!(
        "Total eligible 2-posting transactions: {}",
        eligible_txns.len()
    );

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for et in &eligible_txns {
        *counts.entry(et.import_account.as_str()).or_insert(0) += 1;
    }

    let mut accounts_to_eval: Vec<&str> = counts
        .iter()
        .filter(|(_, c)| **c >= min_count)
        .map(|(&acct, _)| acct)
        .collect();
    accounts_to_eval.sort_unstable();

    if accounts_to_eval.is_empty() {
        eprintln!(
            "Error: no known_account has ≥ {min_count} eligible transactions. \
             Lower --account-replacement-cohort threshold or check your journal."
        );
        return ExitCode::from(1);
    }

    eprintln!(
        "Account-replacement cohort: {} accounts with ≥ {} transactions will be evaluated.",
        accounts_to_eval.len(),
        min_count
    );
    eprintln!("Replacement fraction: {fraction:.2}  seed: {seed}");
    eprintln!();

    struct AccountResult {
        account: String,
        n_held_out: usize,
        top1: usize,
        top3: usize,
    }

    let mut account_results: Vec<AccountResult> = Vec::new();
    let mut all_hits: Vec<Hit> = Vec::new();

    for (loop_idx, &account) in accounts_to_eval.iter().enumerate() {
        let synthetic_account = format!("{account}{REPLACEMENT_SUFFIX}");
        eprint!(
            "  [{}/{}] {} ... ",
            loop_idx + 1,
            accounts_to_eval.len(),
            account
        );

        // Collect this account's eligible txns; everything else goes to a base
        // training pool (ineligible + other-account eligible).
        let mut target_eligible: Vec<Transaction> = Vec::new();
        let mut base_train: Vec<Transaction> = ineligible_txns.clone();

        for et in &eligible_txns {
            if et.import_account == account {
                target_eligible.push(et.txn.clone());
            } else {
                base_train.push(et.txn.clone());
            }
        }

        // Shuffle with a per-account seed derived from the global seed so
        // different accounts get independent random splits.
        let account_seed = seed ^ (loop_idx as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut rng = StdRng::seed_from_u64(account_seed);
        target_eligible.shuffle(&mut rng);

        let holdout_count = ((target_eligible.len() as f64) * fraction).round().max(1.0) as usize;

        let test_txns: Vec<Transaction> = target_eligible.drain(..holdout_count).collect();
        // The remainder of the target account's txns stay in training.
        base_train.extend(target_eligible);

        let index = Index::build(&make_journal(base_train), DefaultNormalizer);
        let hits =
            evaluate_with_replacement(&test_txns, &index, config, import_re, &synthetic_account);

        let n_eval = hits.len();
        let top1 = hits.iter().filter(|h| h.rank == Some(1)).count();
        let top3 = hits
            .iter()
            .filter(|h| matches!(h.rank, Some(r) if r <= 3))
            .count();

        eprintln!(
            "{} queries  top-1: {:.1}%  top-3: {:.1}%",
            n_eval,
            if n_eval == 0 {
                0.0
            } else {
                100.0 * top1 as f64 / n_eval as f64
            },
            if n_eval == 0 {
                0.0
            } else {
                100.0 * top3 as f64 / n_eval as f64
            }
        );

        account_results.push(AccountResult {
            account: account.to_string(),
            n_held_out: n_eval,
            top1,
            top3,
        });
        all_hits.extend(hits);
    }

    eprintln!();

    if account_results.is_empty() || all_hits.is_empty() {
        eprintln!("No evaluable queries across any account.");
        return ExitCode::from(1);
    }

    println!(
        "=== Account-Replacement Cohort Results (min {min_count} txns, fraction {fraction:.2}) ==="
    );
    println!("(held-out queries use known_account = original + '{REPLACEMENT_SUFFIX}')");
    println!();
    println!(
        "{:<50}  {:>9}  {:>8}  {:>8}",
        "Account", "N held-out", "Top-1", "Top-3"
    );
    println!("{}", "-".repeat(80));
    for ar in &account_results {
        let top1_pct = if ar.n_held_out == 0 {
            0.0
        } else {
            100.0 * ar.top1 as f64 / ar.n_held_out as f64
        };
        let top3_pct = if ar.n_held_out == 0 {
            0.0
        } else {
            100.0 * ar.top3 as f64 / ar.n_held_out as f64
        };
        println!(
            "{:<50}  {:>9}  {:>7.1}%  {:>7.1}%",
            ar.account, ar.n_held_out, top1_pct, top3_pct
        );
    }
    println!();

    let n_accounts = account_results.len() as f64;
    let mean_top1: f64 = account_results
        .iter()
        .map(|ar| {
            if ar.n_held_out == 0 {
                0.0
            } else {
                ar.top1 as f64 / ar.n_held_out as f64
            }
        })
        .sum::<f64>()
        / n_accounts;
    let mean_top3: f64 = account_results
        .iter()
        .map(|ar| {
            if ar.n_held_out == 0 {
                0.0
            } else {
                ar.top3 as f64 / ar.n_held_out as f64
            }
        })
        .sum::<f64>()
        / n_accounts;

    println!(
        "Cohort mean (unweighted, {} accounts):  top-1 = {:.1}%  top-3 = {:.1}%",
        account_results.len(),
        mean_top1 * 100.0,
        mean_top3 * 100.0
    );

    let agg = Metrics::compute(&all_hits);
    println!(
        "Aggregate total ({} queries, size-weighted):  top-1 = {:.1}%  top-3 = {:.1}%",
        agg.total,
        agg.top1_pct(),
        agg.top3_pct()
    );

    ExitCode::from(0)
}

fn main() -> ExitCode {
    // ---- argument parsing -----------------------------------------------
    let mut args = env::args().skip(1);
    let journal_path = match args.next() {
        Some(p) => p,
        None => {
            print_usage();
            return ExitCode::from(2);
        }
    };
    let mut import_regex = DEFAULT_IMPORT_REGEX.to_string();
    let mut config = Config::default();
    let mut holdout_fraction = DEFAULT_HOLDOUT;
    let mut replacement_fraction = DEFAULT_HOLDOUT;
    let mut seed = DEFAULT_SEED;
    let mut strategy_name = String::from("hybrid");
    let mut df_threshold: u32 = 50;

    // Mode flags — mutually exclusive.
    let mut cold_account: Option<String> = None;
    let mut loao_min: Option<usize> = None;
    let mut acct_replacement: Option<String> = None;
    let mut acct_replacement_cohort_min: Option<usize> = None;

    let args_vec: Vec<String> = std::iter::from_fn(|| args.next()).collect();
    let mut i = 0;
    while i < args_vec.len() {
        match args_vec[i].as_str() {
            "--import-regex" => {
                i += 1;
                if i >= args_vec.len() {
                    eprintln!("--import-regex requires a value");
                    return ExitCode::from(2);
                }
                import_regex = args_vec[i].clone();
            }
            "--no-amount-weighting" => {
                config.use_amount_weighting = false;
            }
            "--strategy" => {
                i += 1;
                if i >= args_vec.len() {
                    eprintln!("--strategy requires a value (exact|token-idf|hybrid)");
                    return ExitCode::from(2);
                }
                strategy_name = args_vec[i].clone();
            }
            "--df-threshold" => {
                i += 1;
                if i >= args_vec.len() {
                    eprintln!("--df-threshold requires a u32");
                    return ExitCode::from(2);
                }
                df_threshold = match args_vec[i].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("--df-threshold requires a u32");
                        return ExitCode::from(2);
                    }
                };
            }
            "--holdout" => {
                i += 1;
                if i >= args_vec.len() {
                    eprintln!("--holdout requires a fraction in [0, 1)");
                    return ExitCode::from(2);
                }
                holdout_fraction = match args_vec[i].parse::<f64>() {
                    Ok(f) if (0.0..1.0).contains(&f) => f,
                    _ => {
                        eprintln!("--holdout requires a fraction in [0, 1)");
                        return ExitCode::from(2);
                    }
                };
            }
            "--replacement-fraction" => {
                i += 1;
                if i >= args_vec.len() {
                    eprintln!("--replacement-fraction requires a fraction in (0, 1)");
                    return ExitCode::from(2);
                }
                replacement_fraction = match args_vec[i].parse::<f64>() {
                    Ok(f) if f > 0.0 && f < 1.0 => f,
                    _ => {
                        eprintln!("--replacement-fraction requires a fraction in (0, 1)");
                        return ExitCode::from(2);
                    }
                };
            }
            "--seed" => {
                i += 1;
                if i >= args_vec.len() {
                    eprintln!("--seed requires a u64");
                    return ExitCode::from(2);
                }
                seed = match args_vec[i].parse() {
                    Ok(s) => s,
                    Err(_) => {
                        eprintln!("--seed requires a u64");
                        return ExitCode::from(2);
                    }
                };
            }
            "--cold-account" => {
                i += 1;
                if i >= args_vec.len() {
                    eprintln!("--cold-account requires an account name");
                    return ExitCode::from(2);
                }
                cold_account = Some(args_vec[i].clone());
            }
            "--leave-one-account-out" => {
                // Optional numeric argument: peek at next token.
                let next_is_value = args_vec
                    .get(i + 1)
                    .map(|s| s.parse::<usize>().is_ok())
                    .unwrap_or(false);
                if next_is_value {
                    i += 1;
                    loao_min = Some(args_vec[i].parse::<usize>().unwrap());
                } else {
                    loao_min = Some(DEFAULT_LOAO_MIN);
                }
            }
            "--account-replacement" => {
                i += 1;
                if i >= args_vec.len() {
                    eprintln!("--account-replacement requires an account name");
                    return ExitCode::from(2);
                }
                acct_replacement = Some(args_vec[i].clone());
            }
            "--account-replacement-cohort" => {
                // Optional numeric argument: peek at next token.
                let next_is_value = args_vec
                    .get(i + 1)
                    .map(|s| s.parse::<usize>().is_ok())
                    .unwrap_or(false);
                if next_is_value {
                    i += 1;
                    acct_replacement_cohort_min = Some(args_vec[i].parse::<usize>().unwrap());
                } else {
                    acct_replacement_cohort_min = Some(DEFAULT_LOAO_MIN);
                }
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_usage();
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    // ---- mutually-exclusive mode check ----------------------------------
    let mode_count = cold_account.is_some() as u8
        + loao_min.is_some() as u8
        + acct_replacement.is_some() as u8
        + acct_replacement_cohort_min.is_some() as u8;
    if mode_count > 1 {
        eprintln!(
            "Error: --cold-account, --leave-one-account-out, --account-replacement, \
             and --account-replacement-cohort are mutually exclusive."
        );
        return ExitCode::from(2);
    }

    // ---- load journal ---------------------------------------------------
    let mut journal = match load_journal(Path::new(&journal_path)) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("failed to load journal: {e}");
            return ExitCode::from(1);
        }
    };

    let import_re = match Regex::new(&import_regex) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("invalid --import-regex: {e}");
            return ExitCode::from(2);
        }
    };

    config.strategy = match parse_strategy(&strategy_name, df_threshold) {
        Some(s) => s,
        None => {
            eprintln!("--strategy must be one of: exact, token-idf, hybrid");
            return ExitCode::from(2);
        }
    };

    eprintln!(
        "Loaded {} transactions from {}",
        journal.transactions.len(),
        journal_path
    );
    eprintln!("Import regex:     {}", import_regex);
    eprintln!("Amount weighting: {}", config.use_amount_weighting);
    eprintln!("Strategy:         {:?}", config.strategy);
    if mode_count == 0 {
        eprintln!("Holdout fraction: {:.2} (seed={})", holdout_fraction, seed);
    }
    eprintln!();

    let all_txns: Vec<Transaction> = std::mem::take(&mut journal.transactions);

    if let Some(account) = cold_account {
        run_cold_account(all_txns, &import_re, &config, &account)
    } else if let Some(min) = loao_min {
        run_loao(all_txns, &import_re, &config, min)
    } else if let Some(account) = acct_replacement {
        run_account_replacement(
            all_txns,
            &import_re,
            &config,
            &account,
            replacement_fraction,
            seed,
        )
    } else if let Some(min) = acct_replacement_cohort_min {
        run_account_replacement_cohort(
            all_txns,
            &import_re,
            &config,
            min,
            replacement_fraction,
            seed,
        )
    } else {
        run_uniform_holdout(all_txns, &import_re, &config, holdout_fraction, seed)
    }
}
