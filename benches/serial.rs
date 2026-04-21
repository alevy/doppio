use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::{io::Write as _, path::PathBuf};

mod data;

fn make_parser() -> ledger::parser::Parser<impl Fn(&str) -> String> {
    ledger::parser::Parser {
        openner: |_: &str| String::new(),
        base_path: PathBuf::new(),
    }
}

fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize");
    for (name, input) in data::workloads() {
        let journal = ledger::compile(&input, make_parser()).unwrap();
        group.bench_with_input(BenchmarkId::new("serialize", name), &journal, |b, journal| {
            b.iter(|| {
                let mut out = std::io::BufWriter::new(std::io::sink());
                postcard::to_io(journal, &mut out).unwrap();
                out.flush().unwrap();
            })
        });
    }
    group.finish();
}

fn bench_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("deserialize");
    for (name, input) in data::workloads() {
        let journal = ledger::compile(&input, make_parser()).unwrap();
        let bytes = postcard::to_allocvec(&journal).unwrap();
        group.bench_with_input(BenchmarkId::new("deserialize", name), &bytes, |b, bytes| {
            b.iter(|| postcard::from_bytes::<ledger::Journal>(bytes).unwrap())
        });
    }
    group.finish();
}

criterion_group!(benches, bench_serialize, bench_deserialize);
criterion_main!(benches);
