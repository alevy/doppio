# doppio Requirements

**Last updated**: 2026-05-09 (doppio v2.2.0)

---

## How to use this document

This document answers **what doppio needs to do and why** — grounded in downstream consumer analysis, open issues, and design decisions. GitHub issues answer **what specific work is being done and whether it's done**.

- **Before starting any issue** that touches the library API or CLI behavior: read the relevant section here for context and constraints.
- **For API design decisions**: the canonical API surface (§1, §3) is authoritative — e.g., callers construct `resolution::Transaction`, not `ast::Transaction`.
- **Update this doc via PR** when requirements change: new downstream discovered, design decision made, or post-milestone retrospective. Do not update it as a changelog.
- **REQ-GAP status markers**: ✅ Done · 🔧 #N open issue · ⏳ Phase N deferred · 🚫 Won't do

---

## Executive Summary

doppio is a multi-stage Rust compiler and CLI for the Ledger plain-text accounting format. After Milestones 2 and 3, the library will expose ergonomic APIs for programmatic journal construction and evaluation, and the CLI will support complete balance sheet filtering and reporting. The system will enforce balance assertions during elaboration and provide structured query output (JSON, CSV) alongside the default text format.

**Primary users**:
1. **Library users**: Developers embedding doppio to build accounting applications, tax tools, or import scripts
2. **CLI users**: Accountants and individuals querying ledger files via command line
3. **Integration users**: Systems importing ledger data from bank APIs or other sources, then writing validated ledgers

**Confirmed downstream consumers**:
- `alevy/better-bytes-ledger-import` — imports Mercury bank transactions, Gusto payroll CSVs, and recognizes grant revenue. Constructs `resolution::Transaction` / `resolution::Posting` values; reads `elaboration::Journal` for deduplication.
- `alevy/bookie` — imports SimpleFIN bank transactions using `ast::Transaction` / `ast::Posting` for construction (not the resolution layer); reads `elaboration::Journal` for deduplication. Uses `ast::ValueExpr::parse()` for YAML-configured amount expressions.
- `betterbytes-org/ledger` *(future migration target)* — the actual Better Bytes accounting books, currently using OG ledger-cli. Requires `!include` globs, account/tag/commodity directives, and an invoice generation workflow (Python + Typst + ledger-cli subprocess calls) to be ported to doppio. See §5.1.3.

---

## 1. Library API Requirements (Milestone 2: Foundation)

### 1.1 Programmatic Transaction Construction (Issues #28–#31)

**Current State**: The library provides builder methods on `resolution::Transaction` and `resolution::Posting`, enabling fluent construction:

```rust
let txn = resolution::Transaction::new(date, "Payee")
    .with_posting(
        resolution::Posting::new("Assets:Checking")
            .with_amount(100)
    )
    .with_posting(
        resolution::Posting::new("Expenses:Food")
    );
```

