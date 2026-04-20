use std::{fs::File, io::{Read as _, Write as _}, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = PathBuf::from(std::env::args().nth(1).expect("usage: timings <source>"));

    let t0 = Instant::now();
    let base_path = source.parent().unwrap().to_path_buf();
    let mut parser = ledger::parser::Parser {
        openner: ledger::file_openner,
        base_path,
    };
    let mut file = String::new();
    File::open(&source)?.read_to_string(&mut file)?;
    eprintln!("read:        {:>8.3}s", t0.elapsed().as_secs_f64());

    let t1 = Instant::now();
    let ast = parser.parse(&file)?;
    eprintln!("parse:       {:>8.3}s", t1.elapsed().as_secs_f64());

    let t2 = Instant::now();
    let hir: ledger::resolution::HIR = ast.try_into()?;
    eprintln!("resolution:  {:>8.3}s", t2.elapsed().as_secs_f64());

    let t3 = Instant::now();
    let journal: ledger::elaboration::Journal = hir.try_into()?;
    eprintln!("elaboration: {:>8.3}s", t3.elapsed().as_secs_f64());

    let t4 = Instant::now();
    let mut output = std::io::BufWriter::new(std::io::sink());
    postcard::to_io(&journal, &mut output)?;
    output.flush()?;
    eprintln!("serialize:   {:>8.3}s", t4.elapsed().as_secs_f64());

    eprintln!("total:       {:>8.3}s", t0.elapsed().as_secs_f64());
    Ok(())
}
