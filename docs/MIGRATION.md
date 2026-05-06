# Migrating from doppio 0.x to 1.0

doppio 1.0 commits to a stable, language-agnostic compiled-journal
format and a curated public Rust API. Reaching that commitment took
two waves of breaking changes since the last published release
(v0.2.0):

1. **Wave 1: wire-format migration** (originally cut as `v0.4.0-rc.1`,
   tag-only). The `.dop` body moved from postcard + xz to canonical
   Protocol Buffers + deflate; `elaboration::Journal` and friends are
   now prost-generated proto types rather than hand-written
   BTreeMap-based wrappers.
2. **Wave 2: API surface audit** (1.0 itself). Internal types were
   demoted to `pub(crate)`, the legacy `parser` module was removed,
   the `Frontend` trait was tightened to return `resolution::HIR`,
   and a few format-internals went private behind their public
   wrappers.

This guide walks each affected audience through what changed and how
to update.

> **Audiences**
>
> 1. **Rust library consumers** depending on the `doppio` crate (Section
>    [§1](#1-rust-library-consumers)).
> 2. **`.dop` file consumers** in any language reading the protobuf
>    format (Section [§2](#2-dop-file-consumers)).
> 3. **CLI users** of the `dop` binary (Section [§3](#3-cli-users)).

For the full release history, see [`CHANGELOG.md`](../CHANGELOG.md).
For ongoing API-evolution rules from 1.0 forward, see
[`docs/proto-evolution.md`](./proto-evolution.md) and the API
discipline notes in [`docs/requirements.md`](./requirements.md).

---

## 1. Rust library consumers

### 1.1 The 30-second summary

If you compile a journal, write/read `.dop` files, and inspect the
resulting `elaboration::Journal`, your call sites are likely already
correct after the 0.4.0-rc.1 wave. The only further changes from
the API audit are:

- `doppio::parser::*` → `doppio::grammars::ledger::*`. The shim
  module was deleted.
- `Frontend::parse` returns `resolution::HIR` instead of
  `ast::Journal`; drop the `.try_into()` step at every call site.
- A handful of `ast::*` and `resolution::*` items now `pub(crate)`.
  In practice consumers of the documented surface (`compile`,
  `eval_transaction`, the `resolution::Transaction` builder API,
  `read_dop`/`write_dop`, `write_ledger`) don't touch them.

The rest of this section is the per-area detail.

### 1.2 Wire-format types: `proto` → `elaboration`, hand-rolled → prost-generated

(Originally landed in v0.4.0-rc.1; included here because some external
consumers still bridge directly from v0.2.0.)

`doppio::proto` was renamed to `doppio::elaboration`. The previous
hand-written types (`Amount(BTreeMap<...>)`, `ResolvedTransaction`,
`ResolvedPosting`, `Decimal as rust_decimal::Decimal`, etc.) are
gone. The public types are now prost-generated from
`proto/doppio.proto`.

The functional consequence at the call site:

| 0.2.0 idiom | 1.0 replacement |
|---|---|
| `tx.amount.0.get("$").unwrap()` (panic-prone) | `tx.amount_in("$").unwrap_or(Decimal::ZERO)` |
| `transaction.date` (was `chrono::NaiveDate`) | `transaction.date_naive()` (returns `chrono::NaiveDate`) |
| `posting.amount` (was `Amount`) | `posting.amount()` (infallible accessor returning `&Amount`) — or `posting.amount_in(commodity)` for a single-commodity lookup |
| `for (commodity, value) in &amount.0` | `for (commodity, value) in amount.iter()` (sorted by commodity) |
| `transaction.state` (was an enum) | `transaction.state()` (typed accessor over the proto3 i32 field) |
| `decimal_from_proto(d)` | `d.to_decimal()` — inherent method on the proto type. The free function still exists internally but is no longer public; see §1.7. |

The protobuf schema itself lives in [`proto/doppio.proto`](../proto/doppio.proto)
and is the source of truth. Inherent methods on the prost-generated
types provide an ergonomic Rust surface; see the type docs for the
full inventory.

### 1.3 `pub mod parser` removed

The backwards-compat re-export shim at `doppio::parser::*` is gone.
Reach for `doppio::grammars::ledger::*` directly:

```rust
// 0.x
let mut parser = doppio::parser::Parser {
    opener: |_| Ok(String::new()),
    base_path: PathBuf::new(),
};

// 1.0
let mut parser = doppio::grammars::ledger::Parser {
    opener: |_| Ok(String::new()),
    base_path: PathBuf::new(),
};
```

`doppio::parser::Rule`, `LedgerParser`, and `parse_ledger` are
similarly accessed through their canonical paths under `grammars::`.

### 1.4 `Frontend::parse` now returns `resolution::HIR`

The trait used to return `ast::Journal`; callers immediately did
`ast_journal.try_into::<HIR>()?` to cross into the resolved stage.
The trait now does that step internally, so:

```rust
// 0.4.0-rc.1
let ast = frontend.parse(&source, base_path, &doppio::file_opener)?;
let hir: doppio::resolution::HIR = ast.try_into()?;
let journal = doppio::elaborate(hir)?;

// 1.0
let hir = frontend.parse(&source, base_path, &doppio::file_opener)?;
let journal = doppio::elaborate(hir)?;
```

If you used `compile()` (which takes a `Parser` and returns a
`Journal`), no change is needed — the function's signature is the
same.

### 1.5 `ast::*` is now mostly internal

The parser-stage AST module remains `pub` but nearly every type and
field inside is `pub(crate)`. The public surface inside `ast::*` is:

- `Journal` (returned from `Parser::parse`, but with a `pub(crate)
  entries` field — pass it through, don't iterate)
- `ValueExpr`, `AmountDetails` (carrier types in the resolution
  builder API; both `#[non_exhaustive]` so 1.x can grow new variants)
- `TransactionState`, `PostingKind` (used in the resolution builder
  fields)
- `LotPricing`, `LotAnnotation`, `Op` (helpers reachable through
  `AmountDetails`)
- `BoolExpr`, `CmpOp`, `BoolOp` (reachable through
  `ValueExpr::Group`)

If you matched on AST `Entry`, `Directive`, parser-stage `Transaction`
or `Posting`, etc., you were touching internal IR. The intended
consumer pipeline is:

1. **Parsing** — call `compile()` or `Frontend::parse` for the high
   level; or build a `resolution::Transaction` directly via the
   builder API.
2. **Inspecting** — work with `elaboration::*` (the proto types) for
   queries, or `resolution::HIR` for the pre-elaboration intermediate.
3. **Serializing back** — `write_ledger` for source text,
   `write_dop` for the binary format.

If you have a use case that genuinely needs deeper AST access, please
file an issue. None of the types we demoted have known external
consumers — but if yours surfaces, we'd rather know now.

### 1.6 `resolution::*` items demoted

Builder API and HIR-level types stay public:

- `resolution::Transaction`, `Posting` and all their `with_*`
  builders.
- `resolution::HIR` plus `HIR::transactions()`.
- `resolution::Context`, `GlobalContext`, `AccountProperties`,
  `CommodityProperties`, `TagProperties`, `HistoricalPrice`,
  `ResolutionError`.

What went `pub(crate)`:

- `resolution::Entry`, `ResolutionEntry`, `AssertionDirective`,
  `Define` — HIR-internal variants.
- `HIR.entries` and `Context.defines` (field types are now private,
  field visibility tracks them).
- `AccountProperties.{asserts,checks}` and
  `TagProperties.{asserts,checks}` — these were the only fields
  exposing `BoolExpr`, which moved out of the public surface as a
  result.

`AccountProperties` and `TagProperties` are still constructable via
`Default::default()` and inspectable via their public fields
(`note`, `metadata` for accounts; commodity properties unchanged).

### 1.7 `lib.rs` format-internals went private behind their public wrappers

| 0.x item | Disposition | Replacement |
|---|---|---|
| `DOP_MAGIC` (const) | `pub(crate)` | use `read_dop`/`write_dop` |
| `DOP_FORMAT_VERSION` (const) | `pub(crate)` | use `read_dop`/`write_dop` |
| `dop_write_header(writer, compression)` | `pub(crate)` | `write_dop(journal, writer, compression)` |
| `dop_read_header(reader, path)` | `pub(crate)` | `read_dop(reader, path)` |
| `decimal_from_proto(p)` | `pub(crate)` | inherent method `p.to_decimal()` |

If you were poking at the header to detect `.dop` files vs source
text, switch to extension-based detection or call `read_dop` and
match on the typed error variant — the message strings and error
shape are stable.

### 1.8 `Frontend` trait, `LedgerFrontend`, `HledgerFrontend`

Unchanged externally, except for the `parse` return type
([§1.4](#14-frontendparse-now-returns-resolutionhir)). If you implement
`Frontend` for a new format, do the `ast → HIR` conversion inside
your `parse` impl (mirror the in-tree implementations).

### 1.9 `pest`-derived parser structs went private

`grammars::ledger::LedgerParser` and `grammars::hledger::HledgerParser`
are now `pub(crate)`. They were always implementation detail. The
`Parser<F>` wrapper (which holds the opener and base path) stays
public — that's what `compile()` accepts.

### 1.10 Pattern-matching on `ValueExpr` and `AmountDetails`

Both enums gained `#[non_exhaustive]`. External `match` arms must
include a wildcard `_`:

```rust
// 0.x — this compiles fine
match expr {
    ValueExpr::Amount { .. } => …,
    ValueExpr::Binary(..) => …,
    // …every variant…
}

// 1.0 — non_exhaustive forces a wildcard
match expr {
    ValueExpr::Amount { .. } => …,
    // …handle the variants you care about…
    _ => …,  // required
}
```

In practice external consumers construct via `ValueExpr::amount(value,
commodity)`, `ValueExpr::parse(str)`, or
`From<(Decimal, S)>` and rarely match on the enum.

### 1.11 `Amount` re-export (additive)

The multi-commodity `Amount` type that's carried inside
`ElaborationError::TransactionDoesNotBalance(Amount)` is now
re-exported at the crate root: `use doppio::Amount;`. Previously you
could match the variant but not name the inner type.

### 1.12 Examples deleted

`crates/doppio/examples/parse_and_print.rs` and `load_and_print.rs`
were removed. They were parser-development tools that printed
`Debug` of an `ast::Journal`; keeping them would have anchored the
AST visibility wider than the audit's recommendations.

### 1.13 Dependencies

Since v0.2.0:

- **Removed**: `postcard`, `xz` (replaced by `prost` + `miniz_oxide`).
- **Added**: `prost = 0.13`, `prost-build = 0.13`, `protoc-bin-vendored`,
  `miniz_oxide = 0.8`, `regex` (CLI patterns).
- **Pruned features** (per #125): the `derive` feature on `serde` is no
  longer enabled; `chrono`'s `serde` feature was dropped; the workspace
  no longer requires `serde_json` outside the CLI.

If you transitively relied on any of those features being on, you'll
need to enable them explicitly in your own crate.

### 1.14 Quick checklist for upgrading a downstream Rust crate

1. Bump `doppio = "1.0"` (and update the `Cargo.lock`).
2. Replace `doppio::parser::*` with `doppio::grammars::ledger::*`.
3. Drop `.try_into()` after any `Frontend::parse` call.
4. Replace `decimal_from_proto(d)` with `d.to_decimal()`.
5. If you matched on `ValueExpr` or `AmountDetails`, add a wildcard
   arm.
6. Run `cargo check`. Fix any "private type in public interface" or
   "private function" errors by routing through the documented
   wrapper (consult the table in [§1.7](#17-librs-format-internals-went-private-behind-their-public-wrappers)).
7. If you depend on a feature flag of `serde`/`chrono` that doppio
   stopped enabling transitively, add it to your own `Cargo.toml`.

---

## 2. `.dop` file consumers

### 2.1 Wire-format version

`DOP_FORMAT_VERSION = 3` from v0.4.0-rc.1 forward. **No bump for the
1.0 cut itself.** Files compiled by 1.0 are byte-compatible with
files compiled by v0.4.0-rc.1 — the schema only gained additive
fields under proto3 evolution rules.

Files compiled by v0.2.0 (header version 2, postcard+xz body) are
**not readable** by 1.0. Recompile from source with
`dop compile <source.ledger> -o <out.dop>`.

### 2.2 What changed in the schema since v0.4.0-rc.1

Additive only. Each new field has its own tag and old readers
ignore unknown fields per proto3:

- `Posting.kind` (tag 7, `PostingKind` enum) — virtual-posting
  semantics. `UNSPECIFIED = 0` is treated as `REAL` so older `.dop`
  files behave correctly.
- `Posting.lot` (tag 8, optional `Lot` message) — lot-cost-basis
  annotation surfaced as `{cost} [date] ((note))` in source.
- `AccountProperties.metadata` (tag 2, `map<string, string>`) — keyed
  metadata extracted from `; key: value` notes on `account`
  directives; denormalised by inheritance.

The full inventory and inherent helper recipes are in
`proto/doppio.proto`'s top comment block (decimal reassembly,
exchange-rate algorithm, evolution policy).

### 2.3 Header layout

Unchanged: 8 bytes total, magic `DOP\0` + u16 LE version + 1 byte
compression + 1 reserved byte. Compression codes: `0 = none`,
`1 = deflate` (raw deflate, not zlib-wrapped — `pako`'s `inflateRaw`
on the JS side, `miniz_oxide::inflate::decompress_to_vec` on the
Rust side).

### 2.4 Cross-language reading

The JS-native reader at `web/src/lib/dop/` is the working reference
implementation. See `proto/doppio.proto`'s top comment for the
canonical algorithms (decimal reassembly, exchange rates) with
recipes in Rust, Python, TypeScript, and Go. Future evolution
follows the rules in [`docs/proto-evolution.md`](./proto-evolution.md).

---

## 3. CLI users

The `dop` binary is feature-additive since v0.2.0. No flags or
subcommands were renamed or removed in the 1.0 cut. Recap of new
surface for users coming from v0.2.0:

### 3.1 `dop compile`

- `--no-compression` — write raw protobuf instead of deflate.

### 3.2 `dop balance` / `dop register`

- `--begin <YYYY-MM-DD>`, `--end <YYYY-MM-DD>` — date-range filter.
- `--cleared` — only cleared transactions.
- `--tag <name>` — only transactions tagged with `name`.
- `--depth <N>` (balance only) — collapse accounts deeper than N
  colon-separated levels.
- `--flat` (balance only) — disable the default tree view.
- `--format text|json|csv` — structured output.
- `-R`, `--real` — exclude virtual postings.
- `-X`, `--exchange <COMMODITY>` — convert balances/amounts to a
  target commodity using `P` directive prices.

### 3.3 New subcommands

- `dop accounts <source> [pattern]` — list every account, optionally
  filtered.
- `dop commodities <source>` — list every commodity used.
- `dop stats <source>` — transaction count, account count, commodity
  count, date range.
- `dop print <source>` — re-emit canonical Ledger source text from a
  source file. **Only accepts source files** (the `.dop` binary
  format does not preserve the original transaction structure).

### 3.4 hledger / `.hledger` / `.journal` files

`dop` now picks the hledger frontend automatically based on the
file extension. Run `dop balance my.hledger` and the dispatch is
transparent. See `docs/SUPPORTED_FEATURES.md` for the dialect
matrix.

---

## Reference

- [`CHANGELOG.md`](../CHANGELOG.md) — full version history.
- [`docs/proto-evolution.md`](./proto-evolution.md) — wire-format
  evolution rules from 1.0 forward.
- [`docs/SUPPORTED_FEATURES.md`](./SUPPORTED_FEATURES.md) — what
  ledger-cli / hledger features are implemented.
- [`docs/exchange-rates.md`](./exchange-rates.md) — FX semantics
  rationale.
- [`docs/requirements.md`](./requirements.md) — the canonical
  requirements doc backing the public API surface.
- [`proto/doppio.proto`](../proto/doppio.proto) — the wire-format
  schema, with recipes for non-Rust consumers.
