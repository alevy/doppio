# doppio

A compiler and query tool for the [Ledger](https://ledger-cli.org/) plain-text accounting format. It parses `.ledger` files through a multi-stage pipeline, producing a compact binary format (`.dop`) suitable for fast repeated querying, and provides simple balance and register views.

## Build

```
cargo build --release
```

The resulting binary is `target/release/dop`.

## Usage

### `compile` — pre-process a journal file

```
dop compile --output my-journal.dop my-journal.ledger
```

Parses the source file, runs it through the full compilation pipeline, and writes the result as a postcard-serialized, XZ-compressed `.dop` file. Use this for large journals to avoid re-parsing on every query.

### `balance` — account balances

```
dop balance my-journal.ledger
dop balance my-journal.dop
```

Prints the running balance for every account across all transactions, grouped by commodity. Both raw `.ledger` files and pre-compiled `.dop` files are accepted.

### `register` — posting register

```
dop register my-journal.ledger [PATTERN]
```

Lists individual postings, optionally filtered to accounts whose name contains `PATTERN` (case-insensitive). Useful for inspecting the history of a specific account or category.

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
 ┌─────────────┐
 │ serialization│  postcard + XZ → .dop
 └─────────────┘
```

### Stage details

**Parse** (`src/parser.rs`, `src/ledger.pest`): A [pest](https://pest.rs/) PEG grammar tokenises the source into an `ast::Journal` containing transactions, directives, and comments. Amount expressions are kept as unevaluated `ValueExpr` trees. `include` directives are resolved recursively here.

**Resolution** (`src/resolution.rs`): Converts `ast::Journal` to a Higher-level Intermediate Representation (`HIR`). Dates are resolved to `NaiveDate` (a full year is required). Commodity and account aliases are accumulated into a versioned `Context` stack so each transaction sees the aliases that were in effect when it was defined. Structured metadata and tags are extracted from freeform notes.

**Elaboration** (`src/elaboration.rs`): Converts `HIR` to the final `elaboration::Journal`. `ValueExpr` trees are evaluated to `(Decimal, commodity)` pairs, commodity aliases are applied, and each transaction is balanced — if exactly one posting has no explicit amount its value is inferred as the negation of the sum of the rest. Balance assertions (`= amount`) and balance assignments (`=amount`) are checked or applied at this stage.

**Serialization**: The `Journal` implements `serde::Serialize`/`Deserialize`. The `compile` command writes it through [postcard](https://github.com/jamesmunns/postcard) into an XZ-compressed stream; the `balance` and `register` commands decompress and deserialize it in the reverse direction.

## API documentation

```
cargo doc --no-deps --open
```
