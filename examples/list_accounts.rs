use std::{fs::File, io::Read};

use ledger::{elaboration::Journal, resolution::HIR};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = &std::env::args().collect::<Vec<String>>()[1];
    let mut file = String::new();
    File::open(&path)
        .unwrap()
        .read_to_string(&mut file)
        .unwrap();
    let mut file = file.as_str();
    let output = ledger::parser::parse_ledger(&mut file)?;
    let hir: HIR = output.try_into()?;
    let journal: Journal = hir.try_into()?;


    if let Some(last_txn) = journal.transactions.last() {
        for (account, balances) in last_txn.running_state.account_balances.iter() {
            let mut balances = balances.commodity.iter();
            if let Some((commodity, value)) = balances.next() {
                let balance = format!("{} {value}", commodity);
                println!(
                    "{balance:>20}  {}",
                    account,
                );
            }
            for (commodity, value) in balances {
                let balance = format!("{} {value}", commodity);
                println!("{balance:>20}");
            }
        }
    }
    Ok(())
}
