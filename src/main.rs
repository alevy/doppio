use std::{collections::BTreeMap, fs::File, io::Read as _, path::PathBuf};

use clap::{Parser, Subcommand};
use rust_decimal::Decimal;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// does testing things
    Compile {
        /// lists test values
        #[arg(short, long)]
        output: PathBuf,
        source: PathBuf,
    },
    Balance {
        source: PathBuf,
    },
    Register {
        source: PathBuf,
        pattern: Option<String>,
    },
}

fn load_journal(path: &PathBuf) -> Result<ledger::Journal, Box<dyn std::error::Error>> {
    if let Some("bki") = path.extension().and_then(|e| e.to_str()) {
        let input_xz = xz::read::XzDecoder::new(File::open(path)?);
        let mut buf = vec![0; 102400];
        Ok(postcard::from_io((input_xz, &mut buf))?.0)
    } else {
        let base_path = path.parent().unwrap().to_path_buf();
        let parser = ledger::parser::Parser {
            openner: ledger::file_openner,
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
                openner: ledger::file_openner,
                base_path,
            };
            let mut file = String::new();
            File::open(source)?.read_to_string(&mut file)?;
            let journal = ledger::compile(&file, parser)?;
            let output_file = File::create(output)?;
            let mut output_xz = xz::write::XzEncoder::new(output_file, 6);
            postcard::to_io(&journal, &mut output_xz)?;
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
                            Decimal::deserialize(*posting.amount.0.get("$").unwrap())
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
                            .or_default()) += Decimal::deserialize(*amount);
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
