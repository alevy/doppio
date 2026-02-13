use std::{fs::File, io::Read};

fn main() {
    let path = &std::env::args().collect::<Vec<String>>()[1];
    let mut file = String::new();
    File::open(&path)
        .unwrap()
        .read_to_string(&mut file)
        .unwrap();
    let mut file = file.as_str();
    let mut output = ledger::JournalAst::parse(&mut file).unwrap();

    output.resolve_includes(path).unwrap();

    let journal = ledger::Journal::compile(&output).unwrap();

    println!("\"account code\",\"account name\"");
    for account in journal.accounts.values() {
        println!("\"{}\",\"{}\"", account.name, account.note.as_ref().unwrap_or(&"".into()));
    }
}
