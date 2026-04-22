use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::{path::PathBuf, time::Duration};

mod data;

fn make_parser() -> doppio::parser::Parser<impl Fn(&str) -> String> {
    doppio::parser::Parser {
        opener: |_: &str| String::new(),
        base_path: PathBuf::new(),
    }
}

fn bench_resolution(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolution");
    group.measurement_time(Duration::from_secs(15));
    for (name, input) in data::workloads() {
        group.bench_with_input(BenchmarkId::new("resolution", name), &input, |b, input| {
            b.iter_batched(
                || make_parser().parse(input).unwrap(),
                |ast| -> doppio::resolution::HIR { ast.try_into().unwrap() },
                criterion::BatchSize::PerIteration,
            )
        });
    }
    group.finish();
}

fn bench_elaboration(c: &mut Criterion) {
    let mut group = c.benchmark_group("elaboration");
    group.measurement_time(Duration::from_secs(20));
    for (name, input) in data::workloads() {
        group.bench_with_input(BenchmarkId::new("elaboration", name), &input, |b, input| {
            b.iter_batched(
                || {
                    let ast = make_parser().parse(input).unwrap();
                    let hir: doppio::resolution::HIR = ast.try_into().unwrap();
                    hir
                },
                |hir| -> doppio::elaboration::Journal { hir.try_into().unwrap() },
                criterion::BatchSize::PerIteration,
            )
        });
    }
    group.finish();
}

fn bench_compile(c: &mut Criterion) {
    let mut group = c.benchmark_group("compile");
    group.measurement_time(Duration::from_secs(20));
    for (name, input) in data::workloads() {
        group.bench_with_input(BenchmarkId::new("compile", name), &input, |b, input| {
            b.iter(|| doppio::compile(input, make_parser()).unwrap())
        });
    }
    group.finish();
}

criterion_group!(benches, bench_resolution, bench_elaboration, bench_compile);
criterion_main!(benches);
