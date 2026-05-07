# Cross-frontend parity fixtures

Source files used by `scripts/parity_check.py` to verify that doppio's
elaborator agrees with each format's canonical tool (`bean-check` /
Beancount API for `.beancount`, `hledger` for `.hledger`, `ledger` for
`.ledger`). See [issue #183] for the motivation.

## Conventions

- One file per (test case × format). Keep the basename consistent
  across formats so a single test case can be referenced uniformly
  (e.g. `bad-balance.beancount`, `bad-balance.hledger`,
  `bad-balance.ledger` together exercise the same invariant in three
  frontends).
- **Negative-control fixtures** are deliberately invalid (e.g. an
  unbalanced transaction). Both the canonical tool AND doppio must
  reject them. Run `python3 scripts/parity_check.py --negative` to
  exercise these; CI runs them on every PR. They guard against the
  "silent CI no-op" foot-gun: a future change that turns the parity
  job into a no-op makes negative cases pass-by-omission instead of
  failing as they should.
- **Positive corpus fixtures**: upstream- or generator-sourced files
  we want to validate doppio against. The "primary" parity fixtures
  stay where their primary consumers do
  (`crates/doppio/tests/fixtures/sample.beancount`,
  `crates/doppio-cli/tests/fixtures/sample.hledger`,
  `web/fixtures/sample.ledger`); broader corpus additions go here.

## Vendoring upstream fixtures

When pulling a fixture from an upstream repo (preferred over fetching
at CI time -- vendoring keeps the test reproducible and bisectable),
prepend a comment block at the top of the file recording:

- **Source URL** -- a permalink including the upstream commit SHA
- **Commit SHA + branch + fetch date**
- **License** -- and a one-line note on compatibility with doppio's MIT
- **What it exercises** -- one or two sentences naming the features
  this fixture covers, so a future reader can decide whether to keep,
  delete, or replace it

Example: see `hledger-ascii.journal` in this directory.

The harness reads the file verbatim; comment blocks are skipped by
each canonical tool's parser. Naming convention for upstream-sourced
fixtures: `<format>-<descriptive-name>.<ext>`
(e.g. `hledger-ascii.journal`, `beancount-vesting.beancount`).

## Generated fixtures

`bean-example` (ships with the `beancount` Python package) produces a
realistic synthetic Beancount journal. To regenerate deterministically:

    bean-example --seed 42 -o tests/parity/beancount-example.beancount

Then prepend a header recording the generator version + seed + date.
Beancount is the only frontend with a usable generator; ledger-cli
and hledger don't ship analogues.

> **Currently blocked**: `bean-example` output exercises Beancount's
> per-transaction balance tolerance (small sub-cent rounding residuals
> that bean-check accepts but doppio's elaborator currently rejects).
> Tracked in #198. Once that lands, regenerate per the command above
> and add to `POSITIVE` in `scripts/parity_check.py`.

## Adding a new case

1. Vendor / generate the file(s) into this dir per the conventions above.
2. Append `Case("label", path, canonical_fn)` rows to the relevant list
   (`POSITIVE` or `NEGATIVE`) in `scripts/parity_check.py`. Use a
   `frontend:name` label (e.g. `hledger:ascii`) so failures are easy
   to attribute.
3. Run `python3 scripts/parity_check.py [--negative]` locally to
   confirm. CI exercises both positive and negative on every PR.
4. If the fixture surfaces a real gap in doppio (silent miscomputation,
   parse rejection of valid syntax), file the gap as its own issue
   before adding the fixture to `POSITIVE` -- the parity harness is
   only useful when its passes are honest.

[issue #183]: https://github.com/alevy/doppio/issues/183
