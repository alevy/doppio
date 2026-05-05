use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::{path::PathBuf, time::Duration};

mod data;

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    group.measurement_time(Duration::from_secs(15));
    for (name, input) in data::workloads() {
        let mut parser = doppio::grammars::ledger::Parser {
            opener: |_: &str| Ok(String::new()),
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
