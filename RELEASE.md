# Releasing doppio

This document describes how to cut a new tagged release and publish it to
crates.io. The process is currently manual; automating it on tag push is a
post-v0.2 follow-up.

## Versioning

doppio follows [SemVer](https://semver.org/) with the usual pre-1.0 caveat:
breaking changes bump the **minor** version. Bug fixes and additive,
non-breaking changes bump the **patch** version.

Breaking surfaces include:

- The library API exposed from `lib.rs` (functions, public types, public
  trait impls).
- The `.dop` binary format (`DOP_FORMAT_VERSION`).
- The CLI's flag/subcommand contract — removing or renaming a flag is
  breaking; adding one is additive.

The MSRV is declared in `Cargo.toml` as `rust-version`. Bumping it is a
minor-version-only change.

## Pre-release checklist

In dependency order. Each is a separate commit on a `release/v$NEW` branch.

1. **Cargo.toml** — bump `version`. Bump `rust-version` only if MSRV
   actually changed (release notes the bump).
2. **CHANGELOG.md** — replace the leading `## [Unreleased]` heading with
   `## [$NEW] - YYYY-MM-DD` (today's ISO date). Add a fresh empty
   `## [Unreleased]` heading above. Audit recent commits to ensure no
   notable change was missed.
3. **README.md** — sync any feature descriptions, CLI flag listings, or
   library examples affected by the release. The "Supported Ledger
   features" table should still be accurate.
4. **docs/SUPPORTED_FEATURES.md** — bump the "Last updated" stamp; flip
   any `🔧 Partial` / `🚫 Not supported` rows that became supported, and
   add new rows for any new syntax/CLI surface.
5. **docs/requirements.md** — bump "Last updated"; flip any REQ-GAP
   markers that closed.

## Local quality gate

Run all of these before pushing the release branch. CI runs the first three.

```sh
cargo fmt --check
cargo clippy -- -D warnings        # matches CI; lib + bin
cargo test
cargo test --doc
cargo doc --no-deps                # see "Known doc warnings" below
```

(`cargo clippy --all-targets -- -D warnings` flags additional lints in
benches/examples/tests; those are pre-existing dev-time issues outside the
shipped surface and do not gate releases.)

### Known doc warnings (as of v0.2.0)

`cargo doc --no-deps` currently emits 9 warnings: a handful of unresolved
intra-doc links (`elaboration::Journal`, `ast::Journal`, `ast::ValueExpr`),
the documented-private-item flags for `PRATT_PARSER` and the `evaluator`
submodule, and the ambiguity between the `Parser` struct and the
`pest_derive::Parser` macro in scope. None are introduced by v0.2.0; they're
pre-existing tech debt. Tracking issue: TBD (file post-release).

## End-to-end smoke test

Against a real downstream books fixture. The reference target is
`betterbytes-org/ledger` (clone or use a checked-in subset).

```sh
cargo build --release

# Source-path entry point — exercises path resolution and the full pipeline.
./target/release/dop balance /path/to/bb-ledger/accounts/books.ledger
# Expect: exit 0, full balance report on stdout, only legitimate `tag check`
# warnings on stderr (data-driven, not a doppio bug).

# .dop round-trip — exercises serialisation and the .dop reader.
./target/release/dop compile /path/to/bb-ledger/accounts/books.ledger -o /tmp/bb.dop
./target/release/dop balance /tmp/bb.dop
# Expect: same balance report.

# Manual CLI sweep on a small fixture:
for cmd in balance register print stats accounts commodities; do
    ./target/release/dop $cmd /path/to/sample.ledger > /dev/null \
        && echo "OK: $cmd" || echo "FAIL: $cmd"
done
```

## `.dop` v1 → v2 rejection

Verify that a `.dop` file produced by an older release fails cleanly with a
clear "recompile" message rather than mis-parsing. A v0.1.0-built fixture
lives at `crates/doppio-cli/tests/fixtures/v1.dop`:

```sh
./target/release/dop balance crates/doppio-cli/tests/fixtures/v1.dop
# Expect: an Err mentioning the version mismatch and pointing at `dop compile`.
```

When the format version bumps in a future release, regenerate this fixture
from the previous release's binary.

## Dry-run publish

```sh
cargo publish --dry-run
```

Surfaces missing files, license issues, README rendering. Must exit 0 before
the real publish.

## Publish

The order matters: publish first, tag only on success. A failed
`cargo publish` left after a tag-push is awkward to clean up.

```sh
git checkout main && git pull            # release PR merged first
cargo publish                            # the real thing — irreversible
git tag -a v$NEW -m "Release v$NEW"
git push origin v$NEW
```

## GitHub release

```sh
gh release create v$NEW \
    --title "v$NEW" \
    --notes "$(awk '/^## \['$NEW'\]/,/^## \[/' CHANGELOG.md | sed '$d')"
```

Or paste the relevant `CHANGELOG.md` section manually.

## Verify

1. `cargo install doppio` from a fresh shell installs the new version.
2. `dop --version` reports the new version.
3. <https://crates.io/crates/doppio/$NEW> renders correctly (description,
   keywords, categories, README).
4. Downstream consumers (`bookie`, `better-bytes-ledger-import`) can
   `cargo update -p doppio` and rebuild.
