//! Example: print a final balance table for all accounts in a journal.
//!
//! Accepts either a raw `.ledger` source file or a pre-compiled `.dop` file as
//! its sole command-line argument. Outputs one line per commodity per account,
//! formatted to match the `balance` subcommand of the `dop` CLI.
//!
//! Usage:
//!   cargo run --example list_accounts -- path/to/journal.ledger
//!   cargo run --example list_accounts -- path/to/journal.dop

use std::{collections::BTreeMap, fs::File, io::Read as _, path::PathBuf};

use rust_decimal::Decimal;

fn load_journal(path: &PathBuf) -> Result<doppio::Journal, Box<dyn std::error::Error>> {
    if let Some("dop") = path.extension().and_then(|e| e.to_str()) {
        let mut f = File::open(path)?;
        Ok(doppio::read_dop(&mut f, path)?)
    } else {
        let base_path = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let parser = doppio::parser::Parser {
            opener: doppio::file_opener,
            base_path: base_path.to_path_buf(),
        };
        let mut source = String::new();
        File::open(path)?.read_to_string(&mut source)?;
        Ok(doppio::compile(&source, parser)?)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: list_accounts <path.ledger|path.dop>");
        std::process::exit(1);
    }
    let path = PathBuf::from(&args[1]);

    let journal = load_journal(&path)?;

    // Accumulate per-account, per-commodity balances by walking every posting
    // in every transaction.  Running state is no longer stored on individual
    // transactions, so we compute it here from scratch.
    let mut balances: BTreeMap<&String, BTreeMap<&String, Decimal>> = BTreeMap::new();
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

    // Print: commodity + value right-aligned in a 20-char field, then account.
    for (account, account_balances) in balances.iter() {
        let mut iter = account_balances.iter();
        if let Some((commodity, value)) = iter.next() {
            let balance = format!("{} {value}", commodity);
            println!("{balance:>20}  {}", account);
        }
        for (commodity, value) in iter {
            let balance = format!("{} {value}", commodity);
            println!("{balance:>20}");
        }
    }

    Ok(())
}
