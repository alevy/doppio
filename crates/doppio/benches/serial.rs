use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::PathBuf;

mod data;

fn make_parser()
-> doppio::grammars::ledger::Parser<impl Fn(&str) -> Result<String, Box<dyn std::error::Error>>> {
    doppio::grammars::ledger::Parser {
        opener: |_: &str| Ok(String::new()),
        base_path: PathBuf::new(),
    }
}

fn bench_write_dop(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize");
    for (name, input) in data::workloads() {
        let journal = doppio::compile(&input, make_parser()).unwrap();
        group.bench_with_input(
            BenchmarkId::new("write_dop", name),
            &journal,
            |b, journal| {
                b.iter(|| {
                    let mut out = std::io::sink();
                    doppio::write_dop(journal, &mut out, doppio::Compression::Deflate).unwrap();
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_write_dop);
criterion_main!(benches);
