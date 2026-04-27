# Changelog

## [Unreleased]

### Breaking changes

- `.dop` binary format version bumped from 1 → 2 (adds `CommodityProperties`
  to `elaboration::Journal`). Existing `.dop` files compiled with v0.1.0 are
  **no longer readable** and must be recompiled with `dop compile`.

### Added

- `commodity` directive: `format`, `nomarket`, `note`, and `default` sub-keys
  are now parsed, resolved, and stored in `elaboration::Journal::commodities`.
- `balance` and `register` commands apply the declared `format` string when
  rendering amounts (correct prefix/suffix, thousands separator, decimal places,
  and sign placement).
- Unrecognised `commodity` sub-keys now emit a warning instead of panicking.

### Fixed

- `detect_separators`: a lone separator followed by exactly 3 digits (e.g.
  `$1.000`) is now correctly treated as a thousands separator rather than a
  decimal point, matching ledger convention.
- Sign placement for prefix-symbol formats: `-$100` instead of `$-100`.

### Not in scope

- `print` re-emits raw source text and intentionally does not apply format
  strings — source amounts are not reformatted. Users who want formatted amounts
  should use `balance` or `register`.

---

## [0.1.0] - 2026-04-22

First tagged release.

### Pipeline

- Four-stage compiler: PEG parser → resolution → elaboration → binary serialisation (postcard + XZ → `.dop`)
- `compile()` entry point processes a ledger source string end-to-end
- Historical price directives (`P`) parsed, resolved, and wired through to the elaborated journal
- `define` directives enable named value aliases with context versioning

### Library API (`src/lib.rs`)

- `compile(source, parser)` — full pipeline, returns `elaboration::Journal`
- `write_ledger(txns, writer)` — serialise a sequence of `resolution::Transaction` to any `Write` sink
- `eval_transaction(txn, context)` — evaluate a single transaction through elaboration without a full journal
- `dop_write_header(writer)` / `dop_read_header(reader, path)` — portable `.dop` header I/O with clear version-mismatch errors
- `resolution::Transaction` and `resolution::Posting` builder APIs (`new`, `with_posting`, `with_tag`, `with_comment`, `with_metadata`, `with_amount`, `with_code`, `with_state`)
- Typed amount shorthand: `From<(Decimal, S)> for ValueExpr`, `From<I: Into<ValueExpr>> for AmountDetails`

### `.dop` binary format

- 8-byte header: `DOP\0` magic + u16 version (currently 1) + u16 reserved
- Incompatible files produce a clear error rather than a silent mis-parse
- Body: postcard-serialised `elaboration::Journal` wrapped in XZ compression

### Balance assertion enforcement

- Standalone balance-assertion directives (`= amount` / `== amount`) parsed and resolved
- Assertions enforced during elaboration; `ElaborationError::BalanceAssertionFailed` carries account, date, expected, and actual amounts
- Posting-level balance assertions also enforced

### CLI

- `compile` — parse and compile a `.ledger` file to a `.dop` binary
- `balance` — account balances with `--depth`, `--flat`, `--begin`, `--end`, `--cleared`, regex `--pattern`; text, JSON, CSV output
- `register` — posting register with `--begin`, `--end`, `--cleared`, regex `--pattern`; text, JSON, CSV output
- `print` — re-emit journal as canonical Ledger source text
- `stats` — transaction/account/commodity counts and date range
- `accounts` — list all account names

### Deferred to v0.2

- Query / filter DSL (#23, #45)
- Framed postcard format with append, range-scan, and snapshot support (#17, #39–#41)
- `JournalFilter` shared abstraction (#43)
- `Amount` accessor ergonomics; `pub(crate)` tightening of `HIR` internals (#24)
