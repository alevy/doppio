# doppio

**A typed compiler pipeline and CLI for [Ledger](https://ledger-cli.org/) plain-text accounting — built to be embedded.**

doppio parses `.ledger` files through a four-stage compiler (parse → resolution → elaboration → binary serialization), enforces double-entry balance and balance assertions, and exposes every stage as a first-class Rust library API. Use the `dop` CLI to query your journals directly, or embed the library to build importers, reporting tools, and accounting applications on a correct, type-safe foundation.

## Why doppio?

Most Ledger tooling treats the format as a parsing problem. doppio treats it as a compilation problem: source text goes in, a fully elaborated, validated journal comes out — along with a compact binary (`.dop`) for fast repeated queries without re-parsing.

**For library users:** Construct transactions programmatically with a fluent builder API, run them through elaboration to validate balance, and serialize back to Ledger source text or the binary format. The library exposes the full pipeline at each stage (`ast`, `resolution`, `elaboration`) so you work at the right level of abstraction.

**For CLI users:** Compile once to `.dop`, then query balance sheets and posting registers in milliseconds. Accepts both raw `.ledger` files and pre-compiled `.dop` files interchangeably.

## Quick start

**CLI:**

```sh
cargo install doppio
dop compile --output my-journal.dop my-journal.ledger
dop balance my-journal.dop
dop register my-journal.dop Expenses
```

**Library:**

```rust
use doppio::resolution::{Context, Transaction, Posting};
use chrono::NaiveDate;
use rust_decimal::Decimal;

// Build a transaction programmatically
let txn = Transaction::new(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
    .with_posting(Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "$")))
    .with_posting(Posting::new("Assets:Checking"));

// Validate and elaborate it (balance is checked, null posting inferred)
let resolved = doppio::eval_transaction(txn, &Context::default())?;

// Or compile a full journal from source. The opener returns
// Result<String, Box<dyn Error>> so it can surface I/O failures.
let journal = doppio::compile(&source_text, doppio::grammars::ledger::Parser {
    opener: doppio::file_opener, // built-in glob-aware opener
    base_path: std::path::PathBuf::from("."),
})?;

for txn in &journal.transactions {
    println!("{}: {}", txn.date_naive(), txn.description);
}
```

## CLI reference

### `compile` — pre-process a journal file

```
dop compile --output my-journal.dop my-journal.ledger
```

Parses the source file, runs it through the full compilation pipeline, and writes the result as a postcard-serialized, XZ-compressed `.dop` file. Use this for large journals to avoid re-parsing on every query.

### `balance` — account balances

```
dop balance my-journal.ledger
dop balance my-journal.dop --depth 2 --begin 2024-01-01 --cleared
dop balance my-journal.dop --pattern "^Expenses" --format json
```

Prints account balances grouped by commodity. Flags: `--depth N` (truncate hierarchy), `--flat` (single-line output), `--begin`/`--end` (date range), `--cleared` (cleared transactions only), `--tag KEY` (transactions tagged with `KEY`), `--pattern REGEX` (filter accounts), `--format text|json|csv`.

### `register` — posting register

```
dop register my-journal.ledger
dop register my-journal.dop Expenses --format csv
```

Lists individual postings with running totals per commodity, optionally filtered to accounts matching a regex pattern. Flags: `--begin`/`--end` (date range), `--cleared`, `--tag KEY`, `--format text|json|csv`.

### `print` — re-emit canonical Ledger source

```
dop print my-journal.ledger
```

Parses and re-emits the journal in canonical Ledger source format — useful for normalizing formatting or verifying round-trip fidelity.

### `stats` — journal summary

```
dop stats my-journal.ledger
```

Prints transaction count, account count, commodity count, and date range.

### `accounts` — list account names

```
dop accounts my-journal.ledger
```

Lists all account names found in the journal.

## Library API

The library exposes four modules corresponding to the pipeline stages, plus top-level entry points:

| Function | Description |
|---|---|
| `compile(source, parser)` | Full pipeline: source text → elaborated `Journal` |
| `eval_transaction(txn, ctx)` | Elaborate a single `resolution::Transaction` — validate balance, infer null posting, apply aliases |
| `write_ledger(txns, writer)` | Serialize `resolution::Transaction` values to canonical Ledger source text |
| `dop_write_header` / `dop_read_header` | Portable `.dop` header I/O with clear version-mismatch errors |

The `resolution::Transaction` and `resolution::Posting` builder APIs are the intended construction layer for programmatic use:

```rust
doppio::resolution::Transaction::new(date, "Payee")
    .with_state(doppio::ast::TransactionState::Cleared)
    .with_metadata("import_id", &bank_transaction_id)
    .with_posting(
        doppio::resolution::Posting::new("Assets:Checking")
            .with_amount((amount, "USD"))
    )
    .with_posting(doppio::resolution::Posting::new("Expenses:Food"))
```

Full API documentation:

```
cargo doc --no-deps --open
```

## Supported input formats

doppio recognises two input formats by file extension:

| Extension | Format | Frontend |
|---|---|---|
| `.ledger` | [ledger-cli](https://ledger-cli.org/) | `LedgerFrontend` |
| `.hledger` | [hledger](https://hledger.org/) | `HledgerFrontend` |
| `.journal` | hledger (alternative extension) | `HledgerFrontend` |

The hledger frontend parses the same core constructs as the ledger-cli frontend
(transactions, postings, balance assertions/assignments, lot pricing, historical
prices, account/commodity directives, include) and adds hledger-specific
extensions (`/` and `.` date separators, `#` comment lines). Automated posting
rule arithmetic bodies (`*N` multipliers) are stubbed out and produce a parse
error if encountered — see issue #103.

## Supported Ledger features

doppio supports the subset of ledger-cli syntax needed for typical day-to-day
plain-text accounting, including the patterns used by real downstream books.
At a glance:

| Category | Status |
|---|---|
| Transactions, postings, balance assertions/assignments | Supported |
| Directives — `include` (incl. globs), `account`, `commodity`, `alias`, `define` (with parameters), `tag` (with `assert`/`check`), `P` historical price | Supported |
| Expressions — arithmetic, comparisons, regex `=~`/`!~`, `tag()`, parameterised function calls | Supported |
| CLI — `compile`, `balance`, `register`, `print`, `stats`, `accounts`, `commodities`; text / JSON / CSV output | Supported |
| Library API — `compile`, `eval_transaction`, `write_ledger`, `.dop` binary format | Supported |
| hledger input format (`.hledger`, `.journal`) | Supported (v0.3.0, issue #103) |
| Budgets (`~`), automated transactions (`= payee expr`), Lisp-style scripting | Not supported |

See [`docs/SUPPORTED_FEATURES.md`](./docs/SUPPORTED_FEATURES.md) for the full
matrix with notes on partial support and known limitations.

## Pipeline

doppio processes source text through four sequential stages:

```
 .ledger text
      │
      ▼
 ┌─────────┐
 │  parse  │  pest PEG grammar → ast::Journal
 └─────────┘
      │  unresolved dates, aliases, raw ValueExpr amounts
      ▼
 ┌────────────┐
 │ resolution │  ast::Journal → resolution::HIR
 └────────────┘
      │  dates normalized, aliases indexed, notes → tags/metadata
      ▼
 ┌─────────────┐
 │ elaboration │  resolution::HIR → elaboration::Journal
 └─────────────┘
      │  amounts evaluated, transactions balanced, accounts registered
      ▼
 ┌──────────────┐
 │ serialization│  postcard + XZ → .dop
 └──────────────┘
```

### Stage details

**Parse** (`crates/doppio/src/parser.rs`, `crates/doppio/src/ledger.pest`): A [pest](https://pest.rs/) PEG grammar tokenizes the source into an `ast::Journal` containing transactions, directives, and comments. Amount expressions are kept as unevaluated `ValueExpr` trees. `include` directives are resolved recursively here.

**Resolution** (`crates/doppio/src/resolution.rs`): Converts `ast::Journal` to a Higher-level Intermediate Representation (`HIR`). Dates are resolved to `NaiveDate` (a full year is required). Commodity and account aliases are accumulated into a versioned `Context` stack so each transaction sees the aliases that were in effect when it was defined. Structured metadata and tags are extracted from freeform notes.

**Elaboration** (`crates/doppio/src/elaboration.rs`): Converts `HIR` to the final `elaboration::Journal`. `ValueExpr` trees are evaluated to `(Decimal, commodity)` pairs, commodity aliases are applied, and each transaction is balanced — if exactly one posting has no explicit amount, its value is inferred as the negation of the sum of the rest. Balance assertions (`= amount`) and balance assignments (`=amount`) are checked or applied at this stage.

**Serialization**: The `Journal` implements `serde::Serialize`/`Deserialize`. The `compile` command writes it through [postcard](https://github.com/jamesmunns/postcard) into an XZ-compressed stream; the `balance` and `register` commands decompress and deserialize it in the reverse direction.

## Build from source

```
cargo build --release
```

The resulting binary is `target/release/dop`.

## Web demo

A small browser app under [`web/`](./web/) renders balance, register, and chart views over a `.dop` file using a JS-native protobuf decoder — no Rust or WASM at runtime. It serves as the working validator of doppio's format-as-API claim: any non-Rust language can read `.dop` files via the published [`proto/doppio.proto`](./proto/doppio.proto) schema.

Live preview: <https://alevy.github.io/doppio/> (deployed automatically from `main`).

## Changelog

See [CHANGELOG.md](./CHANGELOG.md) for release notes.

## Upgrading from 0.x

If you have downstream code on the `v0.4.0-rc.1` tag or on the published v0.2.0, [`docs/MIGRATION.md`](./docs/MIGRATION.md) walks through every breaking change between then and 1.0 — wire-format migration, API surface audit, and the now-private items behind their public wrappers. Most call sites only need the path renames in §1.3 and the `Frontend::parse` return-type drop-the-`try_into` change in §1.4.
