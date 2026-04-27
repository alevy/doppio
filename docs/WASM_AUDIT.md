# WASM compatibility audit

Last verified: 2026-04-26 against rust 1.95.0, wasm32-unknown-unknown.

This audit catalogs every dependency declared in `Cargo.toml` and reports
its status when targeting `wasm32-unknown-unknown` from the **library**
crate (`cargo build --target wasm32-unknown-unknown --lib`). The `dop`
binary is *not* wasm-clean (it uses `clap`, `std::fs`, `xz`, `serde_json`
for output, etc.) and is not expected to be — only the library surface
needs to be wasm-buildable so downstream consumers (e.g. `doppio-web`)
can embed the parser/elaborator in a browser.

Tracking issue: <https://github.com/alevy/doppio/issues/99>.

## Headline result

With **only** `xz` removed from `[dependencies]`, the entire library
compiles cleanly to `wasm32-unknown-unknown` with no other source or
feature changes:

```text
$ cargo build --target wasm32-unknown-unknown --lib
   Compiling doppio v0.2.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.74s
```

All other current direct dependencies are pure-Rust and wasm-clean as
configured. `xz` is the sole hard blocker, and is already slated for
removal in #100.

## Library deps

| Crate | WASM status | Notes |
|---|---|---|
| `chrono` | works | `default-features = false`, `features = ["serde"]`. The `clock`/`now` features (which need `js-sys` on wasm) are disabled. The crate is used only for `NaiveDate` arithmetic (`Datelike`, `parse_from_str`, `from_ymd_opt`) — no wall-clock APIs. Pure-Rust on wasm. |
| `clap` | works (binary-only in practice) | Pure-Rust crate; compiles fine to wasm. Currently used **only** by `src/main.rs` (the `dop` binary), so it never reaches the library wasm artifact. Recommended next step: move it to a `bin`-only feature gate (e.g. `[features] bin = ["dep:clap"]`) and `optional = true` so the lib build doesn't even resolve it. Not a blocker; a tidy-up. |
| `glob` | works | Pure-Rust. Used in lib via `crate::file_opener` in `src/lib.rs`. The function calls `std::fs::File::open` / `read_to_string`, which compile under `wasm32-unknown-unknown` (they exist in `std` for that target) but will return errors at runtime in a typical browser sandbox. That is acceptable: wasm consumers are expected to supply their own `opener` closure to `parser::Parser` and never call `file_opener`. Documented "what would be needed" if we want stricter hygiene: gate `file_opener` behind `#[cfg(not(target_arch = "wasm32"))]` or a `fs` feature. |
| `pest` | works | Pure-Rust runtime parser. No `std::fs` / time / threads. |
| `pest_derive` | works | Proc-macro; runs at host build time only, never linked into the wasm artifact. |
| `postcard` | works | `features = ["use-std"]` enables `std::io` integration; all of that compiles for wasm. The crate itself is `no_std`-friendly. Used in lib code (`elaboration` round-trip tests) and in the binary. Per #100, postcard is being replaced; in the meantime it is wasm-clean as configured. |
| `regex` | works | Pure-Rust. Used in lib (`parser.rs`, `elaboration.rs`). No optional features that pull in non-wasm code are enabled. |
| `rust_decimal` | works | Pure-Rust arbitrary-precision decimal. Default features are wasm-clean. |
| `serde` | works | `features = ["derive"]`. Both `serde` and `serde_derive` are pure-Rust. |
| `serde-pickle` | works (and **unused in `src/`**) | Pure-Rust pickle (de)serializer. A `grep` of `src/` finds **zero** references to `serde_pickle` / `pickle`. This is a stale dependency. Recommended: drop it in a follow-up. (Not a wasm blocker — it compiles to wasm fine — but it is dead weight in the dep graph.) |
| `serde_json` | works (binary-only in practice) | Pure-Rust. Currently used only in `src/main.rs`. Same recommendation as `clap`: move to a `bin` feature flag. Not a blocker. |
| `xz` | **BLOCKER** | Pulls in `lzma-sys` v0.1.20, which builds the C `liblzma` source via `cc`. The host C toolchain (gcc + glibc headers) is invoked even for the wasm target, and the build fails because liblzma's C sources cannot be compiled against `wasm32-unknown-unknown` without a wasi/clang sysroot. Error: `error: could not compile lzma-sys (lib) due to 2 previous errors`. **Remediation**: tracked in #100 — replace the `.dop` payload's XZ compression with an in-Rust algorithm (e.g. `zstd`'s `ruzstd` pure-Rust decoder, `lz4_flex`, or `miniz_oxide` for deflate). Until #100 lands, the library itself is wasm-clean (`xz` is referenced only in `src/main.rs`); a temporary workaround for downstream consumers is to depend on `doppio` with `default-features = false` once `xz` is moved behind a feature flag. |

## Build-time deps

These run on the **host** during `cargo build` and are never linked into
the wasm artifact, so their wasm compatibility is not relevant. Listed
here for completeness:

| Crate | Role |
|---|---|
| `pest_derive` | proc-macro that expands the PEG grammar at compile time. |
| `serde_derive` (transitive via `serde`'s `derive` feature) | proc-macro. |
| `rust_decimal_macros` (transitive via `rust_decimal` `macros` dev feature) | proc-macro. |
| `thiserror_impl` (transitive via `postcard` → `cobs` → `thiserror`) | proc-macro. |

The issue body also mentions `prost-build` as a future build-time dep
(part of the Phase A wire-format work in #100). It is **not** present in
the current `Cargo.toml` or `Cargo.lock` — this audit applies to the
tree as of commit `4e3baec`.

## Dev-deps

These are only compiled for `cargo test` / `cargo bench` on the **host**
target. They never end up in a wasm artifact built with `--lib`. Spot
check only:

| Crate | Notes |
|---|---|
| `criterion` | host-only benchmark harness; uses threads, files, and `std::time`. Not wasm-relevant. |
| `rust_decimal` (with `macros`) | already covered above; pure-Rust. |
| `tempfile` | host-only test fixture; uses `std::fs`. Not wasm-relevant. |

## Blockers

1. **`xz = "0.1.0"`** — pulls in `lzma-sys`, a C dep that does not cross-compile to `wasm32-unknown-unknown`.
   - **Remediation**: replace XZ in the `.dop` container with a pure-Rust compressor (#100). Candidates:
     - `zstd` via `ruzstd` (pure-Rust decode) + a small encoder, or `zstd` crate with `pure_rust` feature where available;
     - `lz4_flex` (pure-Rust LZ4, fastest);
     - `miniz_oxide` (pure-Rust deflate; smallest, zlib-compatible).
   - **Interim mitigation**: gate `xz` behind a non-default `cli`/`compression-xz` feature so library consumers can opt out today:
     ```toml
     [features]
     default = ["cli"]
     cli = ["dep:xz", "dep:clap", "dep:serde_json"]

     [dependencies]
     xz = { version = "0.1.0", optional = true }
     clap = { version = "4.5.59", features = ["derive"], optional = true }
     serde_json = { version = "1", optional = true }
     ```
     Then move the `xz` and `clap` / `serde_json` `use` lines in `src/main.rs` (and the `dop` `[[bin]]` itself) under `#[cfg(feature = "cli")]`. This is a Cargo.toml + binary-side change only; the library never references `xz`. Documented here as "what would be needed" — not applied, per the read-only-src constraint of this audit.

No other blockers were observed.

## Hygiene findings (non-blocking)

Picked up during the audit; recommend opening separate small PRs:

- **`serde-pickle` is unused.** No references in `src/` or `tests/`. Drop it from `[dependencies]`.
- **`clap` and `serde_json` are binary-only.** Move both to `optional = true` and gate behind a `cli` feature so the published library has a smaller surface.
- **`glob` + `file_opener` and the wasm story.** `file_opener` is a *convenience* function that embeds `std::fs` calls. It compiles under wasm but cannot run usefully in a browser sandbox. Consumers should pass their own opener to `parser::Parser`. Optional cleanup: `#[cfg(not(target_family = "wasm"))]`-gate `file_opener` (and the `glob` dep) so the wasm `doppio` rlib doesn't carry it at all.

## Method

Reproducing the audit on a future dep set:

```sh
# 1. Make sure rustup + the wasm target are available.
nix shell nixpkgs#rustup -c rustup target add wasm32-unknown-unknown

# 2. Build the library only — that is the surface that needs to be wasm-clean.
nix shell nixpkgs#rustup -c cargo build --target wasm32-unknown-unknown --lib
```

If the build fails, the cargo error names the offending crate (typically
a `*-sys` crate that wraps a C library, or a crate with a hard
`std::os::unix` dependency). Workflow per offender:

1. Identify whether the crate is used by the **lib** or only by the **bin** / tests.
   - `grep -rn '<crate_name>' src/` is enough; if it appears only in `src/main.rs` or `src/bin/`, it is binary-only.
2. If binary-only: fix is purely declarative — make the dep `optional = true` and put it behind a `cli` (or similar) feature, then `#[cfg(feature = "cli")]` the binary. The lib build will stop pulling it in.
3. If lib-side: try `default-features = false` first; many crates have a `std`/`alloc` split that drops the offending C bits.
4. If neither works: the crate is a hard blocker. Either find a pure-Rust replacement, or expose the affected feature behind a Cargo feature flag so wasm consumers can opt out.

To workaround a known blocker (e.g. `xz` today) and audit the rest of
the tree, comment out the offending line in `Cargo.toml` temporarily and
re-run step 2. The error from the next-failing crate (if any) tells you
what else to investigate. Restore `Cargo.toml` when done — this audit
does that.

For deeper investigation, `cargo tree --target wasm32-unknown-unknown`
shows the full transitive graph as resolved for the wasm target, which
is useful when a transitive crate (rather than a direct dep) is the
source of trouble.
