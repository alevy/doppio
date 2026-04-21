use std::{collections::{BTreeMap, BTreeSet}, fs::File, io::{Read as _, Write as _}, path::PathBuf};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse and compile a ledger source file into a binary `.bki` archive.
    ///
    /// The output is a postcard-serialised, XZ-compressed snapshot of the
    /// elaborated journal. Loading a `.bki` file is much faster than
    /// re-parsing the source, making it suitable for large ledgers that are
    /// queried repeatedly.
    Compile {
        /// Path for the output `.bki` file.
        #[arg(short, long)]
        output: PathBuf,
        /// Path to the root `.ledger` source file (may use `include`).
        source: PathBuf,
    },

    /// Print the running balance for every account.
    ///
    /// Accepts either a raw `.ledger` source file or a pre-compiled `.bki`
    /// file. Output is formatted with the commodity and value right-aligned,
    /// followed by the account name.
    ///
    /// By default, output is rendered in tree form with indentation. Pass
    /// `--flat` to revert to the classic single-line-per-account format.
    Balance {
        source: PathBuf,
        /// Include only transactions on or after this date (YYYY-MM-DD).
        #[arg(long)]
        begin: Option<String>,
        /// Include only transactions on or before this date (YYYY-MM-DD).
        #[arg(long)]
        end: Option<String>,
        /// Include only cleared transactions.
        #[arg(long)]
        cleared: bool,
        /// Collapse accounts deeper than N colon-separated levels into their parent.
        #[arg(long)]
        depth: Option<usize>,
        /// Print flat output (full account names, no indentation) instead of the
        /// default tree view.
        #[arg(long, default_value_t = false)]
        flat: bool,
    },

    /// List individual postings, optionally filtered by account name.
    ///
    /// `PATTERN` is matched case-insensitively as a substring of the account
    /// name. Omit it to list all postings.
    Register {
        source: PathBuf,
        pattern: Option<String>,
    },

    /// Re-emit the journal as canonical Ledger source text.
    ///
    /// Parses and resolves the source file, then prints each transaction in
    /// canonical Ledger format. Only `.ledger` source files are accepted;
    /// pre-compiled `.bki` files do not preserve the original transaction
    /// structure needed for faithful re-emission.
    Print {
        /// Path to the root `.ledger` source file.
        source: PathBuf,
    },

    /// List all accounts that appear in the journal, one per line.
    ///
    /// Output is sorted alphabetically. Pass `PATTERN` to restrict the list
    /// to accounts whose name contains the pattern (case-insensitive).
    Accounts {
        source: PathBuf,
        /// Optional case-insensitive substring filter on account names.
        pattern: Option<String>,
    },

    /// List all commodity symbols used in the journal, one per line.
    ///
    /// Output is sorted and deduplicated.
    Commodities {
        source: PathBuf,
    },

    /// Print a summary of the journal: transaction count, unique accounts,
    /// unique commodities, and the date range covered.
    Stats {
        source: PathBuf,
    },
}

/// Load a [`ledger::Journal`] from either a compiled `.bki` file or a raw
/// `.ledger` source file.
///
/// The file type is detected by extension:
/// - `.bki` — decompress with XZ and deserialise with postcard.
/// - anything else — parse as Ledger source text, resolving `include`
///   directives relative to the file's parent directory.
fn load_journal(path: &PathBuf) -> Result<ledger::Journal, Box<dyn std::error::Error>> {
    if let Some("bki") = path.extension().and_then(|e| e.to_str()) {
        // Pre-compiled binary format: decompress then deserialise.
        // The 100 KiB scratch buffer is required by postcard's `from_io` API;
        // it does not limit the total data read.
        let input_xz = xz::read::XzDecoder::new(File::open(path)?);
        let mut buf = vec![0; 102400];
        Ok(postcard::from_io((input_xz, &mut buf))?.0)
    } else {
        let base_path = path.parent().unwrap().to_path_buf();
        let parser = ledger::parser::Parser {
            opener: ledger::file_opener,
            base_path,
        };
        let mut file = String::new();
        File::open(path)?.read_to_string(&mut file)?;
        Ok(ledger::compile(&file, parser)?)
    }
}

