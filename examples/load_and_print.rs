use std::{fs::File, io::Read};

fn main() {
    let path = &std::env::args().collect::<Vec<String>>()[1];
    let mut file = String::new();
    File::open(&path)
        .unwrap()
        .read_to_string(&mut file)
        .unwrap();
    let mut file = file.as_str();
    let output = doppio::parser::parse_ledger(&mut file).unwrap();

    println!("{:?}", output);
}
