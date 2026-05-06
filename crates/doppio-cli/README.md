# doppio-cli

The `dop` command-line interface for the
[doppio](https://crates.io/crates/doppio) Ledger compiler.

## Install

```sh
cargo install doppio-cli
```

This installs the `dop` binary on your PATH. The library that powers
it is [`doppio`](https://crates.io/crates/doppio); install that
directly if you want to embed the pipeline in your own tool.

## Quick start

```sh
# Compile a journal once for fast queries.
dop compile --output my.dop my.ledger

# Query balances and registers.
dop balance my.dop
dop balance my.dop --depth 2 --begin 2024-01-01 --cleared
dop register my.dop Expenses --format json

# Re-emit canonical Ledger source (formatting / round-trip check).
dop print my.ledger

# Summary, account list, commodity list.
dop stats my.ledger
dop accounts my.ledger
dop commodities my.ledger
```

`dop` accepts both raw `.ledger` (or `.hledger`, `.journal`) source
files and pre-compiled `.dop` files interchangeably for the read-only
commands.

## Useful flags

- `--begin <YYYY-MM-DD>`, `--end <YYYY-MM-DD>` -- date-range filter
  on `balance` and `register`.
- `--cleared` -- only cleared transactions.
- `--tag <name>` -- only transactions tagged with `name`.
- `--depth <N>` (balance) -- collapse accounts deeper than N
  colon-separated levels.
- `--flat` (balance) -- single line per account instead of the
  default tree view.
- `--format text|json|csv` -- structured output.
- `-R` / `--real` -- exclude virtual postings (`(...)` and `[...]`).
- `-X` / `--exchange <COMMODITY>` -- convert balances to a target
  commodity using `P` directive prices.

## Documentation

- [Repository](https://github.com/alevy/doppio) -- full CLI
  reference, library API, supported feature matrix.
- [`docs/SUPPORTED_FEATURES.md`](https://github.com/alevy/doppio/blob/main/docs/SUPPORTED_FEATURES.md) --
  what ledger-cli / hledger features are implemented.
- [Web demo](https://alevy.github.io/doppio/) -- a browser dashboard
  that reads a `.dop` file via a JS-native protobuf decoder.

## License

ISC. See [LICENSE](https://github.com/alevy/doppio/blob/main/LICENSE).
