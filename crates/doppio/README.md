# doppio

A typed compiler pipeline for [Ledger](https://ledger-cli.org/)
plain-text accounting -- built to be embedded.

This is the **library** crate. For the `dop` command-line interface,
install [`doppio-cli`](https://crates.io/crates/doppio-cli) instead;
it depends on this crate and ships the binary.

## What it does

doppio parses `.ledger` (and `.hledger`, `.journal`) source files
through four sequential stages:

```
source text -> parse -> resolution -> elaboration -> .dop
```

It enforces double-entry balance and balance-assertion semantics at
elaboration time, then exposes the result as a strongly-typed
`elaboration::Journal` (a `prost`-generated Protocol Buffers message)
ready for queries, reporting, or serialisation to the `.dop` binary
format.

## Use cases

- **Bank import pipelines.** Build `resolution::Transaction` values
  programmatically from imported records (Mercury, Plaid, SimpleFIN,
  etc.) and run them through `eval_transaction` to validate balance
  before writing back to source.
- **Reporting tools.** Compile a journal once with `compile`, then
  serialise to a `.dop` file with `write_dop` and load it in
  milliseconds with `read_dop` for repeated queries.
- **Cross-language consumers.** The `.dop` binary format is defined
  by [`proto/doppio.proto`](https://github.com/alevy/doppio/blob/main/proto/doppio.proto)
  in the source repo. Any language with a Protocol Buffers binding
  can read `.dop` files directly without re-parsing the source.

## Quick example

```rust
use doppio::resolution::{Context, Transaction, Posting};
use chrono::NaiveDate;
use rust_decimal::Decimal;

let txn = Transaction::new(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
    .with_posting(Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "$")))
    .with_posting(Posting::new("Assets:Checking"));

// Validate and elaborate -- the null posting on Assets:Checking is inferred as -$50.
let journal_txn = doppio::eval_transaction(txn, &Context::default())?;
```

## Public API surface

The intended construction layer is `resolution::Transaction` /
`resolution::Posting` (builder API). The intended read layer is
`elaboration::*` (the `prost`-generated proto types) plus their
inherent ergonomic methods (`Decimal::to_decimal()`,
`Amount::iter()`, `Posting::amount_in(commodity)`, etc.).

The `Frontend` trait pluralises the parser dispatch -- `LedgerFrontend`
and `HledgerFrontend` are shipped; new formats (e.g. Beancount) can
implement the trait without modifying the library.

## Companion crates

- **[`doppio-cli`](https://crates.io/crates/doppio-cli)** -- the `dop`
  command-line interface (compile, balance, register, print, stats,
  accounts, commodities, with `--exchange`, `--cleared`, `--depth`,
  `--format text|json|csv`, etc.).
- **[`doppio-categorize`](https://crates.io/crates/doppio-categorize)** --
  counter-account suggestions for new transactions, given an existing
  journal as the training corpus.

## Documentation

- [Repository](https://github.com/alevy/doppio) -- README, CLI
  reference, supported feature matrix.
- [`docs/`](https://github.com/alevy/doppio/tree/main/docs) -- proto
  evolution policy, exchange-rate semantics, supported features.
- [`proto/doppio.proto`](https://github.com/alevy/doppio/blob/main/proto/doppio.proto) --
  the canonical wire format with multilingual reassembly recipes.

## License

ISC. See [LICENSE](https://github.com/alevy/doppio/blob/main/LICENSE).
