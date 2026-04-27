# Changelog

## [Unreleased]

---

## [0.2.0] - 2026-04-27

### Breaking changes

- `.dop` binary format version bumped from 1 → 2 (adds `CommodityProperties`
  to `elaboration::Journal`). Existing `.dop` files compiled with v0.1.0 are
  **no longer readable** and must be recompiled with `dop compile`.
- `parser::Parser<F>::opener` signature changed from
  `Fn(&str) -> String` to `Fn(&str) -> Result<String, Box<dyn std::error::Error>>`
  so custom openers can surface I/O errors (file-not-found, glob with no
  matches, non-UTF-8 paths). Library consumers must wrap any existing closure
  body in `Ok(...)`. The CLI's [`crate::file_opener`] is unchanged for callers.

### Added

- **Glob `!include` patterns** (#75): `include path/*.ledger` and recursive
  `**/*.ledger` are expanded in lexicographic order. A glob with no matches
  produces a clear error rather than silently including nothing.
- **Account-level `assert` and `check` directives** (#76): nested under an
  `account` block, evaluated for every posting to that account. `assert`
  failures halt elaboration with `ElaborationError::AccountAssertionFailed`;
  `check` failures emit a warning and continue.
- **Tag directive value validation** (#87): `tag NAME` blocks accept
  `assert <expr>` / `check <expr>` sub-directives where `value` is bound to
  the tag's string value. Validates every metadata-style tag (`; key: value`)
  on transactions and postings.
- **Parameterized `define` macros** (#87): `define name(p1, p2, ...) = expr`
  with both value-typed and bool-typed bodies. Cyclic definitions
  (`define a = b; define b = a`) are caught with `RecursionLimitExceeded`
  rather than crashing the process.
- **Regex match operators `=~` and `!~`** (#86) and the **`tag()` built-in
  function** for boolean expressions: `assert tag("Entity") =~ /^Foo/`.
  Patterns are compiled at parse time so syntactic errors fail fast.
- **`commodity` directive sub-keys** (#84): `format`, `nomarket`, `note`, and
  `default` are now parsed, resolved, and stored in
  `elaboration::Journal::commodities`. The `balance` and `register` commands
  apply the declared `format` string when rendering amounts (prefix/suffix
  placement, thousands separator, decimal places, sign placement).
  Unrecognised sub-keys emit a warning instead of panicking.
- **Transaction-level metadata inherited by postings** (#96): `tag()` lookups
  inside per-posting assertions now see transaction-level metadata
  (e.g., `; Entity: foo` declared on the transaction line). Posting-level
  metadata wins on key collision. Matches OG ledger-cli semantics.
- **Parenthesised boolean expressions** (#94) in `define` bodies and value
  positions: `(amt > -100 or (tag("X") !~ /pat/ and amt < 100))`.
- **`--tag KEY` filter** on `balance` and `register` (#72): include only
  transactions tagged with the given key (transaction-level or any posting).

### Fixed

- `detect_separators` (#84): a lone separator followed by exactly 3 digits
  (e.g. `$1.000`) is now correctly treated as a thousands separator rather
  than a decimal point, matching ledger convention.
- Sign placement for prefix-symbol formats (#84): `-$100` instead of `$-100`.
- `and`/`or` no longer consumed as identifier-style commodities, so
  `assert amt > 0 and amt < 100` parses correctly (#85).
- Bare balance-assignment `=0` (no commodity) infers commodity from the
  account's existing balance, the same transaction's other postings, or the
  default commodity directive — in that priority order (#77).
- Nested `include` paths under a relative `base_path` no longer double-prefix
  (e.g. `accounts/accounts/config/...`) (#92).
- `commodity ... note` no longer emits a spurious "unrecognised sub-key"
  warning (#93).

### Internal

- Shared `JournalFilter` struct extracted from duplicated `balance`/`register`
  filter logic (#72).

### Not in scope

- `print` re-emits raw source text and intentionally does not apply format
  strings — source amounts are not reformatted. Users who want formatted
  amounts should use `balance` or `register`.

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
