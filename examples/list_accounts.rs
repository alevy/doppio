use std::{fs::File, io::Read};

use winnow::ModalResult;

fn main() -> ModalResult<()> {
    let path = &std::env::args().collect::<Vec<String>>()[1];
    let mut file = String::new();
    File::open(&path)
        .unwrap()
        .read_to_string(&mut file)
        .unwrap();
    let mut file = file.as_str();
    let mut output = ledger::JournalAst::parse(&mut file)?;

    output.resolve_includes(path)?;

    let journal = ledger::Journal::compile(&output).unwrap();

    println!("\"account code\",\"account name\"");
    for account in journal.accounts.values() {
        println!(
            "\"{}\",\"{}\"",
            account.name,
            account.note.as_ref().unwrap_or(&"".into())
        );
    }
    Ok(())
}
