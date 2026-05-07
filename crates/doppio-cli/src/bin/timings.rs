//! Pipeline timing tool for per-stage performance profiling.
//!
//! Usage: `timings <source.ledger>`
//!
//! Runs each compilation stage in sequence and prints the wall-clock time
//! spent in each stage to stderr. The final stage serialises to
//! [`std::io::sink`] (discarding output) so that disk I/O does not
//! inflate the serialisation time.
//!
//! This tool is useful for identifying which stage dominates compile time
//! for a given ledger file, particularly when tuning the parser or evaluator.

use std::{fs::File, io::Read as _, path::PathBuf, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = PathBuf::from(std::env::args().nth(1).expect("usage: timings <source>"));

    // --- read ---
    // Measure raw file I/O separately so that parse time is not inflated by
    // disk latency on the first run (cold page cache).
    let t0 = Instant::now();
    let base_path = source.parent().unwrap().to_path_buf();
    let mut parser = doppio::grammars::ledger::Parser {
        opener: doppio::file_opener,
        base_path,
    };
    let mut file = String::new();
    File::open(&source)?.read_to_string(&mut file)?;
    eprintln!("read:        {:>8.3}s", t0.elapsed().as_secs_f64());

    // --- parse ---
    // PEG tokenisation and construction of the ast::Journal.
    let t1 = Instant::now();
    let ast = parser.parse(&file)?;
    eprintln!("parse:       {:>8.3}s", t1.elapsed().as_secs_f64());

    // --- resolution ---
    // Date normalisation, alias indexing, metadata extraction.
    let t2 = Instant::now();
    let hir: doppio::resolution::HIR = ast.try_into()?;
    eprintln!("resolution:  {:>8.3}s", t2.elapsed().as_secs_f64());

    // --- elaboration ---
    // Expression evaluation, transaction balancing, account registration.
    let t3 = Instant::now();
    // The parser is ledger-cli; use the matching default semantics.
    let journal: doppio::elaboration::Journal =
        doppio::elaborate(hir, &doppio::grammars::ledger::LEDGER_DEFAULTS)?;
    eprintln!("elaboration: {:>8.3}s", t3.elapsed().as_secs_f64());

    // --- serialize ---
    // Protobuf + deflate serialisation to a no-op sink.
    let t4 = Instant::now();
    let mut output = std::io::sink();
    doppio::write_dop(&journal, &mut output, doppio::Compression::Deflate)?;
    eprintln!("serialize:   {:>8.3}s", t4.elapsed().as_secs_f64());

    eprintln!("total:       {:>8.3}s", t0.elapsed().as_secs_f64());
    Ok(())
}
