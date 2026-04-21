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
    Balance {
        source: PathBuf,
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
        Commands::Balance { source } => {
            let journal = load_journal(&source)?;
            let mut balances: BTreeMap<&String, BTreeMap<&String, rust_decimal::Decimal>> =
                BTreeMap::new();

            for txn in journal.transactions.iter() {
                for posting in txn.postings.iter() {
                    for (commodity, amount) in posting.amount.0.iter() {
                        *(balances
                            .entry(&posting.account)
                            .or_default()
                            .entry(commodity)
                            .or_default()) += *amount;
                    }
                }
            }
            for (account, balances) in balances.iter() {
                let mut balances = balances.iter();
                if let Some((commodity, value)) = balances.next() {
                    let balance = format!("{} {value}", commodity);
                    println!("{balance:>20}  {}", account,);
                }
                for (commodity, value) in balances {
                    let balance = format!("{} {value}", commodity);
                    println!("{balance:>20}");
                }
            }
        }
    }
    Ok(())
}
