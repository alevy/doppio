# doppio: Supported Ledger features

**Last updated**: 2026-04-29 (doppio v0.4.0-rc.1)

This document is a feature-by-feature comparison of doppio's syntax surface
against [ledger-cli](https://ledger-cli.org/) and [hledger](https://hledger.org/).
The authoritative behaviour is the test suite — this matrix is a navigation aid.

The companion **parity test corpus** at
[`crates/doppio/tests/parity/`](../crates/doppio/tests/parity/) carries one
minimal `.ledger` fixture per feature plus a Rust harness asserting on the
elaborated `Journal`. Tests for not-yet-implemented features are
`#[ignore]`'d with a tracking-issue reference, so the ignored count stays a
visible signal of the remaining parity work toward 1.0 (Phase D).

Status legend:

- ✅ **Supported** — parses and elaborates; behaves equivalently to ledger-cli for the cases this project has tested
- 🔧 **Partial** — parses but with the noted limitations
- 🚫 **Not supported** — rejected, ignored, or out of scope

## Transactions and postings

| Feature | Status | Notes |
|---|---|---|
| Date header `YYYY-MM-DD` / `YYYY/MM/DD` | ✅ | Four-digit year required |
| Secondary date `=YYYY-MM-DD` | ✅ | Stored on `ResolvedTransaction::secondary_date` |
| State markers `*` (cleared), `!` (pending) | ✅ | |
| Code in parentheses `(CODE)` | ✅ | |
| Description / payee | ✅ | |
| Postings (indented) | ✅ | |
| Number-first amounts `100 USD` | ✅ | |
| Symbol-first amounts `$100` | ✅ | |
| Bare amounts `100` | ✅ | Default commodity applied if declared |
| Negative amounts | ✅ | Both `-$100` and `$-100` |
| Lot pricing `@ unit` | ✅ | |
| Lot pricing `@@ total` | ✅ | |
| Null posting (auto-inferred amount) | ✅ | Exactly one per transaction; multiple null postings is an error |
| Posting balance assertion `= amount` | ✅ | Enforced during elaboration |
| Strict balance assertion `== amount` | ✅ | Enforced during elaboration |
| Balance assignment `=target` (with commodity inference) | ✅ | v0.2.0: infers commodity from account balance, same-transaction context, or the default-commodity directive |
| Effective dates beyond secondary | 🚫 | Only the primary + secondary dates are modelled |

## Comments and metadata

| Feature | Status | Notes |
|---|---|---|
| Line comments (`;`, `#`, `*`, `%`, `\|`) | ✅ | Full-line comments at top level |
| Transaction-header notes | ✅ | Indented `;` lines under the transaction header |
| Posting notes | ✅ | Indented `;` lines under a posting |
| Metadata `; key: value` | ✅ | Stored on the carrier (transaction or posting) |
| Bare tag list `; :tag1:tag2:` | ✅ | Stored as a `Vec<String>` |
| Transaction metadata inherited by postings (for `tag()` lookup) | ✅ | v0.2.0 (PR #96): a posting that has no `Foo: ...` of its own sees the transaction's `Foo`. Posting-level wins on collision. |

## Top-level directives

| Feature | Status | Notes |
|---|---|---|
| `include <path>` (literal) | ✅ | |
| `include <glob>` (e.g. `*.ledger`, `**/*.ledger`) | ✅ | v0.2.0 (PR #75). Lexicographic order; zero-match globs error |
| `!` prefix on directives (`!include`) | ✅ | Treated as a hint, parsed identically to the bare form |
| `account NAME` block | ✅ | |
| `account` + `note` sub-directive | ✅ | |
| `account` + `assert <expr>` | ✅ | v0.2.0 (PR #76). Fatal: posting whose elaboration fails an account-level assert halts the run with `ElaborationError::AccountAssertionFailed` |
| `account` + `check <expr>` | ✅ | v0.2.0 (PR #76). Non-fatal: prints a warning to stderr and continues |
| `account` + `alias` sub-directive | ✅ | Resolves through the parse → resolve → elaborate pipeline; covered by `tests/parity/account_alias_*.ledger` (single, multiple-per-block, across-blocks, with-assert, forward-only) |
| `commodity SYMBOL` block | ✅ | |
| `commodity` + `format` sub-directive | ✅ | v0.2.0 (PR #84). Drives `balance` and `register` rendering (prefix/suffix, thousands separator, decimal places, sign) |
| `commodity` + `default` sub-directive | ✅ | v0.2.0. Equivalent to a `D` directive |
| `commodity` + `nomarket` sub-directive | ✅ | v0.2.0. Stored as a flag (currently no FX conversion to suppress) |
| `commodity` + `note` sub-directive | ✅ | v0.2.0 |
| `tag NAME` block (declaration) | ✅ | |
| `tag` + `assert <expr>` (with `value` binding) | ✅ | v0.2.0 (PR #87). Validates every posting and transaction metadata pair carrying that tag |
| `tag` + `check <expr>` | ✅ | v0.2.0 (PR #87). Non-fatal warning |
| `define name = expr` (zero-arg alias) | ✅ | |
| `define name(p1, p2, ...) = expr` (parameterised) | ✅ | v0.2.0 (PR #87). Supports both value-typed and bool-typed bodies. Cyclic definitions are caught with `RecursionLimitExceeded` |
| `alias short = long` | ✅ | Account-name aliases |
| `P <date> <commodity> <price>` (historical price) | ✅ | Parsed and stored. Rendering / FX conversion not yet applied |
| Standalone balance-assertion directive `<date> = account amount` | ✅ | Enforced during elaboration |
| `D $1000.00` (default commodity) | ✅ | Both the bare-`D` form and the `commodity ... default` form are supported; bare `D` is lowered at parse time to the same `Directive::Commodity` representation |
| `~` budget directive | 🚫 | Parsed but intentionally not elaborated. No effect on balances or reports |
| `= payee` automated transactions | 🚫 | Not modelled |

## Expression language

(Used in posting amounts, balance assertions, and `assert`/`check` directives.)

| Feature | Status | Notes |
|---|---|---|
| Arithmetic `+`, `-`, `*`, `/` | ✅ | Pratt-parser precedence (`*` / `/` bind tighter than `+` / `-`) |
| Unary `-`, `+` | ✅ | |
| Parenthesised arithmetic | ✅ | |
| Parenthesised boolean expressions | ✅ | v0.2.0 (PR #94). E.g. `(amt > 0 or (tag("X") =~ /pat/))` |
| Numeric comparisons `==`, `!=`, `<`, `>`, `<=`, `>=` | ✅ | |
| Regex match `=~` and not-match `!~` | ✅ | v0.2.0 (PR #86). Patterns compiled at parse time |
| Boolean `and`, `or` | ✅ | Left-to-right; full Pratt precedence is a known limitation (#74-followup) |
| String literals `"text"` | ✅ | |
| Regex literals `/pattern/` | ✅ | v0.2.0 |
| Commodity-typed expressions `(expr) USD` | ✅ | |
| Function calls — built-ins: `tag(name)`, `account(name)`, `scrub(x)` | ✅ | v0.2.0 added `tag()` |
| Function calls — user-defined via `define` | ✅ | v0.2.0 (PR #87) |
| `value` binding inside `tag NAME` `assert`/`check` | ✅ | v0.2.0 (PR #87) |
| Lisp-style ledger expressions | 🚫 | Not in the grammar |

## CLI

| Subcommand / flag | Status | Notes |
|---|---|---|
| `dop compile -o OUT SRC` | ✅ | Writes a `.dop` v2 binary |
| `dop balance` | ✅ | Tree output by default; `--flat` for flat |
| `dop balance --depth N` | ✅ | |
| `dop balance --begin DATE` / `--end DATE` | ✅ | |
| `dop balance --cleared` | ✅ | |
| `dop balance --tag KEY` | ✅ | v0.2.0 (PR #72) |
| `dop balance --pattern REGEX` | ✅ | Account-name regex |
| `dop balance --format text\|json\|csv` | ✅ | |
| `dop register` | ✅ | Per-posting register with running totals per commodity |
| `dop register --begin/--end/--cleared/--tag/--format` | ✅ | Same filters as `balance` |
| `dop print SRC` | ✅ | Re-emits source. Format strings not applied (intentional) |
| `dop stats` | ✅ | Transaction/account/commodity counts and date range |
| `dop accounts` | ✅ | Lists unique account names |
| `dop commodities` | ✅ | Lists unique commodity symbols |
| Query DSL `--limit "expr"` | 🚫 | Phase 4 / issue #45 |

## Library API

| Surface | Status | Notes |
|---|---|---|
| `compile(source, parser)` | ✅ | Source text → fully elaborated `Journal` |
| `eval_transaction(txn, context)` | ✅ | Single-transaction elaboration |
| `write_ledger(txns, writer)` | ✅ | Canonical Ledger source-text output |
| `dop_write_header` / `dop_read_header` | ✅ | Portable `.dop` header I/O with version-mismatch errors |
| `resolution::Transaction` / `Posting` builder API | ✅ | Fluent construction with `with_*` helpers |
| `parser::Parser<F>::opener` returning `Result` | ✅ | v0.2.0 breaking change. Custom openers can surface I/O errors |
| `.dop` binary format v2 (`elaboration::Journal` with `commodities`) | ✅ | v0.2.0 breaking change. v1 files are rejected with a clear "recompile" message |
| Append / framed `.dop` (range-scan, partial decode) | 🚫 | Phase 4 / issues #17, #39–41 |

## hledger frontend

Added in v0.3.0 (issue #103). Recognised by the `.hledger` and `.journal`
file extensions; selected automatically by `dop` and by
`doppio::frontend_for_extension`.

### Parity with the ledger-cli frontend

| Feature | Status | Notes |
|---|---|---|
| Transactions, cleared/pending state, code, description | ✅ | |
| Postings with two-space rule | ✅ | Same convention as ledger-cli |
| Number-first amounts `100 USD` | ✅ | |
| Symbol-first amounts `$100` | ✅ | |
| Negative amounts `-$110`, `$-110` | ✅ | |
| Null posting (auto-inferred amount) | ✅ | |
| Lot pricing `@ unit` / `@@ total` | ✅ | |
| Balance assertion `= amount` (single-commodity) | ✅ | |
| Strict balance assertion `== amount` (all-commodity) | ✅ | |
| Balance assignment `= target` | ✅ | |
| Transaction notes / posting notes (`;` lines) | ✅ | |
| `P` historical price directive | ✅ | Time component not parsed (hledger omits it) |
| `account` directive with inline note | ✅ | |
| `account` directive with indented sub-directives | ✅ | `note`, `alias`, `type`, unknown keys |
| `commodity` directive (format string) | ✅ | Both `$1,000.00` and `1,000.00 EUR` forms |
| `commodity` directive with indented sub-directives | ✅ | `alias`, `format`, `nomarket`, `default`, `note` |
| `D <amount>` (bare default-commodity directive) | ✅ | Same shape as ledger-cli's `D`; lowered to `Directive::Commodity { Default, Format }` |
| `include` directive (literal and glob) | ✅ | Same glob expansion as ledger-cli frontend |
| Arithmetic in posting amounts | ✅ | Pratt-parsed; same precedence as ledger-cli |
| Comment lines `;` | ✅ | |
| Comment lines `#` | ✅ | hledger extension; not accepted by ledger-cli frontend |
| Date format `YYYY-MM-DD` | ✅ | |
| Date format `YYYY/MM/DD` | ✅ | hledger extension |
| Date format `YYYY.MM.DD` | ✅ | hledger extension |
| Periodic transactions `~` | ✅ | Parsed but not elaborated (same as ledger-cli `~` budget) |

### Known limitations

| Feature | Status | Notes |
|---|---|---|
| Automated posting `*N` arithmetic bodies | 🚫 | TODO(#103). The auto-rule shape is parsed but postings with `*N` multipliers cause a parse error. |
| `comment` / `end comment` block comments | 🚫 | Not yet supported |
| `Y year` directive (date inference) | 🚫 | Full four-digit year required in all dates |
| Per-transaction `= DATE` effective date | 🚫 | hledger uses a different secondary-date syntax; not supported |
| Multiple commodities per posting | 🚫 | v1 limitation shared with ledger-cli frontend |
| `assert` / `check` in account sub-directives | 🚫 | Parsed but not enforced (hledger's semantics differ from ledger-cli's) |

## Out of scope (explicitly)

These ledger-cli features are not modelled by doppio and aren't planned:

- `~` budget directives (parsed but ignored)
- `= payee` automated transactions (automated posting rule arithmetic bodies)
- Third-or-later effective dates in transaction headers
- ledger-cli's Lisp-style scripting / Python integration
- Real-time market-price-driven FX conversion in balance reports

## Reporting gaps

If you find a ledger-cli or hledger construct that doppio rejects (or accepts
but elaborates wrong) and it isn't documented here, please open an issue at
<https://github.com/alevy/doppio/issues> — small minimal-failing-case
snippets are especially welcome.
