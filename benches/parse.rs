use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::path::PathBuf;

mod data;

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for (name, input) in data::workloads() {
        let mut parser = ledger::parser::Parser {
            openner: |_: &str| String::new(),
            base_path: PathBuf::new(),
        };
        group.bench_with_input(BenchmarkId::new("parse", name), &input, |b, input| {
            b.iter(|| parser.parse(input).unwrap())
        });
    }
    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
