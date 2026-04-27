use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rust_decimal::Decimal;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

mod data;

fn make_parser()
-> doppio::parser::Parser<impl Fn(&str) -> Result<String, Box<dyn std::error::Error>>> {
    doppio::parser::Parser {
        opener: |_: &str| Ok(String::new()),
        base_path: PathBuf::new(),
    }
}

fn bench_balance(c: &mut Criterion) {
    let mut group = c.benchmark_group("balance");
    group.measurement_time(Duration::from_secs(12));
    for (name, input) in data::workloads() {
        let journal = doppio::compile(&input, make_parser()).unwrap();
        group.bench_with_input(BenchmarkId::new("balance", name), &journal, |b, journal| {
            b.iter(|| {
                let mut balances: BTreeMap<&str, BTreeMap<&str, Decimal>> = BTreeMap::new();
                for txn in journal.transactions.iter() {
                    for posting in txn.postings.iter() {
                        for (commodity, amount) in posting.amount.0.iter() {
                            *balances
                                .entry(posting.account.as_str())
                                .or_default()
                                .entry(commodity.as_str())
                                .or_default() += *amount;
                        }
                    }
                }
                balances
            })
        });
    }
    group.finish();
}

fn bench_register(c: &mut Criterion) {
    let mut group = c.benchmark_group("register");
    for (name, input) in data::workloads() {
        let journal = doppio::compile(&input, make_parser()).unwrap();
        // Filter to a pattern that matches ~20% of postings
        let pattern = "expenses";
        group.bench_with_input(
            BenchmarkId::new("register", name),
            &journal,
            |b, journal| {
                b.iter(|| {
                    journal
                        .transactions
                        .iter()
                        .flat_map(|txn| txn.postings.iter())
                        .filter(|p| p.account.to_lowercase().contains(pattern))
                        .count()
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_balance, bench_register);
criterion_main!(benches);
