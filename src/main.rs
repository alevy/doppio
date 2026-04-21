use std::{collections::BTreeMap, fs::File, io::{Read as _, Write as _}, path::PathBuf};

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
            for txn in journal.transactions.iter() {
                for posting in txn.postings.iter() {
                    if posting.account.to_lowercase().contains(&pattern) {
                        println!(
                            "{:<20} {:>20}",
                            posting.account,
                            posting.amount.0.get("$").unwrap()
                        );
                    }
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
