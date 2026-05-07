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
- **Positive corpus fixtures** (added later, not yet present): real
  Beancount / hledger / ledger files we want to validate doppio
  against. Add them here when a specific bug or scenario warrants
  permanent coverage. The "primary" parity fixtures stay where their
  primary consumers do (`crates/doppio/tests/fixtures/sample.beancount`,
  `crates/doppio-cli/tests/fixtures/sample.hledger`,
  `web/fixtures/sample.ledger`).

## Adding a new case

1. Drop matching files for each format you want to cover into this dir.
2. Append `Case("...", ...)` rows to the relevant list (`POSITIVE`
   or `NEGATIVE`) in `scripts/parity_check.py`.
3. Run `python3 scripts/parity_check.py [--negative]` locally to
   confirm. CI will exercise it on the next PR.

[issue #183]: https://github.com/alevy/doppio/issues/183
