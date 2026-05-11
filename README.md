# doppio

**A typed compiler pipeline and CLI for [Ledger](https://ledger-cli.org/) plain-text accounting -- built to be embedded.**

doppio parses `.ledger` files through a four-stage compiler (parse -> resolution -> elaboration -> binary serialization), enforces double-entry balance and balance assertions, and exposes every stage as a first-class Rust library API. Use the `dop` CLI to query your journals directly, or embed the library to build importers, reporting tools, and accounting applications on a correct, type-safe foundation.

## Why doppio?

Most Ledger tooling treats the format as a parsing problem. doppio treats it as a compilation problem: source text goes in, a fully elaborated, validated journal comes out -- along with a compact binary (`.dop`) for fast repeated queries without re-parsing.

**For library users:** Construct transactions programmatically with a fluent builder API, run them through elaboration to validate balance, and serialize back to Ledger source text or the binary format. The library exposes the full pipeline at each stage (`ast`, `resolution`, `elaboration`) so you work at the right level of abstraction.

**For CLI users:** Compile once to `.dop`, then query balance sheets and posting registers in milliseconds. Accepts both raw `.ledger` files and pre-compiled `.dop` files interchangeably.

## Quick start

**CLI:**

```sh
cargo install doppio-cli
dop compile --output my-journal.dop my-journal.ledger
dop balance my-journal.dop
dop register my-journal.dop Expenses
```

The `doppio-cli` crate ships the `dop` binary; `doppio` itself is the
library.

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

### `compile` -- pre-process a journal file

```
dop compile --output my-journal.dop my-journal.ledger
```

Parses the source file, runs it through the full compilation pipeline, and writes the result as a Protocol Buffers payload, deflate-compressed by default. Use this for large journals to avoid re-parsing on every query. Pass `--no-compression` for raw protobuf.

### `balance` -- account balances

```
dop balance my-journal.ledger
dop balance my-journal.dop --depth 2 --begin 2024-01-01 --cleared
dop balance my-journal.dop --pattern "^Expenses" --format json
```

Prints account balances grouped by commodity, with a grand-total footer per commodity beneath a divider. By default, output is rendered as an indented tree where parent rows show the sum of every descendant; pass `--flat` to revert to the classic single-line-per-account form. Flags: `--depth N` (truncate hierarchy), `--flat`, `--begin`/`--end` (date range), `--cleared` (cleared transactions only), `--tag KEY` (transactions tagged with `KEY`), `--pattern REGEX` (filter accounts), `-X`/`--exchange COMMODITY` (convert balances to `COMMODITY` using `P` price directives), `--format text|json|csv`.

When stdout is a terminal, balance output is colorized: negative amounts appear in red and account names in blue, matching ledger-cli's color scheme. Pass `--color=never` to suppress color or `--color=always` to force it even when piped. The `NO_COLOR` environment variable is also honoured.

### `register` -- posting register

```
dop register my-journal.ledger
dop register my-journal.dop Expenses --format csv
```

Lists individual postings with running totals per commodity, optionally filtered to accounts matching a regex pattern. Flags: `--begin`/`--end` (date range), `--cleared`, `--tag KEY`, `--format text|json|csv`.

### `print` -- re-emit canonical Ledger source

```
dop print my-journal.ledger
```

Parses and re-emits the journal in canonical Ledger source format -- useful for normalizing formatting or verifying round-trip fidelity.

### `stats` -- journal summary

```
dop stats my-journal.ledger
```

Prints transaction count, account count, commodity count, and date range.

### `accounts` -- list account names

```
dop accounts my-journal.ledger
```

Lists all account names found in the journal.

### `commodities` -- list commodity symbols

```
dop commodities my-journal.ledger
```

Lists every commodity symbol used in the journal, sorted and deduplicated.

## Library API

The library exposes the pipeline stages as modules, plus top-level entry points:

| Function | Description |
|---|---|
| `compile(source, parser)` | Full pipeline: source text -> elaborated `Journal` |
| `eval_transaction(txn, ctx)` | Elaborate a single `resolution::Transaction` -- validate balance, infer null posting, apply aliases |
| `write_ledger(txns, writer)` | Serialize `resolution::Transaction` values to canonical Ledger source text |
| `write_dop` / `read_dop` | Round-trip an `elaboration::Journal` to/from a `.dop` file (8-byte header + optional deflate + protobuf) |

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
error if encountered -- see issue #103.

## Supported Ledger features

doppio supports the subset of ledger-cli syntax needed for typical day-to-day
plain-text accounting, including the patterns used by real downstream books.
At a glance:

| Category | Status |
|---|---|
| Transactions, postings, balance assertions/assignments | Supported |
| Directives -- `include` (incl. globs), `account`, `commodity`, `alias`, `define` (with parameters), `tag` (with `assert`/`check`), `P` historical price | Supported |
| Expressions -- arithmetic, comparisons, regex `=~`/`!~`, `tag()`, parameterised function calls | Supported |
| CLI -- `compile`, `balance`, `register`, `print`, `stats`, `accounts`, `commodities`; text / JSON / CSV output | Supported |
| Library API -- `compile`, `eval_transaction`, `write_ledger`, `.dop` binary format | Supported |
| hledger input format (`.hledger`, `.journal`) | Supported |
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
 │ serialization│  protobuf + deflate → .dop
 └──────────────┘
```

### Stage details

**Parse** (`crates/doppio/src/grammars/ledger/`, `crates/doppio/src/grammars/hledger/`): A [pest](https://pest.rs/) PEG grammar tokenizes the source into an `ast::Journal` containing transactions, directives, and comments. Amount expressions are kept as unevaluated `ValueExpr` trees. `include` directives are resolved recursively here. The `Frontend` trait dispatches on file extension between the ledger-cli and hledger grammars.

**Resolution** (`crates/doppio/src/resolution.rs`): Converts `ast::Journal` to a Higher-level Intermediate Representation (`HIR`). Dates are resolved to `NaiveDate` (a full year is required). Commodity and account aliases are accumulated into a versioned `Context` stack so each transaction sees the aliases that were in effect when it was defined. Structured metadata and tags are extracted from freeform notes.

**Elaboration** (`crates/doppio/src/elaborator.rs`): Converts `HIR` to the final `elaboration::Journal`. `ValueExpr` trees are evaluated to `(Decimal, commodity)` pairs, commodity aliases are applied, and each transaction is balanced -- if exactly one posting has no explicit amount, its value is inferred as the negation of the sum of the rest. Balance assertions (`= amount`) and balance assignments (`=amount`) are checked or applied at this stage.

**Serialization**: The `elaboration::Journal` is a [prost](https://github.com/tokio-rs/prost)-generated Protocol Buffers message defined in [`proto/doppio.proto`](./proto/doppio.proto). `write_dop` writes an 8-byte header followed by the protobuf body, deflate-compressed by default; `read_dop` is the reverse. The format is the canonical `.dop` artifact, designed as a language-agnostic API -- non-Rust consumers read it directly via the published proto schema.

## Companion crates

The workspace publishes three crates:

- **[`doppio`](./crates/doppio/)** -- the library. Parse, resolve,
  elaborate. The `Journal` type, the `Frontend` trait, the `.dop`
  format I/O.
- **[`doppio-cli`](./crates/doppio-cli/)** -- the `dop` binary. What
  you get from `cargo install doppio-cli`.
- **[`doppio-categorize`](./crates/doppio-categorize/)** --
  counter-account suggestions for new transactions, given an existing
  journal as the training corpus. Used by import flows like Mercury
  and SimpleFIN to fill in the second posting of a transaction
  automatically.

## Build from source

```
cargo build --release
```

The resulting binary is `target/release/dop`.

### Cross-frontend parity check

CI validates that the per-frontend parity fixtures produce the same
per-account balance under doppio as under each format's canonical tool
(`bean-check`/Beancount API, `hledger`, `ledger`). To reproduce locally:

```
cargo build --release -p doppio-cli
python3 scripts/parity_check.py             # positive cases
python3 scripts/parity_check.py --negative  # broken-fixture controls
```

Requires `hledger`, `ledger`, and the `beancount` Python package on
PATH (`pip install 'beancount==3.2.0'`).

## Web demos

Two browser apps live in this repo and are deployed to GitHub Pages from `main`:

- **Dashboard** ([`web/dashboard/`](./web/dashboard/), live at <https://alevy.github.io/doppio/dashboard/>) — renders balance, register, and chart views over a `.dop` file using a JS-native protobuf decoder. No Rust or WASM at runtime. Validates doppio's format-as-API claim: any non-Rust language can read `.dop` files via the published [`proto/doppio.proto`](./crates/doppio/proto/doppio.proto) schema.
- **Compile** ([`web/compile/`](./web/compile/), live at <https://alevy.github.io/doppio/compile/>) — paste ledger / hledger / Beancount source and compile it to a `.dop` byte-stream in the browser via the [`doppio-wasm`](./crates/doppio-wasm/) shim. No server round-trip.

A small landing page at <https://alevy.github.io/doppio/> links to both.

## Changelog

See [CHANGELOG.md](./CHANGELOG.md) for release notes.
