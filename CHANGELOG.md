# Changelog

## [Unreleased]

### Added

- **`C N1 X = N2 Y` commodity-conversion directive** for the ledger-cli frontend (#248 stage 2).
  Declaring `C 1.00s = 100c` makes every `c`-denominated posting convert to shillings at a
  divisor of `N1 * N2` (empirically confirmed against ledger-cli): `250c / 100 = 2.50s`.
  Postings in the canonical LHS commodity are similarly scaled by `N1`.  The conversion is
  applied at elaboration time via the existing `commodity_conversions` map; aliases declared with
  `commodity X / alias Y` continue to use `divisor = Decimal::ONE` (a 1:1 rename, unchanged).
  The directive is context-versioned: transactions that precede a `C` directive in source order
  are not retroactively affected.  **No chaining**: `C 1G = 100s` + `C 1s = 100c` converts
  `c`-postings to `s` only (one hop), matching ledger-cli's observed behaviour.
  Grammar addition: `number ~ commodity` (no-space number-first form, e.g. `1428c`) is now
  accepted in posting amounts, enabling wow.dat-style postings.

### Internal

- **`Context::commodity_aliases` renamed to `commodity_conversions`** (pre-stage for #248, no
  behavioural change): the field type changes from `BTreeMap<String, String>` to
  `BTreeMap<String, (String, Decimal)>`, where the tuple is `(canonical_symbol, divisor)`.
  All existing alias insertions use `divisor = Decimal::ONE` (a 1:1 rename), so elaboration
  output is identical. This unification makes stage 2 of #248 (wiring the divisor through
  for full `C`-directive semantics) a natural extension rather than a new concept.
- **`elaborator::evaluator::eval_and_normalize_amount_with_fallback` now applies the conversion
  divisor** stored in `commodity_conversions`: amounts are divided by the divisor and rebranded
  to the canonical commodity at evaluation time. Previously the divisor was stored but ignored.

### Added

