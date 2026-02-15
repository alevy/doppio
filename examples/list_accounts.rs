use std::{fs::File, io::Read};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = &std::env::args().collect::<Vec<String>>()[1];
    let mut file = String::new();
    File::open(&path)
        .unwrap()
        .read_to_string(&mut file)
        .unwrap();
    let mut file = file.as_str();
    let output = ledger::parser::parse_ledger(&mut file)?;

    let journal = ledger::Journal::compile(&output).unwrap();

    for account in journal.accounts.values() {
        let mut balances = account.balances.iter();
        if let Some((commodity, value)) = balances.next() {
            let balance = format!("{} {value}", commodity.clone().unwrap());
            println!(
                "{balance:>20}  {}",
                account.name,
            );
        }
        for (commodity, value) in balances {
            let balance = format!("{} {value}", commodity.clone().unwrap());
            println!("{balance:>20}");
        }
    }
    Ok(())
}
