#!/usr/bin/env python3
"""
Cross-frontend parity harness (issue #183).

For each (frontend, fixture) pair this script:

  1. Computes the per-account-per-commodity balance via the format's canonical
     CLI (or library API for Beancount, which has no balance-emitting CLI).
  2. Computes the same balance via `dop balance --format=json`.
  3. Normalises both sides to a set of `(account, commodity, amount)` tuples
     (zero-balance entries dropped) and asserts equality.

Run with `--negative` to instead exercise the deliberately-broken fixtures and
assert that BOTH tools reject them. This is the test-of-the-test: if a future
change accidentally turns the parity job into a silent no-op, the negative
controls fail loudly.

Usage:
    python3 scripts/parity_check.py [--dop-bin path/to/dop] [--negative]
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from decimal import Decimal
from pathlib import Path
from typing import Callable

REPO = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------------
# Canonical-tool balance extractors. Each returns a set of
# (account, commodity, Decimal) tuples; zero-balance entries are filtered.
# ---------------------------------------------------------------------------

Tuple3 = tuple[str, str, Decimal]


def beancount_balances(fixture: Path) -> set[Tuple3]:
    """Use the Beancount Python API directly. Cleaner than parsing
    `bean-report` text and survives the 2.x → 3.x transition."""
    from beancount.loader import load_file
    from beancount.core.realization import realize, iter_children

    entries, errors, _options = load_file(str(fixture))
    if errors:
        raise RuntimeError(f"beancount errors loading {fixture}: {errors}")
    real_root = realize(entries)
    out: set[Tuple3] = set()
    for ra in iter_children(real_root):
        for pos in ra.balance.get_positions():
            amt = pos.units.number
            if amt is not None and amt != 0:
                out.add((ra.account, pos.units.currency, Decimal(str(amt))))
    return out


def hledger_balances(fixture: Path) -> set[Tuple3]:
    """Run hledger and parse its native JSON balance output.

    hledger emits a [accounts_list, total] pair. Each account entry is
    `[short_name, full_name, depth, [{"acommodity": ..., "aquantity": {"decimalMantissa": ..., "decimalPlaces": ...}}]]`.
    """
    raw = subprocess.run(
        ["hledger", "-f", str(fixture), "balance", "--output-format=json", "--no-total", "--flat"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    payload = json.loads(raw)
    # Outer is [accounts, total]; we only need accounts.
    accounts = payload[0] if isinstance(payload, list) and len(payload) >= 1 else payload
    out: set[Tuple3] = set()
    for entry in accounts:
        # entry shape: [short_name, full_name, depth, [amounts]]
        full_name = entry[1]
        amounts = entry[3]
        for a in amounts:
            commodity = a["acommodity"]
            q = a["aquantity"]
            mantissa = Decimal(q["decimalMantissa"])
            places = int(q["decimalPlaces"])
            value = mantissa / (Decimal(10) ** places)
            if value != 0:
                out.add((full_name, commodity, value))
    return out


# Matches a ledger-cli amount, e.g. "$10,318.88" or "30 AAPL" or "-$48.20" or "24.60 EUR".
# Group 1: optional symbol prefix (e.g. "$", "-$"). Group 2: numeric body.
# Group 3: optional commodity suffix word.
_LEDGER_AMOUNT = re.compile(
    r"^\s*"
    r"(?P<lead>[-+]?[^\d\s.,-]+)?"              # leading symbol like "$" or "-$"
    r"\s*"
    r"(?P<num>[-+]?[\d,]+(?:\.\d+)?)"           # numeric body
    r"\s*"
    r"(?P<suffix>[A-Z][A-Z0-9_'.\-]*)?"         # commodity suffix word
    r"\s*$"
)


def _parse_ledger_amount(s: str) -> tuple[str, Decimal]:
    """Parse `$10,318.88` / `30 AAPL` / `-$48.20` / `420.00 EUR` into
    (commodity_symbol, signed_decimal). Used for both ledger and any text
    fall-back parsers."""
    s = s.strip()
    m = _LEDGER_AMOUNT.match(s)
    if not m:
        raise ValueError(f"unparseable ledger amount: {s!r}")
    lead = m.group("lead") or ""
    suffix = m.group("suffix") or ""
    num_str = m.group("num").replace(",", "")
    if lead and suffix:
        raise ValueError(f"both lead and suffix commodity in {s!r}")
    if lead:
        # Could be "-$" or "$" or "+$". Strip any sign.
        sign = ""
        commodity = lead
        if lead[0] in "+-":
            sign = lead[0]
            commodity = lead[1:]
        return commodity, Decimal(sign + num_str)
    if suffix:
        return suffix, Decimal(num_str)
    raise ValueError(f"no commodity in {s!r}")


def ledger_balances(fixture: Path) -> set[Tuple3]:
    """Parse ledger-cli's flat balance output via a `--format` template.

    Multi-commodity accounts emit a `Account|amount` line followed by one
    or more continuation lines holding only an amount (no `|`); the
    continuation lines belong to the previous account."""
    raw = subprocess.run(
        [
            "ledger",
            "-f", str(fixture),
            "balance",
            "--flat",
            "--no-pager",
            "--no-color",
            "--no-total",
            "--format=%(account)|%(scrub(display_total))\n",
        ],
        capture_output=True,
        text=True,
        check=True,
    ).stdout

    out: set[Tuple3] = set()
    last_account: str | None = None
    for line in raw.splitlines():
        line = line.rstrip()
        if not line:
            continue
        if "|" in line:
            account, amt_str = line.split("|", 1)
            account = account.strip()
            for piece in amt_str.split("\n"):
                piece = piece.strip()
                if not piece:
                    continue
                commodity, value = _parse_ledger_amount(piece)
                if value != 0:
                    out.add((account, commodity, value))
            last_account = account
        else:
            # Continuation amount for the previous account.
            if last_account is None:
                raise RuntimeError(f"orphan amount line {line!r} (no preceding account)")
            commodity, value = _parse_ledger_amount(line)
            if value != 0:
                out.add((last_account, commodity, value))
    return out


def doppio_balances(fixture: Path, dop_bin: Path) -> set[Tuple3]:
    raw = subprocess.run(
        [str(dop_bin), "balance", "--format=json", "--flat", str(fixture)],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    payload = json.loads(raw)
    out: set[Tuple3] = set()
    for entry in payload:
        account = entry["account"]
        for b in entry["balances"]:
            amt = Decimal(b["amount"])
            if amt != 0:
                out.add((account, b["commodity"], amt))
    return out


# ---------------------------------------------------------------------------
# Test catalog
# ---------------------------------------------------------------------------

CanonicalFn = Callable[[Path], set[Tuple3]]


@dataclass
class Case:
    label: str
    fixture: Path
    canonical: CanonicalFn
    canonical_args: dict | None = None  # for canonicals that need extra args


POSITIVE: list[Case] = [
    # "Primary" fixtures live where their primary consumers do -- near
    # each crate's tests, not under a top-level e2e dir. The parity
    # harness reaches in to validate them.
    Case("beancount:sample", REPO / "crates/doppio/tests/fixtures/sample.beancount", beancount_balances),
    Case("hledger:sample",   REPO / "crates/doppio-cli/tests/fixtures/sample.hledger", hledger_balances),
    Case("ledger:sample",    REPO / "web/fixtures/sample.ledger", ledger_balances),
    # Upstream-sourced corpus under tests/parity/. Each fixture's
    # leading comment block records source URL + commit SHA + license
    # per the convention documented in tests/parity/README.md.
    Case("hledger:ascii",         REPO / "tests/parity/hledger-ascii.journal", hledger_balances),
    Case("hledger:zerostar",      REPO / "tests/parity/hledger-zerostar.journal", hledger_balances),
    Case("hledger:block-comment", REPO / "tests/parity/hledger-block-comment.journal", hledger_balances),
]

NEGATIVE: list[Case] = [
    Case("beancount", REPO / "tests/parity/bad-balance.beancount", beancount_balances),
    Case("hledger",   REPO / "tests/parity/bad-balance.hledger",   hledger_balances),
    Case("ledger",    REPO / "tests/parity/bad-balance.ledger",    ledger_balances),
]


# ---------------------------------------------------------------------------
# Comparison + reporting
# ---------------------------------------------------------------------------


def diff_sets(canonical: set[Tuple3], doppio: set[Tuple3]) -> str | None:
    """Return None if equal; otherwise a multi-line human-readable diff."""
    if canonical == doppio:
        return None
    only_canon = canonical - doppio
    only_dop = doppio - canonical
    # Items where (account, commodity) match but amounts differ -- highlight separately.
    by_key = lambda s: {(a, c): v for (a, c, v) in s}
    canon_map = by_key(canonical)
    dop_map = by_key(doppio)
    common_keys = canon_map.keys() & dop_map.keys()
    mismatched = [(k, canon_map[k], dop_map[k]) for k in common_keys if canon_map[k] != dop_map[k]]

    lines = []
    if mismatched:
        lines.append("  amount mismatches:")
        for (acct, comm), cv, dv in sorted(mismatched):
            lines.append(f"    {acct} {comm}: canonical={cv}  doppio={dv}")
    only_canon_strict = {(a, c, v) for (a, c, v) in only_canon if (a, c) not in dop_map}
    only_dop_strict = {(a, c, v) for (a, c, v) in only_dop if (a, c) not in canon_map}
    if only_canon_strict:
        lines.append("  only in canonical:")
        for a, c, v in sorted(only_canon_strict):
            lines.append(f"    {a} {v} {c}")
    if only_dop_strict:
        lines.append("  only in doppio:")
        for a, c, v in sorted(only_dop_strict):
            lines.append(f"    {a} {v} {c}")
    return "\n".join(lines)


def run_positive(case: Case, dop_bin: Path) -> bool:
    print(f"  [{case.label}] {case.fixture.name}", end=" ... ", flush=True)
    canonical = case.canonical(case.fixture)
    doppio = doppio_balances(case.fixture, dop_bin)
    diff = diff_sets(canonical, doppio)
    if diff is None:
        print("OK")
        return True
    print("MISMATCH")
    print(diff, file=sys.stderr)
    return False


def run_negative(case: Case, dop_bin: Path) -> bool:
    """Negative control: BOTH tools must reject the fixture. This proves the
    harness can detect failures (catches the silent-no-op-CI foot-gun)."""
    print(f"  [{case.label}] {case.fixture.name}", end=" ... ", flush=True)
    canonical_failed = False
    doppio_failed = False
    try:
        case.canonical(case.fixture)
    except Exception:
        canonical_failed = True
    try:
        doppio_balances(case.fixture, dop_bin)
    except Exception:
        doppio_failed = True
    if canonical_failed and doppio_failed:
        print("OK (both rejected as expected)")
        return True
    print("UNEXPECTED ACCEPTANCE")
    if not canonical_failed:
        print("    canonical tool accepted a fixture that should be invalid", file=sys.stderr)
    if not doppio_failed:
        print("    doppio accepted a fixture that should be invalid", file=sys.stderr)
    return False


def main() -> int:
    p = argparse.ArgumentParser()
    default_dop = REPO / "target" / "release" / "dop"
    p.add_argument("--dop-bin", default=os.environ.get("DOP_BIN") or str(default_dop))
    p.add_argument("--negative", action="store_true",
                   help="Run negative-control fixtures (assert both tools reject)")
    args = p.parse_args()

    dop_bin = Path(args.dop_bin)
    if not dop_bin.exists():
        print(f"ERROR: dop binary not found at {dop_bin}", file=sys.stderr)
        print("       run: cargo build --release -p doppio-cli", file=sys.stderr)
        return 2

    cases = NEGATIVE if args.negative else POSITIVE
    mode = "negative" if args.negative else "positive"
    print(f"Running {len(cases)} {mode} parity case(s):")
    fail = 0
    for case in cases:
        if not case.fixture.exists():
            print(f"  [{case.label}] MISSING FIXTURE: {case.fixture}", file=sys.stderr)
            fail += 1
            continue
        try:
            ok = run_negative(case, dop_bin) if args.negative else run_positive(case, dop_bin)
        except Exception as e:
            print(f"\n  [{case.label}] ERROR: {e}", file=sys.stderr)
            ok = False
        if not ok:
            fail += 1

    if fail:
        print(f"\n{fail} case(s) failed", file=sys.stderr)
        return 1
    print(f"\nAll {len(cases)} case(s) passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