- **Explicit `*N` multiplier syntax in automated-rule bodies** (#254): both the
  ledger-cli and hledger frontends now accept `*N` (e.g. `*-1`, `* 0.5`,
  `*0.12`) as a posting amount inside an `= QUERY` auto-rule body. The `*N`
  form is lowered to the same bare-number multiplier representation as a plain
  bare decimal — no new semantic path in the elaborator. `*-1` negates the
  matched posting's amount; `*0.10` takes 10%, etc.

### Test infrastructure

- **Vendor `tests/parity/ledger-drewr3.dat`** (refs #257, refs #197): the
  ledger-cli upstream `test/input/drewr3.dat` fixture is vendored with a
  provenance comment block (source URL, commit SHA, license, exercises). It is
  NOT yet wired into the positive parity harness: `drewr3.dat` exposed a
  gap in how `dop balance --flat` reports the balance of intermediate accounts
  that carry both direct postings and child-account postings (ledger-cli
  aggregates the subtree; doppio reports only the direct balance). The fixture
  is tracked in the repo so the gap is reproducible; the harness entry will be
  added once the gap is resolved.

- **Implicit cost-basis inference for ledger-cli and hledger** (#251): a
  two-real-posting multi-commodity transaction where neither posting carries an
  explicit `@`/`@@` price or `{cost}` lot annotation is now accepted by the
  ledger and hledger frontends. The cash leg's absolute amount is synthesised as
  an implicit `@@` total-cost on the non-cash leg, matching ledger-cli and
  hledger semantics. Controlled by the new
  `ElaborationConfig::infer_implicit_total_cost` field (default `false`), which
  is set to `true` in `ledger_defaults()` and `hledger_defaults()`.
  **Beancount is unaffected**: Beancount requires an explicit cost on every
  lot-bearing posting, and `beancount_defaults()` leaves `infer_implicit_total_cost`
  `false`.
- **Automated transaction rules (`= QUERY`)** for both the ledger-cli and hledger frontends (#249).
  Rules are applied during elaboration: for every real posting whose account name matches the rule
  query, each body posting is synthesized as a `PostingKind::VirtualUnbalanced` entry (excluded from
  balance checks, but included in running account-balance state). Multiplier semantics: a body
  posting with a commodity-less bare decimal amount (e.g. `0.10`) scales the matched posting's
  amount in the matched posting's own commodity; body postings that carry an explicit commodity
  symbol (e.g. `50 USD`) are taken as literal amounts. Rule queries wrapped in `/pattern/` are
  compiled as regexes; bare string queries become case-insensitive substring regexes.

### Notes

- **`~` periodic transaction directives** are accepted by both frontends but are not elaborated.
  No postings are synthesized and no balance state is affected. This is a deliberate parse-and-
  discard choice: doppio models accounting facts, not budget planning.

## [0.4.0-rc.1] - 2026-04-28

This release-candidate cuts the toward-1.0 architectural surface for
review-by-use. **Not published to crates.io** -- referenced by
downstream consumers via the `v0.4.0-rc.1` git tag.

### Breaking changes -- public type surface (Phase C, milestone 7)

- **`doppio::elaboration::*` is now the prost-generated Protocol Buffers
  namespace.** The previous BTreeMap-based wrapper types
  (`elaboration::Journal`, `ResolvedTransaction`, `ResolvedPosting`,
  `Amount(BTreeMap<...>)`, `AccountProperties`, `CommodityProperties`,
  `HistoricalPrice`, `TransactionState`) are removed from the public
  API. Replacements: same names under `elaboration::*`, but with the
  proto wire-shape (`Option<Amount>`, `Decimal` as a 3-field message,
  `state: i32`). Inherent methods cover the read-side ergonomic
  surface (see "Added").
- **`compile()`, `read_dop()`, `write_dop()` take/return the new
  `elaboration::Journal` directly.** No more conversion at call
  sites. `read_dop_proto()` removed (now identical to `read_dop()`).
- **`eval_transaction()` returns `elaboration::Transaction`**
  (previously `elaboration::ResolvedTransaction`).
- **`.dop` binary format version bumped from 2 -> 3.** Body encoding
  migrated from `postcard` + `xz` to canonical Protocol Buffers
  (`prost 0.13`) with optional deflate compression (`miniz_oxide
  0.8`). Header layout unchanged (8 bytes: `DOP\0` magic, u16
  version, compression byte, reserved byte) but byte 6 now encodes
  the compression algorithm: `0 = none`, `1 = deflate`. Existing
  `.dop` files compiled with v0.2.0 are **no longer readable** and
  must be recompiled with `dop compile`.

### Added -- Phase C ergonomic accessors

Inherent methods on the prost-generated `elaboration::*` types so
consumers don't reinvent wire-shape unwrapping at every call site.
Per the standing project rule, all consumer-facing helpers for the
read-side public API are method-style on the generated types; free
functions like `decimal_from_proto` exist but are not the documented
surface.

- `Decimal::to_decimal() -> rust_decimal::Decimal` -- reassemble the
  proto-encoded i128 mantissa (#117).
- `Display for Decimal` -- formats via `to_decimal()` so output
  matches `rust_decimal::Decimal`'s Display (#116).
- `Amount::iter() -> impl Iterator<Item = (&str, Decimal)>` -- sorted
  by commodity symbol (BTreeMap-based per build.rs config) (#114).
- `Amount::get(commodity) -> Option<Decimal>` (#114).
- `Posting::amounts()` / `amount_in(commodity)` -- same shape as
  `Amount::iter` / `get`, applied through the `Option<Amount>`
  wrapper (#114).
- `Posting::amount() -> &Amount` -- infallible accessor that papers
  over the proto3 `Option<Amount>` quirk (returns a static empty
  `Amount` if the field is `None`, which the elaborator never
  legitimately produces) (#121).
- `Transaction::date_naive() -> chrono::NaiveDate` and
  `Transaction::secondary_date_naive() -> Option<chrono::NaiveDate>`
  (#122).
- `HistoricalPrice::date_naive() -> chrono::NaiveDate` (#122).

### Added -- frontend / serialization

- **hledger frontend** (#103): `.hledger` and `.journal` files are now
  parsed by a dedicated hledger frontend (`HledgerFrontend`) rather than
  falling through to the ledger-cli parser. Key additions over the
  ledger-cli grammar:
  - Date separators `/` and `.` in addition to `-` (e.g. `2024/01/15`,
    `2024.01.15`).
  - Comment lines starting with `#` (in addition to `;`).
  - `commodity` directive accepts a format string directly
    (e.g. `commodity $1,000.00`) as well as a bare symbol.
  - `account` directive accepts a `type` sub-key.
  - Periodic transactions (`~`) are parsed and silently ignored.
  - Automated posting rules (`= query`) are parsed and silently ignored.
    **Automated posting arithmetic bodies (`*N`) are not yet elaborated**
    (TODO #103 followup).
  - The `Frontend` trait, `frontend_for_extension()`, and
    `HledgerFrontend` are all public so library consumers can select or
    instantiate the frontend directly.
- `doppio::write_dop(journal, writer, compression)` -- public API to
  serialise a compiled journal to any `Write` sink.
- `doppio::read_dop(reader, path)` -- public API to deserialise a `.dop`
  file from any `Read` source with clear error messages.
- `doppio::elaborate(hir)` -- convenience function: runs the elaboration
  stage on a resolved HIR, returning `elaboration::Journal`. Used by
  the CLI when dispatching to a `Frontend` manually.
- `doppio::Compression` enum (`None` | `Deflate`) -- controls the
  compression algorithm used by `write_dop`.
- `--no-compression` flag on `dop compile` -- produces uncompressed `.dop`
  files (useful for streaming or tooling that reads raw protobuf).
- `wasm32-unknown-unknown` library build is now part of CI (#101).

### Removed

- `postcard` and `xz` runtime dependencies from `doppio` and
  `doppio-cli`; replaced by `prost` and `miniz_oxide`.
- The `doppio::proto` module (renamed to `doppio::elaboration`).
- `read_dop_proto()` (now identical to `read_dop()`).
- The previous BTreeMap-based public types in `elaboration::*` (see
  Breaking changes).

### Internal

- **Repo restructured into a Cargo workspace.** The library now lives in
  `crates/doppio/` and the CLI in `crates/doppio-cli/`. The library has
  no `clap` / `serde_json` / `xz` dependencies, which means the lib
  alone compiles cleanly to `wasm32-unknown-unknown`. End-user impact:
  none for `cargo install doppio-cli` or for library consumers depending
  on the `doppio` crate. The `dop` binary is now produced by
  `doppio-cli`. `serde-pickle` (previously declared but unused) was
  also dropped from the dep tree.
- **Multi-frontend extensibility (Phase B, milestone 6).** A `Frontend`
  trait + `crates/doppio/src/grammars/` directory + extension dispatch
  refactor (#102) makes the parser layer pluggable; the hledger
  frontend is the second consumer.
- **Elaborator emits proto types directly (#133 / PR #134).** The
  transitional `elaboration_pipeline` types are gone; the elaborator's
  `try_from` constructs `elaboration::Journal` (= the prost-generated
  proto type) at every output site. Eliminates the per-load boundary
  conversion (~50ms per 100k transactions).
- **prost configured to generate `BTreeMap` for every map field**
  (`btree_map(["."])` in `build.rs`). Doppio's Rust binding has
  deterministic iteration on map fields; the protobuf spec still says
  map order is unspecified for other-language bindings.
- CLI read-only commands iterate `proto::Journal` directly via
  `read_dop_proto` (#111 / #113). Closed in this release as the
  `proto::Journal` rename made the optimization the only path.

## [Unreleased]

---

## [2.0.0] - 2026-05-08

The 2.0 cut bundles every post-1.0.0 change. v1.0.0 marked stability of the `.dop` wire format and the CLI; the library API has since added a Beancount frontend, a configurable elaboration semantics layer, per-lot inventory tracking, and auto-booking for `{}` reductions. Several changes are major SemVer breaks per [Cargo's guidance](https://doc.rust-lang.org/cargo/reference/semver.html), so this is a major bump.

The `.dop` wire format is unchanged (still version 3); v1.0.0-built `.dop` files load unchanged in v2.0.0.

### Breaking changes -- public Rust API surface

- **`elaborate()` signature** (#239): now takes an extra `&ElaborationConfig` argument. Prior signature was `pub fn elaborate(hir: HIR) -> Result<Journal, ElaborationError>`; new signature adds `config: &resolution::ElaborationConfig`. Migration: pick a per-frontend default constructor — `ledger_defaults()`, `hledger_defaults()`, or `beancount_defaults()` — and pass `&that()`.

- **`Frontend` trait gained a required method** `elaboration_defaults(&self) -> ElaborationConfig` (#239). No default body. Out-of-tree `impl Frontend for ...` blocks must add it, typically delegating to the per-frontend default constructor. Returns the elaboration config that mirrors the canonical tool's own behaviour.

- **`ElaborationError` enum: 4 new variants** added across #237 and #238: `PhantomLotReduction`, `AmbiguousLotMatch`, `OverReductionInBooking`, `AugmentingPostingWithMissingCost`. The enum is now `#[non_exhaustive]`, so external matches must include a `_` arm.

- **`AccountProperties` struct: new field `booking_method`** (#238). Now `#[non_exhaustive]`; external callers can construct via `Default::default()` and mutate fields, but cannot use struct-literal-with-spread.

- **`ElaborationConfig` struct: new fields** `lot_validation_mode` (#237) and `default_booking_method` (#238) on top of the type itself being introduced by #239. Now `#[non_exhaustive]`; same construction restriction as `AccountProperties`.

- **`ElaborationError`, `EvaluationError`, `AccountProperties`, `ElaborationConfig`, `BookingMethod`, `ToleranceMode` are now `#[non_exhaustive]`.** Future additive variants / fields won't trigger another major bump.

### Migration

```rust
// v1.0.0
let journal = doppio::elaborate(hir)?;

// v2.0.0
let journal = doppio::elaborate(
    hir,
    &doppio::grammars::ledger::ledger_defaults(), // or hledger_defaults() / beancount_defaults()
)?;
```

```rust
// v2.0.0 — Frontend impls add a method
impl Frontend for MyFrontend {
    fn extensions(&self) -> &'static [&'static str] { ... }
    fn elaboration_defaults(&self) -> ElaborationConfig {
        ElaborationConfig::default() // or a custom config
    }
    fn parse(&self, ...) -> Result<HIR, _> { ... }
}
```

```rust
// v2.0.0 — match on ElaborationError needs a `_` arm now
match err {
    ElaborationError::TransactionDoesNotBalance(_) => ...,
    ElaborationError::BalanceAssertionFailed { .. } => ...,
    // ... pre-existing variants ...
    _ => ...,  // required since the enum is #[non_exhaustive]
}
```

```rust
// v2.0.0 — AccountProperties / ElaborationConfig construction
let mut props = AccountProperties::default();
props.note = Some("Checking".into());
// (Cannot use `AccountProperties { note: ..., ..Default::default() }` from outside the doppio crate.)
```

### Added

#### New frontend
- **`BeancountFrontend`** (#145, #146, #147 + completeness PRs #185, #187, #188, #189, #193, #199, #205, #210, #212, #214, #220). Full Beancount frontend covering the parser, AST adapter, `pad` directive evaluator, balance assertions, escape sequences in string literals, `pushtag` / `poptag` / `pushmeta` / `popmeta` block scoping, multi-commodity pad, org-mode outline tolerance.

#### Elaboration semantics
- **`ElaborationConfig`** decouples elaboration semantics from frontend syntax (#239). Three knobs at v1.0.0-cut: `tolerance_mode`, `balance_mode`, `assertion_scope`. Two more added in this release: `lot_validation_mode` (#237), `default_booking_method` (#238). Per-frontend default constructors: `ledger_defaults()`, `hledger_defaults()`, `beancount_defaults()`.
- **Per-lot inventory in the elaborator's running state** (#237). Postings tracked per `(account, commodity, lot)` rather than per `(account, commodity)` aggregate. Lot keys carry cost basis, acquisition date, free-form note. Subtree aggregation, balance assertions, and `==*` synthesis collapse the lot dimension and behave identically for journals without lot annotations.
- **`LotValidationMode`** (`Permissive` / `Strict`). The Beancount frontend defaults to `Strict`: a reducing posting whose lot key has no matching position in an account that already holds the same commodity under different lots raises `ElaborationError::PhantomLotReduction`, matching bean-check's STRICT booking. ledger-cli + hledger defaults remain `Permissive`. (#237)
- **`BookingMethod` and auto-booking for `{}` reductions** (#238). Six values mirroring Beancount's `Booking` enum: `Strict` (default; reject ambiguous matches), `StrictWithSize`, `None`, `Average`, `Fifo`, `Lifo`, `Hifo`. The Beancount parser captures it on `open` directives as a structured `AccountItem::Booking(BookingMethod)`. At elaboration time a `{}` (or partial-spec) reduction is matched against the account's inventory per the configured method; multi-lot reductions split into one synthesised posting per matched lot. Lot dates auto-fill from the transaction date on augmenting postings, required for FIFO / LIFO to distinguish cost-only annotations.
- **Cost-basis-aware gain inference for `{}` reductions** (#242). The booking step runs inline during the per-posting elaboration loop, so each booked sub-posting's cost-basis cash flows into `transaction_state` before the null posting is inferred. A null `Income:Trading` posting in a `{} @ price` transaction now fills with the realised gain (cash − cost-basis sum) rather than zero, matching bean-check.
- **Partial-spec lot resolution** (#227). `-30 AAPL {185.40 USD}` (cost specified, date inherited as None) now matches against the concrete dated inventory lot via the same booking pass that handles `{}`. Phantom-cost reductions error with `OverReductionInBooking`.
- **Beancount-style balance tolerance via synthetic rounding posting** (#198). New `--tolerance` CLI flag accepts any decimal in `[0, 1)`. `option "inferred_tolerance_default"` directives populate per-commodity overrides on `GlobalContext::tolerance_overrides`.
- **Cross-frontend total-cost lot basis** (#193). `{{total}}` syntax computes per-unit cost as `total / units` consistently across the three frontends.
- **hledger `==*` / `=*` strict-zero balance assignment** (#200, #207). Subtree-aware: `Income ==* 0` zeroes the entire `Income:*` subtree by synthesising a corrective posting on `Income` itself.
- **hledger account-type tag inheritance** (#217). Pinned via tests.
- **Subtree-aware `balance` directives** for Beancount and hledger `==*` (#214).
- **Per-frontend lot `{cost}` / `@price` balance semantics** (#210). `BalanceMode::CostBasis` (ledger-cli + Beancount): `{cost}` drives cash, `@price` informational. `BalanceMode::AtPriceWithSynthesis` (hledger): `@price` drives cash, gains synthesised on the configured account.

#### Parser / frontend completeness
- **Beancount pad fires per-commodity** (#220). Each asserted commodity on the same target consumes the pending pad once.
- **Beancount accepts shebang and `#+` org-mode startup directives** (#199).
- **hledger `comment ... end comment` block-comment syntax** (#205).
- **ledger `apply tag` / `end tag` block markers** propagate onto enclosed transactions (#222).
- **ledger automated transaction rules `= /pattern/`** parse without error (#219); elaboration deferred to a future release.
- **ledger parser tolerates trailing whitespace** on posting lines (#247). Real-world journals (e.g. ledger-cli's own `test/input/standard.dat`) pad account names to a fixed column; the prior strict rule rejected that padding on null postings.
- **Beancount string-literal escape sequences** (#187).

#### Tooling and tests
- **Cross-frontend parity harness** (#183) under `scripts/parity_check.py`. Five comparators -- balance equality, per-transaction tags + metadata (#226 Phase 1), explicit historical-price quotes (Phase 2), pad-synthesised transactions (Phase 3), and per-`(account, commodity, lot)` inventory positions (#227 Phase 4) -- plus a negative-control mode for fixtures that should be rejected by both tools (#226 acceptance: comparator self-tests). 17 positive + 4 negative fixtures vendored under `tests/parity/`. Beancount, hledger, ledger-cli all covered above the bar set in #197.
- **`dop register --format=json` rows** carry an optional `lot` object (`cost_amount`, `cost_commodity`, `date`, `note`) when present (#227). Older rows without the field continue to deserialise.
- **`dop balance --format=json`** stable across the additions.

### Fixed

- **#247** ledger parser rejects null posting with trailing whitespace -- regression test.
- **#220** Beancount pad fired only once per target account; now fires per-commodity.
- **#185** lot-annotation completeness: `{{total}}` arithmetic, parenthesised cost arithmetic, per-currency basis.
- **#193** cross-frontend `{{total}}` lot basis was inconsistent.

### Wire format

`.dop` schema unchanged (still version 3). v1.0.0-built `.dop` files load unchanged. v2.0.0-built `.dop` files written from the same source may differ from v1.0.0 output in two observable ways:

1. **Lot acquisition dates** auto-fill from the transaction date on augmenting postings. v1.0.0 source `Assets:Brokerage 10 AAPL {$150}` recorded `lot.date = None`; v2.0.0 records `lot.date = transaction_date`. Required for FIFO / LIFO matching.
2. **`{}` reductions** become per-lot booked sub-postings. v2.0.0 may produce more postings per transaction than v1.0.0 from the same source.

Neither requires a wire-format version bump; downstream consumers reading mixed-vintage `.dop` files should expect the per-lot dimension to be richer in v2.0.0-built files.

---

## [1.0.0] - 2026-05-06

### Breaking changes -- public Rust API surface (Phase E, milestone 11)

The 1.0 cut commits to a curated public API. The audit (#159) demoted
items used only internally to `pub(crate)`; what stays public is the
documented surface that downstream consumers (`bb-ledger`, `bookie`,
`betterbytes-org/ledger`) actually use.

- **`pub mod parser` removed.** The backwards-compat shim at
  `doppio::parser::*` is gone. Reach for `doppio::grammars::ledger::*`
  directly.
- **`Frontend::parse` now returns `resolution::HIR`** (was
  `ast::Journal`). Every external caller drops the intermediate
  `try_into()` step. `LedgerFrontend` and `HledgerFrontend` impls do
  the resolution step internally.
- **`pub mod ast` items demoted.** `Entry`, `Directive`,
  `CommodityItem`, `AccountItem`, `AssertionDirective`,
  `HistoricalPrice` (parser-stage), `Transaction` / `Posting`
  (parser-stage), `DefineBody`, `Date` -> `pub(crate)`.
  `Journal.entries` is now `pub(crate)` (the `Entry` enum is
  internal). `BoolExpr` / `CmpOp` / `BoolOp` stay public because
  `ValueExpr::Group` carries a boxed `BoolExpr`. Public types
  reachable transitively (`ValueExpr`, `AmountDetails`,
  `TransactionState`, `PostingKind`, `LotPricing`, `LotAnnotation`,
  `Op`) stay.
- **`pub mod resolution` items demoted.** `Entry`, `ResolutionEntry`,
  `AssertionDirective`, `Define` -> `pub(crate)`.
  `AccountProperties.{asserts,checks}` and
  `TagProperties.{asserts,checks}` -> `pub(crate)` (these were the
  only fields exposing `BoolExpr` from outside `ast`). The builder
  API on `resolution::Transaction` / `Posting` is unchanged.
- **`lib.rs` format internals demoted.** `DOP_MAGIC`,
  `DOP_FORMAT_VERSION`, `dop_write_header`, `dop_read_header`,
  `decimal_from_proto` -> `pub(crate)`. Use `read_dop` / `write_dop`
  for the format I/O and the inherent `Decimal::to_decimal()` method
  on the proto type.
- **Grammar internals demoted.** `LedgerParser` / `HledgerParser`
  (pest-derived) -> `pub(crate)`. `parse_ledger` / `parse_hledger`
  are now `#[cfg(test)] pub(crate)` test-only convenience wrappers.
- **`#[non_exhaustive]` on `ast::ValueExpr` and `ast::AmountDetails`.**
  External `match` arms must include a wildcard `_` arm. 1.x can grow
  new variants without bumping the major version.
- **Examples removed**: `examples/parse_and_print.rs` and
  `load_and_print.rs` (parser-development tools that printed AST
  Debug -- kept the AST visibility wider than the audit recommended).

### Breaking changes -- semantic

- **`Journal::exchange_rate_at` no longer chains through intermediate
  commodities (#158).** It returns a direct or inverse quote only,
  matching Beancount's non-transitive design. Multi-hop FX (e.g.
  EUR -> USD via JPY) returns `None`; consumers who want chaining
  implement it explicitly. Rationale and a comparison with hledger /
  Beancount / ledger-cli is in `docs/exchange-rates.md`.
- The library function previously named `Journal::price_at` is now
  `Journal::exchange_rate_at`.

### Added -- ledger feature parity (Phase D, milestone 8)

Filling in the parity gaps surfaced by the parity test corpus
introduced as part of this release (#138):

- **Bare `D` default-commodity directive** (#142). `D $1,000.00`
  declares a default commodity for amounts that don't carry one.
- **`--exchange` / `-X` CLI flag** (#141) on `balance` and
  `register`. Converts amounts to a target commodity using `P`
  directives. Equivalent to ledger-cli's `--exchange`. If no FX
  path exists for a given commodity, the original amount is kept
  and a warning is printed to stderr.
- **Virtual postings** (#140). `(Account)` is unbalanced (excluded
  from the balance check); `[Account]` is balanced (included in the
  balance check). The new `Posting.kind` enum
  (`REAL` / `VIRTUAL_UNBALANCED` / `VIRTUAL_BALANCED`) carries the
  semantic. CLI gains `-R` / `--real` to exclude virtual postings
  from reports.
- **Lot persistence annotations** (#139). `{cost}`, `[date]`, and
  `((note))` annotations on a posting persist into the elaborated
  journal as a new `Lot` message on `Posting`. `{{total}}` is
  rejected at parse time (was previously silently accepted, then
  miscomputed).
- **Account-directive metadata, denormalised by inheritance**
  (#168). `; type:` and other `; key: value` notes on `account`
  directives flow into a new `metadata` field on
  `AccountProperties`, with metadata inherited down the
  colon-separated hierarchy at compile time. Consumers see a
  fully-resolved per-account map without doing the inheritance walk
  themselves. Used by the web demo's natural-sign heuristic to let
  journals override the default Income/Assets/Liabilities/Equity
  classification.

### Added -- web demo (Phase E, milestone 11)

A working browser demo at `web/`, validating the format-as-API claim
end to end:

- **JS-native `.dop` reader** (#151). Reads `.dop` artifacts in the
  browser via a Buf-generated TypeScript decoder, with no Rust or
  WASM at runtime. Public TS shape mirrors the Rust
  `elaboration::Journal` with `decimal.js` for Decimal and an opaque
  `LocalDate` for dates. Single-pass conversion. 28+ vitest specs
  including a balance-assertion-equivalent round-trip on the
  committed sample.
- **Single-page dashboard** (#150 / #169). KPI strip (Net Worth,
  Cash on Hand, Period Net, Avg Monthly Expense), Income vs Expense
  bars by month, current-month category donut, Net Worth Trend line.
  Pure-function utilities under `web/src/lib/views/`. Filter bar
  (date range + cleared-only) shared across the views.
- **Vue 3 + Vite + TS bootstrap** (#148). Lives at `web/` (subdir
  rather than separate repo for proto-schema co-location). Build
  pipeline runs `buf generate` for TS stubs from
  `proto/doppio.proto`, then vue-tsc + vite build. GitHub Pages
  deploy on push to `main` via `.github/workflows/web.yml`.

### Added -- categorizer crate

- **`doppio-categorize`** graduated from research prototype to
  workspace crate. Public types: `Index`, `Query`, `Suggestion`,
  `Config`, `Normalizer` trait, `DefaultNormalizer`,
  `ScoringStrategy`. Surface counter-account suggestions for new
  transactions based on payee tokenisation + frequency / recency /
  amount weighting. Used by `bb-ledger` and `bookie` import flows.

### Documentation

- **`docs/exchange-rates.md`** -- captures the FX algorithm and
  rationale.
- **`docs/proto-evolution.md`** -- normative wire-format evolution
  rules (additive only, reserved on deprecation, header-version
  bump rules, behaviour preservation). Enforced mechanically by a
  new CI test (`crates/doppio/tests/proto_evolution.rs`) that
  rejects a PR which removes a field without `reserved` covering
  the tag, or which reuses a tag.
- **`proto/doppio.proto` top comment** gains an "Evolution policy"
  section summarising the rules.
- **Repo-wide ASCII pass.** Replaced unicode oddities (em-dashes,
  arrows, ellipses, smart quotes, status emoji, decorative
  box-drawing dividers) with ASCII equivalents across all
  documentation surfaces. Markdown renderers convert `--` back to
  a typographic em-dash, so visual output is preserved while the
  source stays plain ASCII.

### Removed

- The `Journal::price_at` helper (renamed to `exchange_rate_at` -- see
  Breaking changes).
- `parse_ledger` / `parse_hledger` no longer reachable in non-test
  builds; use `compile()` or `Frontend::parse` instead.

### Internal

- **Dependency audit** (#125). Pruned unused features: `serde`'s
  `derive` feature is no longer enabled at the workspace level;
  `chrono`'s `serde` feature dropped; `serde_json` scoped to the
  CLI crate only. Downstream crates that relied on transitively
  enabled features must enable them explicitly.

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
