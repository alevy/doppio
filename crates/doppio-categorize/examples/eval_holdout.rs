//! Held-out evaluation harness for doppio-categorize v0.1.
//!
//! Holds out a fraction of 2-posting transactions (whose import-side posting
//! matches a regex), builds the index from the rest, then queries each
//! held-out transaction and reports top-1 / top-3 accuracy.
//!
//! Usage:
//!
//! ```sh
//! cargo run --release --example eval_holdout -- \
//!   <journal-path> \
//!   [--import-regex REGEX] \
//!   [--no-amount-weighting] \
//!   [--holdout 0.10] \
//!   [--seed 42]
//! ```
//!
//! The journal can be a `.ledger`, `.hledger`, `.journal`, or `.dop` file.

use std::collections::{BTreeMap, HashSet};
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
        Ok(hir.try_into()?)
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
    /// Number of training samples in the queried bucket. 0 = cold-start.
    cluster_size: usize,
    /// Confidence of the rank-1 suggestion, only set when rank == Some(1).
    confidence_at_top1: Option<f64>,
}

fn print_usage() {
    eprintln!(
        "usage: eval_holdout <journal-path> [--import-regex REGEX] [--no-amount-weighting] \
         [--strategy exact|token-idf|hybrid] [--df-threshold N] \
         [--holdout FRACTION] [--seed N]"
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
    let mut seed = DEFAULT_SEED;
    let mut strategy_name = String::from("hybrid");
    let mut df_threshold: u32 = 50;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--import-regex" => {
                import_regex = match args.next() {
                    Some(v) => v,
                    None => {
                        eprintln!("--import-regex requires a value");
                        return ExitCode::from(2);
                    }
                };
            }
            "--no-amount-weighting" => {
                config.use_amount_weighting = false;
            }
            "--strategy" => {
                strategy_name = match args.next() {
                    Some(v) => v,
                    None => {
                        eprintln!("--strategy requires a value (exact|token-idf|hybrid)");
                        return ExitCode::from(2);
                    }
                };
            }
            "--df-threshold" => {
                df_threshold = match args.next().and_then(|v| v.parse().ok()) {
                    Some(n) => n,
                    None => {
                        eprintln!("--df-threshold requires a u32");
                        return ExitCode::from(2);
                    }
                };
            }
            "--holdout" => {
                holdout_fraction = match args.next().and_then(|v| v.parse().ok()) {
                    Some(f) if (0.0..1.0).contains(&f) => f,
                    _ => {
                        eprintln!("--holdout requires a fraction in [0, 1)");
                        return ExitCode::from(2);
                    }
                };
            }
            "--seed" => {
                seed = match args.next().and_then(|v| v.parse().ok()) {
                    Some(s) => s,
                    None => {
                        eprintln!("--seed requires a u64");
                        return ExitCode::from(2);
                    }
                };
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_usage();
                return ExitCode::from(2);
            }
        }
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
    eprintln!("Import regex:    {}", import_regex);
    eprintln!("Holdout fraction: {:.2} (seed={})", holdout_fraction, seed);
    eprintln!("Amount weighting: {}", config.use_amount_weighting);
    eprintln!("Strategy:         {:?}", config.strategy);
    eprintln!();

    // ---- partition into train / test -----------------------------------
    let all_txns: Vec<Transaction> = std::mem::take(&mut journal.transactions);

    // Eligible: 2 postings, exactly one of which matches the import regex.
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

    let training_journal = Journal {
        transactions: train_txns,
        accounts: BTreeMap::new(),
        commodities: BTreeMap::new(),
        prices: Vec::new(),
    };

    // ---- build index ---------------------------------------------------
    let index = Index::build(&training_journal, DefaultNormalizer);
    eprintln!(
        "Index built. Evaluating {} held-out queries...\n",
        test_txns.len()
    );

    // ---- evaluate ------------------------------------------------------
    let mut hits: Vec<Hit> = Vec::new();
    for txn in &test_txns {
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

        let cluster_size = index.bucket_size(&query.payee, &query.known_account);
        let suggestions = index.suggest(&query, &config);

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

    if hits.is_empty() {
        eprintln!("No queries were evaluable (all held-out transactions had zero amounts).");
        return ExitCode::from(1);
    }

    // ---- report --------------------------------------------------------
    let total = hits.len();
    let top1 = hits.iter().filter(|h| h.rank == Some(1)).count();
    let top3 = hits
        .iter()
        .filter(|h| matches!(h.rank, Some(r) if r <= 3))
        .count();
    let cold_start = hits.iter().filter(|h| h.cluster_size == 0).count();
    let avg_conf_top1: f64 = {
        let confs: Vec<f64> = hits.iter().filter_map(|h| h.confidence_at_top1).collect();
        if confs.is_empty() {
            0.0
        } else {
            confs.iter().sum::<f64>() / confs.len() as f64
        }
    };

    println!("=== Results ===");
    println!("Total queries:               {}", total);
    println!(
        "Top-1 accuracy:              {} / {} = {:.1}%",
        top1,
        total,
        100.0 * top1 as f64 / total as f64
    );
    println!(
        "Top-3 accuracy:              {} / {} = {:.1}%",
        top3,
        total,
        100.0 * top3 as f64 / total as f64
    );
    println!(
        "Cold-start (bucket empty):   {} / {} = {:.1}%",
        cold_start,
        total,
        100.0 * cold_start as f64 / total as f64
    );
    println!("Avg confidence on top-1 hits: {:.3}", avg_conf_top1);
    println!();

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

    let acc = top1 as f64 / total as f64;
    println!();
    if acc >= 0.70 {
        println!(
            "Top-1 accuracy {:.1}% meets the v0.1 ship criterion (>=70%).",
            acc * 100.0
        );
    } else {
        println!(
            "Top-1 accuracy {:.1}% is BELOW the v0.1 ship criterion (>=70%).",
            acc * 100.0
        );
    }

    ExitCode::from(0)
}