/// Truncate an account name to at most `depth` colon-separated components.
///
/// Returns a subslice of `account` ending at the position of the `depth`-th
/// colon, or the full string if it has fewer than `depth` components.
///
/// # Examples
///
/// ```
/// assert_eq!(truncate_account("Expenses:Food:Restaurants", 2), "Expenses:Food");
/// assert_eq!(truncate_account("Assets:Checking", 1), "Assets");
/// assert_eq!(truncate_account("Assets", 1), "Assets");
/// ```
fn truncate_account(account: &str, depth: usize) -> &str {
    let mut colon_pos = None;
    let mut count = 0;
    for (i, c) in account.char_indices() {
        if c == ':' {
            count += 1;
            if count == depth {
                colon_pos = Some(i);
                break;
            }
        }
    }
    match colon_pos {
        Some(pos) => &account[..pos],
        None => account,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compile { output, source } => {
            let base_path = source.parent().unwrap().to_path_buf();
            let parser = ledger::parser::Parser {
                opener: ledger::file_opener,
                base_path,
            };
            let mut file = String::new();
            File::open(source)?.read_to_string(&mut file)?;
            let journal = ledger::compile(&file, parser)?;
            let mut output_xz = xz::write::XzEncoder::new(File::create(output)?, 6);
            {
                let mut buf = std::io::BufWriter::new(&mut output_xz);
                postcard::to_io(&journal, &mut buf)?;
                buf.flush()?;
            }
            output_xz.finish()?;
        }
        Commands::Register { source, pattern } => {
            let pattern = pattern.unwrap_or_default().to_lowercase();
            let journal = load_journal(&source)?;
            // Per-commodity running total across all matching postings.
            let mut running: BTreeMap<String, rust_decimal::Decimal> = BTreeMap::new();

            for txn in journal.transactions.iter() {
                // txn.date is Unix epoch days (1970-01-01 = 0); convert back to a
                // human-readable date string for display.
                let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                    .and_then(|epoch| {
                        epoch.checked_add_signed(chrono::Duration::days(txn.date as i64))
                    })
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "????-??-??".to_string());

                for posting in txn.postings.iter() {
                    if !posting.account.to_lowercase().contains(&pattern) {
                        continue;
                    }

                    // Accumulate every commodity in this posting into the running total.
                    for (commodity, amount) in posting.amount.0.iter() {
                        *running.entry(commodity.clone()).or_default() += amount;
                    }

                    // Print one output line per commodity in the posting.
                    // The first line carries date, description, and account;
                    // subsequent commodity lines are blank in those columns.
                    let mut commodities = posting.amount.0.iter();
                    if let Some((commodity, amount)) = commodities.next() {
                        let amount_str = format!("{} {}", commodity, amount);
                        let running_str = format!(
                            "{} {}",
                            commodity,
                            running.get(commodity).copied().unwrap_or_default()
                        );
                        println!(
                            "{:<10}  {:<20}  {:<30}  {:>15}  {:>15}",
                            date,
                            txn.description.chars().take(20).collect::<String>(),
                            posting.account,
                            amount_str,
                            running_str,
                        );
                    }
                    for (commodity, amount) in commodities {
                        let amount_str = format!("{} {}", commodity, amount);
                        let running_str = format!(
                            "{} {}",
                            commodity,
                            running.get(commodity).copied().unwrap_or_default()
                        );
                        println!(
                            "{:<10}  {:<20}  {:<30}  {:>15}  {:>15}",
                            "", "", "", amount_str, running_str,
                        );
                    }
                }
            }
        }
        Commands::Print { source } => {
            if let Some("bki") = source.extension().and_then(|e| e.to_str()) {
                return Err(
                    "print only works with .ledger source files; \
                     .bki binary archives do not preserve the original transaction structure"
                        .into(),
                );
            }
            let base_path = source.parent().unwrap().to_path_buf();
            let mut parser = ledger::parser::Parser {
                opener: ledger::file_opener,
                base_path,
            };
            let mut file = String::new();
            File::open(&source)?.read_to_string(&mut file)?;
            let ast_journal: ledger::ast::Journal = parser.parse(&file)?;
            let hir: ledger::resolution::HIR = ast_journal.try_into()?;
            for entry in hir.entries {
                if let ledger::resolution::Entry::Transaction(txn) = entry.data {
                    println!("{txn}");
                }
            }
        }
        Commands::Accounts { source, pattern } => {
            let journal = load_journal(&source)?;
            let pattern = pattern.map(|p| p.to_lowercase()).unwrap_or_default();
            for account in journal.accounts.keys() {
                if account.to_lowercase().contains(&pattern) {
                    println!("{}", account);
                }
            }
        }
        Commands::Commodities { source } => {
            let journal = load_journal(&source)?;
            let commodities: BTreeSet<&String> = journal
                .transactions
                .iter()
                .flat_map(|txn| txn.postings.iter())
                .flat_map(|posting| posting.amount.0.keys())
                .collect();
            for commodity in commodities {
                println!("{}", commodity);
            }
        }
        Commands::Stats { source } => {
            let journal = load_journal(&source)?;

            let commodities: BTreeSet<&String> = journal
                .transactions
                .iter()
                .flat_map(|txn| txn.postings.iter())
                .flat_map(|posting| posting.amount.0.keys())
                .collect();

            // txn.date is Unix epoch days (1970-01-01 = 0).
            let unix_epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
            let first_date = journal
                .transactions
                .first()
                .and_then(|txn| {
                    unix_epoch.checked_add_signed(chrono::Duration::days(txn.date as i64))
                });
            let last_date = journal
                .transactions
                .last()
                .and_then(|txn| {
                    unix_epoch.checked_add_signed(chrono::Duration::days(txn.date as i64))
                });

            println!("Transactions: {}", journal.transactions.len());
            println!("Accounts:     {}", journal.accounts.len());
            println!("Commodities:  {}", commodities.len());
            match (first_date, last_date) {
                (Some(first), Some(last)) => {
                    println!("First date:   {}", first);
                    println!("Last date:    {}", last);
                }
                _ => {
                    println!("First date:   N/A");
                    println!("Last date:    N/A");
                }
            }
        }
        Commands::Balance {
            source,
            begin,
            end,
            cleared,
            depth,
            flat,
        } => {
            let journal = load_journal(&source)?;

            let unix_epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();

            let begin_date = begin
                .as_deref()
                .map(|s| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                        format!(
                            "invalid --begin date '{}': expected format YYYY-MM-DD",
                            s
                        )
                    })
                })
                .transpose()?;

            let end_date = end
                .as_deref()
                .map(|s| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                        format!(
                            "invalid --end date '{}': expected format YYYY-MM-DD",
                            s
                        )
                    })
                })
                .transpose()?;

            // Balances keyed by owned account name so depth-truncation can
            // produce new strings that aren't borrowed from the journal.
            let mut balances: BTreeMap<String, BTreeMap<String, rust_decimal::Decimal>> =
                BTreeMap::new();

            for txn in journal.transactions.iter() {
                if cleared {
                    if !matches!(txn.state, ledger::elaboration::TransactionState::Cleared) {
                        continue;
                    }
                }

                if begin_date.is_some() || end_date.is_some() {
                    let txn_date = unix_epoch
                        .checked_add_signed(chrono::Duration::days(txn.date as i64));
                    if let Some(txn_date) = txn_date {
                        if let Some(begin) = begin_date {
                            if txn_date < begin {
                                continue;
                            }
                        }
                        if let Some(end) = end_date {
                            if txn_date > end {
                                continue;
                            }
                        }
                    }
                }

                for posting in txn.postings.iter() {
                    let account = match depth {
                        Some(d) => truncate_account(&posting.account, d).to_owned(),
                        None => posting.account.clone(),
                    };
                    for (commodity, amount) in posting.amount.0.iter() {
                        *(balances
                            .entry(account.clone())
                            .or_default()
                            .entry(commodity.clone())
                            .or_default()) += *amount;
                    }
                }
            }

            for (account, commodities) in balances.iter() {
                let indent_depth = account.chars().filter(|&c| c == ':').count();
                let label: &str = if flat || indent_depth == 0 {
                    account.as_str()
                } else {
                    // Show only the last component in tree mode.
                    account.rsplit_once(':').map(|(_, last)| last).unwrap_or(account.as_str())
                };
                let indent = if flat { 0 } else { indent_depth * 2 };
                let prefix = " ".repeat(indent);

                let mut commodities = commodities.iter();
                if let Some((commodity, value)) = commodities.next() {
                    let balance = format!("{} {value}", commodity);
                    println!("{balance:>20}  {prefix}{label}");
                }
                for (commodity, value) in commodities {
                    let balance = format!("{} {value}", commodity);
                    println!("{balance:>20}");
                }
            }
        }
    }
    Ok(())
}