**Completed** (#28, #29):
- `Transaction::with_posting()`, `with_tag()`, `with_comment()`, `with_metadata()`
- `Posting::with_tag()`, `with_comment()`, `with_metadata()`, `with_amount()`
- Typed amount shorthand (numeric literal conversion to `ast::AmountDetails`)

**Remaining Requirements** (Issues #30, #31):

**REQ-M2-001**: **`write_ledger<W: Write>(&mut W, &[resolution::Transaction]) -> Result<()>`** *(#30)*
- Convert a slice of `resolution::Transaction` to canonical Ledger source format
- Write to any `Write` sink (file, string buffer, stdout)
- Preserve transaction metadata, tags, and comments in output
- **Rationale**: Users who construct transactions programmatically need to serialize them back to source form for validation, review, or persistence.
- **Evidence**: The `print` subcommand already implements similar logic; #30 asks for a reusable library function.

**REQ-M2-002**: **`eval_transaction(txn: &resolution::Transaction, context: &resolution::Context) -> Result<elaboration::ResolvedTransaction>`** *(#31)*
- Evaluate a single transaction: resolve aliases, evaluate amount expressions, balance postings, apply cost basis
- Return a fully resolved transaction or an error (e.g., unbalanced, expression evaluation failure)
- Accept a `Context` to supply alias definitions and default commodity
- **Rationale**: Users need a bridge between the programmatic construction API and full elaboration without requiring a full `HIR` and elaboration::Journal. Useful for:
  - Single-transaction validation (e.g., import validation)
  - Rapid iteration during transaction entry (parse, validate, store)
  - Integration tests and transaction introspection
- **Evidence**: Milestone 2 roadmap lists this as a core ergonomic improvement.

**Design notes**:
- Both functions should integrate cleanly with the existing 4-stage pipeline
- `eval_transaction` likely creates a minimal `HIR` internally, runs elaboration, and extracts the result
- `write_ledger` should produce output that round-trips: `write_ledger(txns)` → parse → should yield equivalent transactions

---

### 1.2 Builder Ergonomics & Type Safety

**REQ-M2-003**: **Posting with typed amount shorthand**
- `Posting::with_amount(100)` should accept `Into<ast::AmountDetails>`
- Support: integers, `Decimal`, `(Decimal, &str)` tuples for commodity, and raw `ast::AmountDetails`
- **Status**: Completed in #29

**REQ-M2-004**: **Secondary date support in builder**
- `Transaction::with_secondary_date()` for optional processing date
- **Status**: Already implemented; appears in code at resolution.rs:240–244

**REQ-M2-005**: **Code and state setters in builder**
- `Transaction::with_code()`, `with_state()` for optional transaction code and cleared/pending state
- **Status**: Already implemented

---

## 2. Balance Assertion Requirements (Milestone 3: Issue #12)

### 2.1 Assertion Directive Parsing & Resolution (Issue #36 – COMPLETED)

**REQ-M3-001**: **Parse standalone balance assertion directives**
- Syntax: `YYYY-MM-DD [==|=] Account  AMOUNT`
- Example: `2024-01-15 = Assets:Checking  $1000.00`
- Distinguish strict (`==`) from weak (`=`) semantics
- **Status**: Completed in PR #55 (commit 9c7ca4f)
- **Evidence**: `ast::AssertionDirective`, `resolution::AssertionDirective` types added; parsed through pest grammar

**REQ-M3-002**: **Thread assertions through resolution stage**
- Convert `ast::Assertion` to `resolution::Entry::Assertion`
- Resolve dates to `NaiveDate`
- Keep amount expressions unevaluated for elaboration
- **Status**: Completed; `resolution::AssertionDirective` carries `date`, `account`, `amount: ast::ValueExpr`, `strict: bool`

---

### 2.2 Assertion Enforcement During Elaboration (Issue #37)

**REQ-M3-003**: **Enforce standalone balance assertions**
- During elaboration, after processing each transaction, check any assertions for that date/account
- Compare `account.balance(commodity)` against the assertion's expected amount
- Error if assertion fails: `ElaborationError::BalanceAssertionFailed`
- **Status**: TODO at elaboration.rs:277
- **Blocking on**: Issue #37

**REQ-M3-004**: **Assertion semantics and error reporting**
- **Weak balance assertion** (`=`): Asserts that the account balance equals the given amount (in a single commodity)
  - Behavior: After the posting, the account should have exactly this balance
  - Error if balance ≠ amount
- **Strict balance assertion** (`==`): (exact semantics TBD; likely strict equality without tolerance)
  - Placeholder: assume identical to weak for now
- **Multi-commodity handling**: When an assertion specifies a single-commodity amount (e.g., `$1000`):
  - Fail if account holds other commodities (strict interpretation)
  - Or: succeed if the specified commodity matches exactly (lenient interpretation)
  - **Design decision needed**: Issue #37 should clarify this
- **Error context**: When an assertion fails, report:
  - Expected amount
  - Actual balance per commodity
  - Account name and date
- **Status**: Not yet implemented

**REQ-M3-005**: **No balance assertions in compiled `.dop` files**
- Balance assertions are checked at elaboration time but not persisted in the serialized `elaboration::Journal`
- Deserialized `.dop` files cannot replay assertions
- **Rationale**: Assertions are validation artifacts, not data to carry forward
- **Status**: Implied by architecture; not yet tested

---

### 2.3 Balance Assertion Tests (Issue #38)

**REQ-M3-006**: **Test suite for balance assertion enforcement**
- Cases:
  1. Weak assertion succeeds: balance matches
  2. Weak assertion fails: balance mismatch
  3. Multi-commodity account with single-commodity assertion (edge case)
  4. Assertion at transaction boundary
  5. Sequential assertions (each checks balance *after* previous)
  6. Assertion with unevaluated expression (e.g., `= 100 + 50`)
- **Status**: Not yet implemented; blocked on #37

---

## 3. CLI Requirements (Milestone 3: Issue #21)

### 3.1 Balance Command Enhancements (Issue #21 – IN PROGRESS)

**Completed** (#32, #34, #35):
- `--begin DATE` / `--end DATE` for date range filtering
- `--depth N` to truncate account hierarchy
- Tree output mode (default) with indentation
- `--flat` for single-line output
- Regex pattern filtering on account names

**Remaining Requirements** (Issue #33):

**REQ-M3-007**: **`balance --cleared` flag**
- Filter to only transactions with `*` (cleared) state
- Exclude pending (`!`) and uncleared transactions
- Combine with `--begin`, `--end`, and account patterns for precise filtering
- **Status**: Partially implemented; `cleared: bool` field exists in main.rs but may not be wired correctly
- **Evidence**: main.rs:52 shows flag definition; implementation at main.rs:467–470

**REQ-M3-008**: **Output formats for balance command**
- Text (default): current tree/flat format with right-aligned amounts
- JSON: structured output with account hierarchy and per-commodity balances
- CSV: one row per (account, commodity) pair with amount
- Flag: `--format text|json|csv` (default: text)
- **Status**: Completed in PR #53 (ef7d0c8)
- **Evidence**: OutputFormat enum and format-specific match arms at main.rs:508–566

---

### 3.2 Register Command Enhancements (Issue #22)

**REQ-M3-009**: **Register filtering improvements**
- Account name regex filtering (like balance): `register [PATTERN]`
- **Status**: Already implemented; accepts optional pattern argument at main.rs:70–76

**REQ-M3-010**: **Register output formats**
- Text (default): date, description, account, amount, running total per commodity
- JSON: structured array with per-commodity entries
- CSV: RFC 4180 format, one row per posting per commodity
- **Status**: Completed in PR #53; match arms at main.rs:234–336

**REQ-M3-011**: **Register date range filtering** *(deferred to Phase 4?)*
- `--begin` and `--end` flags (like balance)
- **Status**: Not yet implemented; likely deferred to later milestone
- **Evidence**: Issue #22 mentions it but no implementation in code

**REQ-M3-012**: **Register cleared-only mode** *(deferred to Phase 4?)*
- `--cleared` flag to filter to cleared transactions only
- **Status**: Not implemented; likely deferred
- **Evidence**: Not mentioned in main.rs register command

---

## 4. Non-Functional Requirements

### 4.1 Performance & Scalability

**REQ-NF-001**: **Parse large journals efficiently**
- Target: 100k+ transactions should parse in < 5 seconds on commodity hardware
- **Evidence**: Benchmarks exist in `crates/doppio/benches/parse.rs`, `crates/doppio/benches/pipeline.rs`; compiled binary should be optimized with `--release`

**REQ-NF-002**: **Compiled `.dop` files deserialize rapidly**
- Deserialization should be order-of-magnitude faster than parsing source
- **Target**: 100k transactions in < 100ms
- **Status**: XZ decompression + postcard deserialization; performance TBD

**REQ-NF-003**: **Minimal memory footprint for queries**
- Balance and register commands should not require re-compiling the journal
- Load from `.dop` once, execute multiple queries without re-parsing
- **Evidence**: CLI design accepts both `.ledger` and `.dop`

---

### 4.2 Data Integrity & Correctness

**REQ-NF-004**: **Double-entry invariant enforcement**
- Every transaction must balance (sum of postings = 0 per commodity)
- Error on unbalanced transactions with clear diagnostic
- **Status**: Implemented in elaboration; `ElaborationError::TransactionDoesNotBalance(Amount)` carries actual imbalance

**REQ-NF-005**: **Deterministic transaction balancing**
- Null-posting inference is deterministic: exactly one null posting inferred as negation of rest
- Multi-pass elaboration produces same output given same input
- **Evidence**: Two-pass approach in elaboration.rs

**REQ-NF-006**: **Commodity consistency**
- Account aliases and commodity aliases resolve consistently within their context
- Error on conflicting definitions (e.g., alias A→B and later A→C)
- **Status**: Contexts form immutable history; conflicts allowed in different contexts; within a context, BTreeMap prevents duplicates

**REQ-NF-007**: **Date stability**
- Partial dates (missing year) resolve via fallback year or error
- All dates stored as `NaiveDate` after resolution
- Epoch-day representation `i32` is stable: 1970-01-01 = 0, 2100-01-01 ≈ 47482
- **Status**: Implemented; test at resolution.rs:642–677

---

### 4.3 Usability & Developer Experience

**REQ-NF-008**: **Clear error messages**
- Parse errors: source location, problem, suggestion
- Elaboration errors: account name, expected vs. actual balance, date
- Builder usage errors: type mismatches caught at compile time where possible
- **Evidence**: ElaborationError and ResolutionError enums define error cases; Display impl at elaboration.rs:246–251 is minimal (TODO)

**REQ-NF-009**: **API documentation**
- All public types and functions in lib.rs, resolution.rs, elaboration.rs have `///` doc comments
- Examples show typical workflows (load, compile, query)
- **Status**: Largely complete; `cargo doc --no-deps` generates full API docs

**REQ-NF-010**: **Round-trip fidelity for source files**
- Ledger files loaded, compiled, printed should maintain semantic equivalence
- Comments, tags, and metadata preserved
- **Status**: Partially; comments preserved in resolution (issue #5), but `.dop` deserialization cannot restore source form

---

### 4.4 Interoperability & Formats

**REQ-NF-011**: **Support multiple output formats**
- CLI balance and register: text, JSON, CSV
- **Status**: Completed in PR #53

**REQ-NF-012**: **CSV RFC 4180 compliance**
- Proper escaping of quotes, commas, and newlines
- **Evidence**: Utility function `csv_field()` at main.rs:584–590

**REQ-NF-013**: **JSON schema stability**
- Balance JSON: `{account, balances: [{commodity, amount}]}`
- Register JSON: `{date, description, account, commodity, amount, running_total}`
- Schema should be documented and stable across versions
- **Status**: Implemented; not yet documented in schema form

---

## 5. Gaps & Implicit Requirements

### 5.1 Downstream Consumer Needs (Analysis of bookie & better-bytes-ledger-import)

Both `alevy/bookie` and `alevy/better-bytes-ledger-import` were read via GitHub API in April 2026. Both use doppio as a local path dependency. The analysis below documents what each downstream currently does, assesses whether that reflects genuine requirements or an accidental API choice, and recommends the canonical API each should target.

**Canonical construction API: `resolution::Transaction`**

`resolution::Transaction` is the correct and intended API for programmatic transaction construction. It already covers every field both downstreams need: `date`, `secondary_date`, `state: ast::TransactionState`, `code`, `description`, `metadata: BTreeMap<String, String>` (output as `; key: value` lines), `comments: Vec<String>`, `tags`, `postings`, `Default`, `Display`. Callers should not construct `ast::Transaction` directly for output purposes — that type is the raw parse tree and an implementation detail of the parser.

---

#### 5.1.1 bookie

`bookie` imports SimpleFIN bank transactions using YAML-configured account mapping rules.

**What bookie currently does** (uses `ast::Transaction` — to be migrated):

```rust
// Currently uses ast layer — accidental, not a genuine requirement
doppio::ast::Transaction {
    date: naive_date.into(),
    state: doppio::ast::TransactionState::Cleared,
    notes: vec!["sfin_org: domain".into(), "sfin_txn: id".into()],
    postings: vec![doppio::ast::Posting::new(account).with_amount((amount, &currency))],
    ..
}
```

**Target API** (what bookie should use after migration):

```rust
doppio::resolution::Transaction::new(naive_date, description)
    .with_state(doppio::ast::TransactionState::Cleared)
    .with_metadata("sfin_org", &account.org.domain)
    .with_metadata("sfin_account", &account.id)
    .with_metadata("sfin_txn", &transaction.id)
    .with_posting(
        doppio::resolution::Posting::new(account_name)
            .with_amount((amount, &currency))
    )
```

The `resolution::Transaction` `Display` impl already outputs `; key: value` lines for metadata — exactly what bookie needs for round-trip deduplication checks. The transition from `ast::Transaction.notes: Vec<String>` to `resolution::Transaction.metadata: BTreeMap` is actually an improvement: the structured map enforces the `key: value` shape that bookie relied on by convention.

**Genuine requirements confirmed from bookie**:

1. **Compile + deduplicate** — `doppio::compile()` + `journal.transactions.iter().any(|t| t.metadata.get("sfin_txn") == ...)` already works correctly. No changes needed.

2. **`ast::ValueExpr::parse(&str)`** — used in `account_mapper.rs` to parse amount strings from YAML config (e.g. `"$100"`). This is a genuine public API need that has nothing to do with transaction construction. `ast::ValueExpr` and its `parse()` method must remain public.
   The parsed `ValueExpr` is passed to `resolution::Posting::with_amount()` since `with_amount<A: Into<ast::AmountDetails>>` — verify `ValueExpr: Into<AmountDetails>` holds (add the impl if it doesn't).

3. **`ast::TransactionState`** must remain public — it is the type of `resolution::Transaction.state` and `resolution::Posting.state`.

4. **`resolution::Transaction: Display`** for `println!("{txn}")` — already implemented.

**What does NOT need to be a stable public API after migration**:
- `ast::Transaction` as a construction type
- `ast::Posting` as a construction type
- `ast::Transaction.notes` field name

#### 5.1.2 better-bytes-ledger-import

`better-bytes-ledger-import` imports Mercury bank transactions, Gusto payroll CSVs, and recognizes grant revenue. It already uses `resolution::Transaction` and `resolution::Posting` — the correct layer. Minor cleanup needed.

**Current usage** (mostly correct):

```rust
// ✓ Correct: compile and query
let journal = doppio::compile(&ledger_buf, doppio::parser::Parser { opener: ..., base_path: ... })?;
journal.transactions.iter().find(|t| t.metadata.get("mercury_id") == Some(&id))
for (account, properties) in journal.accounts { properties.note }  // AccountProperties

// ✓ Correct: builder pattern
doppio::resolution::Posting::new(account).with_amount((decimal, "$")).with_metadata("k", "v")

// ✓ Works but builder preferred: struct-literal construction
doppio::resolution::Transaction { date, description, metadata, postings, ..Default::default() }
doppio::resolution::Posting { account, amount: Some((amount, "$").into()), ..Default::default() }

// ✓ Revenue: posting amount access
posting.amount.0.get("$")  // Amount(pub BTreeMap<Commodity, Decimal>)
```

**Genuine requirements confirmed**:

1. `journal.accounts: BTreeMap<String, AccountProperties>` with `AccountProperties { note: Option<String> }` — used by `GustoLedger` to export account-to-code mappings. **Must remain stable public API.**
2. `elaboration::ResolvedPosting.amount: Amount` where `Amount(pub BTreeMap<Commodity, Decimal>)` — accessed as `.amount.0.get("$")` for revenue calculations. **Must remain stable.**
3. `resolution::Transaction` + `resolution::Posting` builder API — already correct, no migration needed.
4. `write_ledger()` (#30) — replaces the current `println!("{txn}")` pattern.

---

#### 5.1.3 betterbytes-org/ledger (future migration target)

`betterbytes-org/ledger` is the actual accounting books for Better Bytes, currently written in OG ledger-cli. It is a **future** downstream user — not yet using doppio — but it represents the most demanding real-world ledger-cli usage in this project family and is the primary litmus test for what features doppio must support to be a viable replacement.

**Ledger file structure**: A hierarchical include tree (`books.ledger` → `config/config-npo.ledger` → individual config files → `org/main-org.ledger` → `programs/*.ledger`) plus glob includes (`!include ../people/*.ledger`). Each file is a focused unit (accounts by type, per-grant declarations, per-person accounts).

**Features currently used in ledger-cli** (required for migration):

| Feature | Example | doppio status |
|---------|---------|-----------------|
| `!include <path>` | `!include config/config-npo.ledger` | Supported |
| `!include <glob>` | `!include ../people/*.ledger` | Supported (v0.2.0, PR #75) |
| `account` + `note` | `account Foo\n  note Description` | Supported (read-only via `AccountProperties.note`) |
| `account` + `assert` | `account Foo\n  assert commodity == "$"` | Supported (v0.2.0, PR #76) |
| `account` + `check` | `account Foo\n  check value =~ /regex/` | Supported (v0.2.0, PR #76) |
| `commodity` directive | `commodity $\n  format $1,000.00\n  default` | Supported (v0.2.0, PR #84) |
| `define` macros | `define assetChecker(amt) = (amt > -100)` | Supported (v0.2.0, PR #87) — parameterised, with cycle detection |
| `tag` directives | `tag Statement\n  assert value =~ /regex/` | Supported (v0.2.0, PR #87) |
| `alias` | `alias Assets:Checking = Assets:Checking:Mercury:7920` | Supported |
| Balance assertions | `Assets:Checking =$858.89` | Supported (parsed, resolved, and enforced during elaboration) |

**The invoice generation workflow** (`ledger.py`):

The invoice script does the following using ledger-cli subprocess calls:

```python
# Step 1: Filter expenses by metadata expression + account regex + date range
ledger csv -E \
  --limit "meta('program') == 'Grant:UW:HARVEST'" \
  --begin 2025-06-01 --end 2025-08-31 \
  "/^Expenses:Grants:UW:HARVEST:/"

# Step 2: Group by account (last segment), sum amounts, compute benefits + indirect

# Step 3: Append revenue recognition transaction to ledger file (raw string append)

# Step 4: Get cumulative income data
ledger csv -E -s --invert --no-rounding "Income:Grants:UW:HARVEST"

# Step 5: Write Typst data files + invoke typst compile to produce PDF
```

**What doppio needs to replace `ledger.py`**:

The invoice workflow maps cleanly to doppio primitives, some of which are M2/M3 scope and some deferred:

| Step | Requirement | Milestone |
|------|------------|-----------|
| Filter by date range | `--begin`/`--end` already in CLI; library needs date-range query | M3 / already done for CLI |
| Filter by account regex | `--pattern` flag already in CLI; `Regex` filter in library | M3 / already done |
| Filter by metadata expression (`meta('program') == ...`) | Expression-based `--limit` query | **Phase 4** (issue #45) |
| Sum amounts per account | Iterate `journal.transactions`, filter postings, accumulate | M2 (manual today, query API in Phase 4) |
| Write revenue recognition transaction | `resolution::Transaction` + `write_ledger()` (#30) | **M2** |
| Output cumulative income | Same query as above with inverted filter | M2 / Phase 4 |
| PDF generation via Typst | Out of scope for doppio; external tool invocation | Never — always external |

The revenue recognition transaction construction (`Step 3`) is **already implemented** in `better-bytes-ledger-import/src/revenue.rs` as a library function. It can be lifted almost verbatim once `write_ledger()` (#30) exists.

**Critical insight**: The `--limit "meta('program') == '...'"` metadata expression filter (issue #45) is what makes the invoice script work correctly in multi-grant ledgers. Without it, the query would have to use account regex alone (`/^Expenses:Grants:UW:HARVEST:/`), which is sufficient for single-grant filtering but fragile at scale. This confirms issue #45 (expression-based query DSL) is a **high-value Phase 4 target**, not a nice-to-have.

**Features better in doppio than ledger-cli** (improvement opportunities):

1. **`!include` with glob**: ledger-cli's glob include (`../people/*.ledger`) is order-dependent and non-deterministic across filesystems. doppio can sort glob results deterministically and report errors when the glob matches nothing.

2. **Account `assert`/`check`**: ledger-cli runs these at transaction entry time with an opaque expression language. doppio can provide:
   - Clear error messages with account name, failing expression, transaction location
   - Type-safe assertion DSL (commodity check, tag regex) vs. free-form Lisp-like expressions

3. **Balance assertions** (`=$0` at end of payroll transactions): Already parsed/resolved by doppio PR #55; enforcement (#37) will make this more robust than ledger-cli's sometimes-silent handling.

4. **`define` macros**: ledger-cli macros are global and can shadow built-ins silently. doppio can scope them to their include context and produce better error messages.

---

**REQ-GAP-000** 🔧 *(needs issue)* — **`file_opener` / `Parser.opener` spelling** *(Confirmed breakage in both downstreams)*
- Both downstreams use `file_openner` / `Parser { openner: ... }` (double 'n'). Library currently has `file_opener` / `Parser { opener: ... }` (single 'n'). Neither compiles against current HEAD.
- **Recommendation**: Fix both downstreams to single-'n'. Do not add a deprecated alias to the library.
- **Priority**: Blocking for M2.

**REQ-GAP-001** 🔧 #30 — **`write_ledger<W: Write>(writer: &mut W, txns: &[resolution::Transaction]) -> Result<()>`**
- Both downstreams currently do `for txn in txns { println!("{txn}"); }` and rely on shell redirection to append to a ledger file. `write_ledger()` enables writing to any `Write` sink in-process.

**REQ-GAP-001b** 🔧 #31 — **`eval_transaction()` bridge from resolution to elaboration**
- See §1.1 REQ-M2-002 for full description.

**REQ-GAP-002** ⏳ Phase 4 — **Streaming / incremental elaboration**
- For large journals, users may want to elaborate only recent transactions or a filtered subset
- **Current state**: Full elaboration rebuilds all balances from scratch; fast enough for typical ledgers (< 50k txns)

**REQ-GAP-002b** ✅ *(exists; stability only)* — **`ast::ValueExpr::parse()` must remain public**
- `bookie`'s `account_mapper.rs` parses amount strings from YAML config with `doppio::ast::ValueExpr::parse("$100")`. This is the one genuine public API need bookie has at the `ast` layer.
- **Required**: `ast::ValueExpr` and `ValueExpr::parse(&str)` remain public. `ValueExpr: Into<ast::AmountDetails>` must hold so `resolution::Posting::with_amount(value_expr)` compiles.

**REQ-GAP-003** ⏳ Phase 4 — **Error recovery and partial parsing**
- If one transaction has a parse error, the entire journal fails. Users may want best-effort parsing (keep valid transactions, report all errors) — useful for batch import tools where malformed rows should not abort the whole run.

**REQ-GAP-004** 🔧 #45 — **Query API / expression-based filtering**
- Users currently iterate `journal.transactions` manually. The invoice workflow in `betterbytes-org/ledger` depends on `--limit "meta('program') == 'Grant:UW:HARVEST'"` to isolate per-grant expenses across a multi-grant ledger.
- M2/M3 workaround: filter by account regex alone (`/^Expenses:Grants:UW:HARVEST:/`), sufficient for single-grant journals.
- Phase 4 progress: Issue #43 (JournalFilter struct) ✅ shipped in v0.2.0 (#72) — `--tag KEY` filter on `balance`/`register` is now available. Issue #45 (expression DSL spike) remains deferred.

**REQ-GAP-004b** ✅ Done (PR #75, v0.2.0) — **`!include` glob pattern support**
- `include path/*.ledger` and recursive `**/*.ledger` are expanded in lexicographic order. Globs that match no files produce a clear error.

**REQ-GAP-004c** ✅ Done (PR #76, v0.2.0) — **Account-level `assert`/`check` directives**
- `account` blocks accept nested `assert <expr>` (fatal) and `check <expr>` (warning) sub-directives. Expressions can reference `amount`, `commodity`, `tag("name")`, regex match `=~`/`!~`, parameterised `define` macros, and parenthesised boolean grouping.

**REQ-GAP-004d** ✅ Done (PRs #84, #87, v0.2.0) — **`commodity`, `define`, and `tag` directives**
- `commodity` directive: `format`, `default`, `nomarket`, and `note` sub-keys parsed and applied (the `format` string drives `balance`/`register` rendering).
- `define` directive: now supports parameterised macros (`define f(x) = ...`) with both value and bool bodies. Cyclic definitions are caught with a recursion limit.
- `tag` directive: supports nested `assert <expr>` / `check <expr>` for value validation, with `value` bound to the tag's string value.

**REQ-GAP-005** 🔧 #37 — **Commodity conversion / FX handling in balance assertions**
- Mixed-commodity accounts (USD + EUR) raise questions about assertion semantics. Clarify in #37 design phase.

---

### 5.2 Serialization & Persistence

**REQ-GAP-006** 🔧 Phase A (#100) — **Canonical wire format**
- v0.1.0 / v0.2.0 ship `.dop` as postcard + XZ. Postcard is Rust-specific
  and locks the wire format to the internal struct layout. Migrating to
  canonical Protocol Buffers (`proto/doppio.proto`) decouples the wire
  format from internal Rust types and makes `.dop` consumable from any
  language with a `protoc` plugin (Python, JS/TS, Go, etc.) without an
  in-process binding to the doppio crate.
- This supersedes the original "framed format / snapshot workflow"
  framing (`#17`, `#39`–`#41` — closed as deferred). The format-as-API
  analysis in [`doppio-research/serialization-followup.md`](https://github.com/alevy/doppio-research) (private)
  shows that ~80% of plausible downstream consumer use cases (P&L,
  invoices, registers, charts, reconciliation, custom balance views,
  etc.) are read-only against an elaborated journal — served fully by
  the protobuf schema, no PyO3/CGo binding needed.
- Tracked in milestone "Phase A: Wire format & WASM (toward 1.0)"
  ([#98](../../doppio/issues/98), [#100](../../doppio/issues/100)).

**REQ-GAP-007** ⏳ Post-1.0 — **Incremental `.dop` updates**
- Appending new transactions to a large journal requires full
  recompilation. Delta-encoding or append-only mode not yet designed.
- Real-world journals stay well below the threshold where full
  recompilation is painful (the bb-ledger production journal compiles
  in tens of milliseconds). Reconsider only if a downstream consumer
  reports perceptible compile latency on a real workload.

---

### 5.3 Balance Assertion Edge Cases

*(All three pending resolution of semantics in issue #37)*

**REQ-GAP-008** 🔧 #37 — **Assertion on same date as transaction**
- If a transaction and assertion both occur on the same date, assertion checks balance *after* all same-date transactions (source order).

**REQ-GAP-009** 🔧 #37 — **Assertion failure recovery**
- Recommendation: fail-fast (first failing assertion halts elaboration with a clear error).

**REQ-GAP-010** 🔧 #37 — **Rounding and tolerance in assertions**
- Recommendation: exact match to start; tolerance mode deferred to Phase 4 if needed.

---

## 6. Non-Requirements (Explicitly Out of Scope)

### 6.1 Features Intentionally Deferred

**Budget / periodic directives** (`~`) — Issue #49 decision: parsed but intentionally not elaborated. doppio models accounting facts, not budget planning.

**Automated transaction `*N` explicit multiplier syntax** — Implemented in #254. Both `*N` and `* N` (with whitespace) forms are now accepted in auto-rule body postings in both the ledger-cli and hledger frontends. They lower to the same bare-number multiplier representation as a plain bare decimal.

**Transaction matching / duplicate detection** — Phase 4 or later

**Query DSL** (Domain-specific query language for filtering/aggregating) — Issue #45, explicitly deferred from Phase 3; target Phase 4

**Multi-user/collaborative features** (locks, merge conflict resolution) — Out of scope for single-user tool

**Export to other formats** (QuickBooks, YNAB, OFX) — Out of scope; consider add-on ecosystem

**Real-time streaming journals** — Out of scope; designed for static batch files

**Native-language bindings (PyO3, CGo, JNI, etc.)** — Superseded by the
format-as-API approach (REQ-GAP-006, Phase A). The protobuf-canonical
`.dop` body is consumable from any language with a `protoc` plugin, no
in-process bindings required for read-only consumers. Re-elaboration
consumers (re-running balance assertions, FX recalc) are better served
by invoking `dop` as a CLI subprocess on a `.ledger` source than by a
language-specific wrapper. See
[`doppio-research/serialization-followup.md`](https://github.com/alevy/doppio-research) (private).

**Streaming / framed `.dop` format** — Originally tracked as #17 and
#39–#41 (Phase 4 deferred, then closed). The current format is
exceptionally compact (10k transactions: postcard+XZ ≈ 31 KB; protobuf
+ XZ ≈ 35 KB), and no consumer has reported perceptible load latency.
Reconsider only if a multi-million-transaction journal appears in
practice.

---

### 6.2 Design Decisions Establishing Boundaries

**No retaining original source in `.dop`** — Serialized journals cannot round-trip to source form. This is intentional: `.dop` is optimized for queries, not authoring.

**No in-place mutation of deserialized journals** — `elaboration::Journal` is immutable. Modifications require re-elaboration from source.

**No partial elaboration** — All transactions are elaborated together; no API for elaborating a subset while maintaining per-account balance state.

---

## 7. Key Assumptions & Open Design Questions

### 7.1 Assumptions Embedded in Current Code

1. **Dates are calendar dates only** — No sub-day precision; transactions and assertions are implicitly treated as same-day events within their date.

2. **Commodity symbols are opaque strings** — No USD-vs-dollar normalization; `USD`, `$`, `us dollar` are distinct commodities (though aliases can unify them).

3. **Accounts are hierarchical but flat** — Stored as strings with `:` separators; no implicit parent/child relationship beyond naming convention.

4. **Transaction order matters** — Balance calculations are stateful; moving a transaction earlier/later changes running balances. Assertions depend on transaction order.

5. **One null posting per transaction** — Exactly one amount can be inferred; multiple unknowns cannot be uniquely solved.

6. **Context snapshots are immutable** — Alias changes create new contexts; old contexts are never updated. This means transactions always see the same alias definitions even if directives are later amended.

---

### 7.2 Open Design Questions (Pending Clarification)

**Q1: Strict vs. Weak Balance Assertion Semantics** (Issue #37)
- What is the behavioral difference between `=` and `==`?
- Hypothesis: `==` means exact match; `=` allows rounding difference
- **Action**: Clarify in issue #37 spec before implementing

**Q2: Multi-Commodity Balance Assertion Handling**
- When an assertion specifies `$1000`, but the account holds `$1000 + 100 EUR`, should it pass or fail?
- **Hypothesis**: Fail (strict); account must have *only* the asserted commodity(ies)
- **Alternative**: Pass if the asserted commodity matches (ignore others)
- **Action**: Clarify in issue #37; add test cases

**Q3: Write_Ledger Round-Trip Semantics**
- After `write_ledger()`, if the output is parsed again, should transactions be identical?
- What about comment formatting, whitespace, metadata ordering?
- **Hypothesis**: Semantic equivalence (same accounts, amounts, dates, metadata) but formatting may differ
- **Action**: Define round-trip test in issue #30 PR

**Q4: Register `--begin` / `--end` for Milestone 3 or 4?**
- Issue #22 lists this but no implementation exists
- **Current**: M3 scope says "balance improvements" with sub-issues #32–#35; register improvements are open-ended
- **Recommendation**: Clarify in issue #22 whether `--begin`/`--end` are M3 or M4

**Q5: Cleared-Only Mode for Register**
- Issue #22 mentions it; no implementation exists
- **Action**: Clarify scope; add to M3 if time permits, else defer to M4

---

## 8. Success Criteria & Testing Strategy

### 8.1 Acceptance Criteria for Milestone 2 (Foundation)

- [ ] `write_ledger<W: Write>(&mut W, &[resolution::Transaction]) -> Result<()>` implemented and tested
- [ ] `eval_transaction()` bridge function implemented and tested
- [ ] All builder methods compile and produce semantically correct transactions
- [ ] Round-trip test: programmatically constructed txn → write_ledger → parse → same semantics
- [ ] 100% test coverage for new APIs (builder integration)
- [ ] API documentation complete (doc comments + examples)

---

### 8.2 Acceptance Criteria for Milestone 3 (CLI Completeness)

**Balance command**:
- [ ] `--cleared` flag filters to cleared transactions only
- [ ] `--format json` outputs valid JSON matching documented schema
- [ ] `--format csv` outputs RFC 4180 compliant CSV
- [ ] All flags combine orthogonally (`--cleared --begin 2024-01-01 --depth 2 --format json`)
- [ ] Output matches ledger-cli behavior for common cases

**Register command**:
- [ ] `--format json` and `--format csv` work as documented
- [ ] Output formats match balance command schema (where applicable)
- [ ] Running totals per commodity are correct (tested against manual calculation)

**Balance assertions**:
- [ ] Parse and resolve directives correctly (issue #36 – DONE in PR #55)
- [ ] Elaboration enforces assertions and reports errors clearly (#37 implementation)
- [ ] Test suite covers success, failure, multi-commodity, sequential assertions (#38)
- [ ] Error messages include expected balance, actual balance, account, date

**Overall**:
- [ ] CLI `--help` text is complete and accurate
- [ ] All new flags have short + long forms (where sensible)
- [ ] Performance targets met: balance/register on 100k txn < 1 sec from `.dop`

---

### 8.3 Testing Approach

**Unit tests**:
- Builder ergonomics (type conversions, method chaining)
- Assertion parsing and resolution
- Assertion enforcement (success and failure cases)
- Output format generation (JSON, CSV structure)

**Integration tests**:
- Full M2 workflow: build txn → eval → verify amounts
- Full M3 workflow: load ledger → query with all flags → verify output
- Round-trip: write_ledger → parse → elaborate → same as original

**Property-based tests** (if time permits):
- Commodity algebra: multi-commodity balance calculations
- Date resolution: fallback year logic
- CSV escaping: round-trip CSV → parse → CSV

---

## 9. Timeline & Dependency Graph

**Critical path**:
1. **Issue #37** (Enforce balance assertions) — blocks #38 (tests)
2. Issues #30, #31 (write_ledger, eval_transaction) — parallel; no dependency
3. Issue #33 (`--cleared` flag) — likely trivial; no dependency
4. Issue #38 (Assertion tests) — blocks nothing; can be done after #37

**Non-blocking**:
- Assertion enforcement (#37) does not block any CLI work
- CLI flags (#33, output formats) do not block library API

**Suggested order**:
1. **Week 1**: Implement #30 (write_ledger), #31 (eval_transaction), #33 (--cleared)
   - Small, orthogonal, can be tested independently
2. **Week 2**: Implement #37 (assertion enforcement) + #38 (tests)
   - Larger feature; needs spec clarification first
3. **Week 3**: Integration, performance testing, documentation

---

## 10. Documentation & Communication

### 10.1 User-Facing Documentation

**For library users**:
- Quickstart: "Building transactions programmatically" with builder examples
- API reference: Generated from doc comments; `cargo doc --no-deps`
- Round-trip workflow: "Compiling ledgers to `.dop` and querying"

**For CLI users**:
- Updated `--help` text for each subcommand
- Examples in README showing `--format json`, `--cleared`, `--depth`, date ranges
- Balance assertion guide: what they are, when to use, how to debug failures

### 10.2 Internal Documentation

- Issue #37: Spec for balance assertion enforcement (strict vs. weak, multi-commodity handling, error messages)
- Design doc for `write_ledger()` round-trip semantics
- Test plan for assertion edge cases (same-day txn + assertion, sequential assertions, etc.)

---

## 11. Risk Mitigation

### 11.1 Known Risks

**Risk**: Balance assertion enforcement is under-specified (strict vs. weak unclear)
- **Mitigation**: Block #37 on issue discussion; require consensus before implementation
- **Owner**: Project manager / issue #37 discussion

**Risk**: `write_ledger()` round-trip semantics may be ambiguous
- **Mitigation**: Define round-trip test cases in PR #30; clarify formatting expectations upfront
- **Owner**: Feature implementer for #30

**Risk**: Multi-commodity balance assertions may have surprising behavior
- **Mitigation**: Add comprehensive test cases; document assumptions in issue #37 spec
- **Owner**: Issue #37 spec owner + #38 test writer

**Risk**: CLI output format changes may break downstream tools
- **Mitigation**: Document JSON/CSV schemas explicitly; version schemas if changes needed
- **Owner**: CLI maintainer

### 11.2 Testing Gaps

**Currently**, no tests exist for:
- Balance assertion enforcement
- Multi-commodity balance assertions
- `write_ledger()` round-trip
- `eval_transaction()` with various expression types
- JSON/CSV output format schema compliance

**Action**: Create test fixtures and harnesses before implementation begins.

---

## 12. Toward 1.0

doppio v0.2.0 (2026-04-27) closed the Phase 4 / Better Bytes migration
gaps. The push to **1.0** is about committing to a stable contract for
downstream consumers across both Rust and other languages, not about
shipping every remaining feature.

The working roadmap lives in
[`alevy/doppio-research/roadmap.md`](https://github.com/alevy/doppio-research)
(private). The summary below is the public-facing sketch; the research
repo carries the full reasoning, format-comparison data, and prototype
artefacts that informed the decisions.

### What 1.0 commits to

- **Stable Rust library API** under semver discipline (additive minor,
  no breaking changes within a major).
- **Stable canonical wire format** (Protocol Buffers, defined in
  `proto/doppio.proto`) with proto3 evolution rules — additive only,
  field tags reserved on deprecation, version bumps only for
  incompatible changes.
- **Cross-language consumer story** working in production: at least
  one consumer outside the doppio crate reads `.dop` files via the
  published `.proto` schema.
- **At least one alternative frontend** (hledger) working end-to-end,
  proving the parser layer's pluggability.
- **WASM-buildable** so edge / browser / serverless consumers are
  unblocked.

### Phase A — Wire format & WASM

Migrate `.dop` from postcard+XZ to canonical Protocol Buffers; drop
xz; add `wasm32-unknown-unknown` to CI. Tracked in milestone
[Phase A: Wire format & WASM (toward 1.0)](../../doppio/milestone/5).

| Issue | Description | Depends on |
|---|---|---|
| [#98](../../doppio/issues/98) | Lock the protobuf schema for `Journal` (field numbers, decimal encoding, file location) | — (review gate) |
| [#99](../../doppio/issues/99) | Audit dependencies for `wasm32-unknown-unknown` compatibility | — (parallel with #98) |
| [#100](../../doppio/issues/100) | Migrate `Journal` serialization from postcard+xz to protobuf | #98 |
| [#101](../../doppio/issues/101) | Add `wasm32-unknown-unknown` to CI build matrix | #99, #100 |

### Phase B — Multi-frontend extensibility

Refactor the parser layer to accept multiple frontends; add hledger
as the second. Tracked in milestone
[Phase B: Multi-frontend extensibility (toward 1.0)](../../doppio/milestone/6).

| Issue | Description | Depends on |
|---|---|---|
| [#102](../../doppio/issues/102) | Refactor: `Frontend` trait + `src/grammars/` directory + extension dispatch | — (review gate) |
| [#103](../../doppio/issues/103) | Implement hledger frontend | #102 |

Phase A and Phase B are independent and can run as parallel agent
streams; the two converge at integration time. The frontends-report in
the research repo concluded hledger fits the existing AST overwhelmingly
without changes; no Journal schema changes are expected.

### Beyond 1.0

- Beancount frontend — feasibility-tested in the research repo;
  defer until demand appears (the `pad` directive needs new evaluator
  logic that has no clean ledger-cli analogue).
- Bayesian transaction-matching library (`doppio-match`) — separate
  Rust crate; downstream proof of the library API.
- Web visualizer (`doppio-web`) — browser front-end using
  WASM-compiled doppio plus a JS protobuf consumer; downstream proof
  of the WASM + format-as-API path.
- Query DSL (`--limit "expr"`, REQ-GAP-004 / #45) — substantially
  reduced urgency given format-as-API; reconsider if a recurring
  CLI-side filtering pattern emerges.

---

## Appendix A: Issue Cross-Reference

| Issue | Title | Status | M2 | M3 | Notes |
|-------|-------|--------|----|----|-------|
| #12 | Implement standalone balance assertions | Parent | | X | Blocks M3 |
| #18 | Improve ergonomics of programmatic journal entry construction | Parent | X | | |
| #21 | balance command improvements | Parent | | X | |
| #22 | register command improvements | Parent | | X | Partially deferred to M4 |
| #28 | Transaction builder | Done | X | | PR #50 |
| #29 | Typed amount shorthand | Done | X | | PR #50 |
| #30 | Add `write_ledger<W: Write>()` | Open | X | | #18 sub |
| #31 | Add `eval_transaction()` bridge | Open | X | | #18 sub |
| #32 | `--begin`/`--end` date range | Done | | X | PR #48 |
| #33 | `--cleared` flag | In Progress | | X | #21 sub |
| #34 | Tree output, `--depth` flag | Done | | X | PR #52 |
| #35 | Regex patterns for filtering | Done | | X | PR #54 |
| #36 | Parse AssertionDirective | Done | | X | PR #55 |
| #37 | Enforce balance assertions in elaboration | Open | | X | #12 sub, blocks #38 |
| #38 | Tests for balance assertions | Open | | X | #12 sub, blocked by #37 |
| #43 | JournalFilter query API | Done | | | v0.2.0 (PR #72) — `--tag KEY` filter shipped |
| #44 | JSON/CSV output formats | Done | | X | PR #53 |
| #45 | Query DSL | Deferred | | | Post-1.0 — superseded by format-as-API |
| #98 | Lock the protobuf schema for Journal | Open | | | Phase A (review gate) |
| #99 | WASM dependency audit | Open | | | Phase A |
| #100 | Migrate Journal to protobuf | Open | | | Phase A — depends on #98 |
| #101 | wasm32 in CI | Open | | | Phase A — depends on #99, #100 |
| #102 | Frontend trait + grammars/ refactor | Open | | | Phase B (review gate) |
| #103 | hledger frontend | Open | | | Phase B — depends on #102 |

---

## Appendix B: Glossary

| Term | Definition |
|------|-----------|
| **Assertion** | Balance assertion directive; standalone entry claiming an account balance at a date |
| **Commodity** | A unit of value (USD, EUR, BTC, AAPL, $, etc.) |
| **Context** | Immutable snapshot of active aliases and default commodity at a point in the journal |
| **Elaboration** | Third stage: evaluating expressions, balancing txns, enforcing assertions, producing final journal |
| **HIR** | Higher-level Intermediate Representation; output of resolution stage |
| **.dop** | Binary doppio format; postcard-serialized + XZ-compressed in v0.2.x. Migrating to canonical protobuf in Phase A (#100). |
| **Null posting** | A posting with no explicit amount; inferred as negation of sum of other postings |
| **Resolution** | Second stage: normalizing dates, indexing aliases, extracting metadata |
| **ValueExpr** | Unevaluated expression tree (numbers, operators, functions, field access) |

---

**Document Version**: 1.1
**Last Updated**: 2026-04-27
**Next Review**: Upon completion of Phase A and Phase B (toward 1.0)
