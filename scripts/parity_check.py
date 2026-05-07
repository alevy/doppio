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
    `bean-report` text and survives the 2.x → 3.x transition.

    Aggregates per-lot positions into a single (account, commodity,
    total) tuple so the comparison matches doppio's aggregate balance
    view. Beancount tracks lots individually (so an account holding
    three ITOT purchases at different costs has three position rows
    for the same currency); doppio reports the aggregate. Per-lot
    inventory comparison is its own concern (#185)."""
    from beancount.loader import load_file
    from beancount.core.realization import realize, iter_children

    entries, errors, _options = load_file(str(fixture))
    if errors:
        raise RuntimeError(f"beancount errors loading {fixture}: {errors}")
    real_root = realize(entries)
    aggregated: dict[tuple[str, str], Decimal] = {}
    for ra in iter_children(real_root):
        for pos in ra.balance.get_positions():
            amt = pos.units.number
            if amt is None:
                continue
            key = (ra.account, pos.units.currency)
            aggregated[key] = aggregated.get(key, Decimal(0)) + Decimal(str(amt))
    return {(acct, comm, total) for (acct, comm), total in aggregated.items() if total != 0}


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
    r"(?P<suffix>[A-Za-z][A-Za-z0-9_'.\-]*)?"   # commodity suffix word (case-insensitive; ledger-cli allows lowercase like `bytes`)
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
# Tag / metadata extractors (#226 Phase 1).
#
# Per-transaction fingerprint: (date, description, frozenset(tags),
# frozenset((key, value), ...)). Compared as multisets so duplicate
# (date, description) transactions in a fixture don't collapse.
# ---------------------------------------------------------------------------

# Fingerprint = (date, description, frozenset[tag], frozenset[(meta_key, meta_value)]).
Fingerprint = tuple[str, str, frozenset[str], frozenset[tuple[str, str]]]


def _fp_sort_key(fp: Fingerprint) -> tuple:
    """Stable sort key for a Fingerprint -- frozensets aren't directly orderable."""
    date, desc, tags, meta = fp
    return (date, desc, tuple(sorted(tags)), tuple(sorted(meta)))


def beancount_tags_metadata(fixture: Path) -> list[Fingerprint]:
    """Per-transaction tags + metadata via Beancount's Python API.

    Beancount distinguishes `#tag` from `^link`; doppio's Beancount parser
    keeps the `^` prefix on link tags so the distinction is recoverable.
    Combine both into a single frozen set on the canonical side, prefixing
    links with `^` to match doppio's representation."""
    from beancount.loader import load_file
    from beancount.core.data import Transaction

    entries, errors, _ = load_file(str(fixture))
    if errors:
        raise RuntimeError(f"beancount errors loading {fixture}: {errors}")
    out: list[Fingerprint] = []
    for entry in entries:
        if not isinstance(entry, Transaction):
            continue
        # Skip pad-synthesised transactions: bean-check inserts them as
        # `(Padding inserted for ...)`. doppio's analogous synthesised
        # transactions are dropped on the doppio side too. They're a
        # tool-internal artefact, not a user-meaningful tag/metadata
        # carrier.
        narration = entry.narration or ""
        if narration.startswith("(Padding inserted"):
            continue
        tags = set(entry.tags or ())
        links = {f"^{ln}" for ln in (entry.links or ())}
        # Beancount filters `filename` / `lineno` / `__tolerances__` out of
        # user-visible metadata; do the same so canonical and doppio agree.
        meta = {
            k: str(v)
            for k, v in (entry.meta or {}).items()
            if not k.startswith("_") and k not in ("filename", "lineno")
        }
        out.append(
            (
                entry.date.isoformat(),
                narration,
                frozenset(tags | links),
                frozenset(meta.items()),
            )
        )
    return out


def hledger_tags_metadata(fixture: Path) -> list[Fingerprint]:
    """Per-transaction tags + metadata via `hledger print --output-format=json`.

    `ttags` is a list of [name, value] pairs. hledger separates "bare" tags
    from key:value metadata only by convention (a tag with empty value is
    bare); we emit `name` as a tag for empty-value pairs and `name=value`
    pairs into metadata otherwise."""
    raw = subprocess.run(
        ["hledger", "-f", str(fixture), "print", "--output-format=json"],
        capture_output=True, text=True, check=True,
    ).stdout
    payload = json.loads(raw)
    out: list[Fingerprint] = []
    for txn in payload:
        date = txn.get("tdate") or ""
        desc = txn.get("tdescription") or ""
        tags: set[str] = set()
        meta: dict[str, str] = {}
        for pair in txn.get("ttags") or ():
            name, value = pair[0], pair[1]
            if value == "":
                tags.add(name)
            else:
                meta[name] = value
        out.append((date, desc, frozenset(tags), frozenset(meta.items())))
    return out


def doppio_tags_metadata(fixture: Path, dop_bin: Path) -> list[Fingerprint]:
    """Read per-transaction tags + metadata from `dop register --format=json`.

    Register emits per-posting rows with txn-level tags/metadata duplicated
    across every posting of the same transaction; dedupe by (date,
    description, txn_tags, txn_metadata). Posting-level tags / metadata are
    not currently compared at this phase (see #226 follow-on)."""
    raw = subprocess.run(
        [str(dop_bin), "register", "--format=json", str(fixture)],
        capture_output=True, text=True, check=True,
    ).stdout
    payload = json.loads(raw)
    seen: set[Fingerprint] = set()
    out: list[Fingerprint] = []
    for row in payload:
        date = row["date"]
        desc = row["description"]
        # Skip the synthesised pad rows -- they're a doppio-internal artefact
        # of pad+balance reconciliation that has no canonical analogue at
        # this phase.
        if desc == "(padding inserted for balance assertion)":
            continue
        tags = frozenset(row.get("txn_tags") or ())
        meta = frozenset((row.get("txn_metadata") or {}).items())
        fp = (date, desc, tags, meta)
        if fp in seen:
            continue
        seen.add(fp)
        out.append(fp)
    return out


def diff_fingerprints(canonical: list[Fingerprint], doppio: list[Fingerprint]) -> str | None:
    """Compare distinct per-transaction (date, description, tags, metadata)
    fingerprints between the two tools.

    Set-based, not multiset: if the same fingerprint occurs N times on
    one side and M times on the other, that's tolerated. Multiset
    precision would need a per-transaction id surfaced by `dop register`,
    which doesn't exist yet; for the gap classes #226 actually targets
    (a tag dropped, a metadata key renamed, a quoted string preserving
    its quotes), set-based comparison surfaces the divergence either
    way."""
    canon_set = set(canonical)
    dop_set = set(doppio)
    if canon_set == dop_set:
        return None
    only_canon = canon_set - dop_set
    only_dop = dop_set - canon_set
    lines = []
    if only_canon:
        lines.append("  txn fingerprints only in canonical:")
        for fp in sorted(only_canon, key=_fp_sort_key):
            date, desc, tags, meta = fp
            lines.append(
                f"    [{date}] '{desc}' tags={sorted(tags)} meta={dict(sorted(meta))}"
            )
    if only_dop:
        lines.append("  txn fingerprints only in doppio:")
        for fp in sorted(only_dop, key=_fp_sort_key):
            date, desc, tags, meta = fp
            lines.append(
                f"    [{date}] '{desc}' tags={sorted(tags)} meta={dict(sorted(meta))}"
            )
    return "\n".join(lines)


# Map balance extractor -> tags/metadata extractor. ledger-cli's tag handling
# isn't structured in its native output; skip for this phase.
TAGS_META_EXTRACTOR: dict[CanonicalFn, Callable[[Path], list[Fingerprint]] | None] = {}


def _register_tags_meta_extractors() -> None:
    """Populate TAGS_META_EXTRACTOR after the balance extractors are defined.
    Cleaner than ordering by source position."""
    TAGS_META_EXTRACTOR[beancount_balances] = beancount_tags_metadata
    TAGS_META_EXTRACTOR[hledger_balances] = hledger_tags_metadata
    TAGS_META_EXTRACTOR[ledger_balances] = None


_register_tags_meta_extractors()


# ---------------------------------------------------------------------------
# Historical-price extractors (#226 Phase 2).
#
# Quadruple per quote: (date, source_commodity, target_commodity, value).
# Compared as a set: order doesn't matter, duplicates are collapsed.
# ---------------------------------------------------------------------------

PriceQuad = tuple[str, str, str, Decimal]


def beancount_prices(fixture: Path) -> set[PriceQuad]:
    """Per-quote price tuples via the Beancount Python API.

    Each `Price` entry has `.date`, `.currency` (the source commodity),
    and `.amount` (a `(number, currency)` pair). Beancount auto-derives
    additional price entries from `@`/`@@` annotations on transaction
    postings; doppio's `Journal.prices` only carries explicit `P` /
    `price` directives. To compare like-with-like, filter out
    auto-derived entries by inspecting `entry.meta`: bean-check tags
    auto-derived prices with `__implicit_prices__` in newer versions or
    omits a real source line; defensively, accept entries whose
    `meta['filename']` matches the fixture path."""
    from beancount.loader import load_file
    from beancount.core.data import Price

    entries, errors, _ = load_file(str(fixture))
    if errors:
        raise RuntimeError(f"beancount errors loading {fixture}: {errors}")
    out: set[PriceQuad] = set()
    # bean-check populates `entry.meta['filename']` with the resolved
    # absolute path of the source file; canonicalise the fixture path
    # the same way for the comparison.
    fixture_abs = str(fixture.resolve())
    for entry in entries:
        if not isinstance(entry, Price):
            continue
        # Skip auto-derived quotes that bean-check synthesises from
        # `@`/`@@` postings (Beancount's "implicit price" mechanism).
        # Such entries have a synthetic filename set by the synthesiser,
        # not the user-visible path. doppio doesn't synthesise these,
        # so filtering keeps the comparison apples-to-apples.
        meta = entry.meta or {}
        if meta.get("filename") != fixture_abs:
            continue
        out.add(
            (
                entry.date.isoformat(),
                entry.currency,
                entry.amount.currency,
                entry.amount.number,
            )
        )
    return out


def hledger_prices(fixture: Path) -> set[PriceQuad]:
    """Per-quote price tuples via `hledger prices`. Outputs lines of the
    form `P <date> <commodity> <amount>` where `<amount>` is in
    ledger-style (`$1.10` / `0.70 EUR`). hledger only emits explicit
    source-defined `P` directives -- it does NOT synthesise quotes from
    `@`/`@@` posting annotations, so this matches doppio's posture."""
    raw = subprocess.run(
        ["hledger", "-f", str(fixture), "prices"],
        capture_output=True, text=True, check=True,
    ).stdout
    out: set[PriceQuad] = set()
    for line in raw.splitlines():
        line = line.strip()
        if not line or not line.startswith("P "):
            continue
        # P <date> <commodity> <amount>
        parts = line[2:].split(None, 2)
        if len(parts) != 3:
            continue
        date, commodity, amount_str = parts
        target_commodity, value = _parse_ledger_amount(amount_str)
        out.add((date, commodity, target_commodity, value))
    return out


def doppio_prices(fixture: Path, dop_bin: Path) -> set[PriceQuad]:
    """Read explicit `P` / `price` directives via `dop prices --format=json`."""
    raw = subprocess.run(
        [str(dop_bin), "prices", "--format=json", str(fixture)],
        capture_output=True, text=True, check=True,
    ).stdout
    payload = json.loads(raw)
    out: set[PriceQuad] = set()
    for row in payload:
        out.add(
            (
                row["date"],
                row["commodity"],
                row["price_commodity"],
                Decimal(row["price_amount"]),
            )
        )
    return out


def diff_prices(canonical: set[PriceQuad], doppio: set[PriceQuad]) -> str | None:
    """Compare price quote sets; return None on match."""
    if canonical == doppio:
        return None
    only_canon = canonical - doppio
    only_dop = doppio - canonical
    lines = []
    if only_canon:
        lines.append("  price quotes only in canonical:")
        for q in sorted(only_canon):
            date, src, tgt, val = q
            lines.append(f"    [{date}] {src} -> {tgt}: {val}")
    if only_dop:
        lines.append("  price quotes only in doppio:")
        for q in sorted(only_dop):
            date, src, tgt, val = q
            lines.append(f"    [{date}] {src} -> {tgt}: {val}")
    return "\n".join(lines)


PRICES_EXTRACTOR: dict[CanonicalFn, Callable[[Path], set[PriceQuad]] | None] = {}


def _register_prices_extractors() -> None:
    """ledger-cli's `prices` / `pricedb` mixes explicit `P` directives
    with auto-derived quotes from lot annotations and `@`/`@@`
    annotations, with no clean way to filter to just the explicit
    ones. Skip ledger-cli for Phase 2; same posture as Phase 1's
    tag-metadata skip. Filed as a follow-on under #226."""
    PRICES_EXTRACTOR[beancount_balances] = beancount_prices
    PRICES_EXTRACTOR[hledger_balances] = hledger_prices
    PRICES_EXTRACTOR[ledger_balances] = None


_register_prices_extractors()


# ---------------------------------------------------------------------------
# Pad-synthesised transaction extractors (#226 Phase 3).
#
# Both Beancount and doppio synthesise a transaction backdated to the pad
# directive's own date that brings the running balance up to the next
# balance assertion. The two-posting shape is identical -- target gets the
# corrective amount, source absorbs the offset -- so a per-(date, frozenset
# of (account, commodity, amount)) comparison catches drift in either the
# pad date, the chosen source, or the per-commodity amount. The frozenset
# is intentionally symmetric over the two postings: bean-check doesn't
# label which posting is target vs source, and we don't need to.
# ---------------------------------------------------------------------------

# Per-pad-txn fingerprint: (date, frozenset((account, commodity, value))).
PadFingerprint = tuple[str, frozenset[tuple[str, str, Decimal]]]


def beancount_pad_fingerprints(fixture: Path) -> set[PadFingerprint]:
    from beancount.loader import load_file
    from beancount.core.data import Transaction

    entries, errors, _ = load_file(str(fixture))
    if errors:
        raise RuntimeError(f"beancount errors loading {fixture}: {errors}")
    out: set[PadFingerprint] = set()
    for entry in entries:
        if not isinstance(entry, Transaction):
            continue
        if not (entry.narration or "").startswith("(Padding inserted"):
            continue
        postings_set: set[tuple[str, str, Decimal]] = set()
        for posting in entry.postings:
            if posting.units is None:
                continue
            postings_set.add(
                (
                    posting.account,
                    posting.units.currency,
                    Decimal(posting.units.number),
                )
            )
        out.add((entry.date.isoformat(), frozenset(postings_set)))
    return out


def doppio_pad_fingerprints(fixture: Path, dop_bin: Path) -> set[PadFingerprint]:
    """Group `dop register --format=json` rows for pad-synthesised
    transactions into per-(date, target, source) postings sets, then
    discard the labelling and return per-(date, frozenset(postings))
    fingerprints to match the canonical shape."""
    raw = subprocess.run(
        [str(dop_bin), "register", "--format=json", str(fixture)],
        capture_output=True, text=True, check=True,
    ).stdout
    payload = json.loads(raw)
    grouped: dict[tuple[str, str, str, str], set[tuple[str, str, Decimal]]] = {}
    for row in payload:
        if row["description"] != "(padding inserted for balance assertion)":
            continue
        source = row.get("txn_metadata", {}).get("pad")
        if source is None:
            continue
        date = row["date"]
        account = row["account"]
        commodity = row["commodity"]
        amount = Decimal(row["amount"])
        if account == source:
            grouped.setdefault((date, "__pending__", source, commodity), set()).add(
                (account, commodity, amount)
            )
        else:
            grouped.setdefault((date, account, source, commodity), set()).add(
                (account, commodity, amount)
            )
    out: set[PadFingerprint] = set()
    for key, postings in grouped.items():
        date, target, source, commodity = key
        if target == "__pending__":
            continue
        pending_key = (date, "__pending__", source, commodity)
        pending = grouped.get(pending_key, set())
        target_amount = next(iter(postings))[2]
        matching_source = next(
            (
                p
                for p in pending
                if p[0] == source and p[1] == commodity and p[2] == -target_amount
            ),
            None,
        )
        full = set(postings)
        if matching_source is not None:
            full.add(matching_source)
        out.add((date, frozenset(full)))
    return out


def diff_pad_fingerprints(
    canonical: set[PadFingerprint], doppio: set[PadFingerprint]
) -> str | None:
    if canonical == doppio:
        return None
    only_canon = canonical - doppio
    only_dop = doppio - canonical
    lines = []
    if only_canon:
        lines.append("  pad txns only in canonical:")
        for fp in sorted(only_canon, key=lambda x: (x[0], sorted(map(str, x[1])))):
            date, postings = fp
            posting_strs = ", ".join(
                f"{a} {v} {c}" for (a, c, v) in sorted(postings)
            )
            lines.append(f"    [{date}] {{ {posting_strs} }}")
    if only_dop:
        lines.append("  pad txns only in doppio:")
        for fp in sorted(only_dop, key=lambda x: (x[0], sorted(map(str, x[1])))):
            date, postings = fp
            posting_strs = ", ".join(
                f"{a} {v} {c}" for (a, c, v) in sorted(postings)
            )
            lines.append(f"    [{date}] {{ {posting_strs} }}")
    return "\n".join(lines)


PAD_EXTRACTOR: dict[CanonicalFn, Callable[[Path], set[PadFingerprint]] | None] = {}


def _register_pad_extractors() -> None:
    """Only Beancount has a `pad` directive analogue. hledger / ledger-cli
    have no equivalent; their fixtures return empty sets on both sides."""
    PAD_EXTRACTOR[beancount_balances] = beancount_pad_fingerprints
    PAD_EXTRACTOR[hledger_balances] = None
    PAD_EXTRACTOR[ledger_balances] = None


_register_pad_extractors()


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
    Case("ledger:transfer",       REPO / "tests/parity/ledger-transfer.ledger",  ledger_balances),
    Case("hledger:quickstart",    REPO / "tests/parity/hledger-quickstart.journal", hledger_balances),
    Case("hledger:ascii",         REPO / "tests/parity/hledger-ascii.journal", hledger_balances),
    Case("hledger:zerostar",      REPO / "tests/parity/hledger-zerostar.journal", hledger_balances),
    Case("hledger:zerostar-subtree", REPO / "tests/parity/hledger-zerostar-subtree.journal", hledger_balances),
    Case("hledger:block-comment", REPO / "tests/parity/hledger-block-comment.journal", hledger_balances),
    Case("beancount:example",     REPO / "tests/parity/bean-example.beancount", beancount_balances),
    Case("beancount:subtree-balance", REPO / "tests/parity/beancount-subtree-balance.beancount", beancount_balances),
    Case("beancount:subtree-pad",     REPO / "tests/parity/beancount-subtree-pad.beancount",     beancount_balances),
    Case("beancount:starter",         REPO / "tests/parity/beancount-starter.beancount",         beancount_balances),
    Case("beancount:basic",           REPO / "tests/parity/beancount-basic.beancount",           beancount_balances),
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
    # Phase 0: balance equality (the original parity check, #196).
    canonical = case.canonical(case.fixture)
    doppio = doppio_balances(case.fixture, dop_bin)
    bal_diff = diff_sets(canonical, doppio)

    # Phase 1: per-transaction tags + metadata (#226). Skipped for ledger-cli
    # since its native output doesn't expose tags structurally.
    tags_meta_diff: str | None = None
    tm_extractor = TAGS_META_EXTRACTOR.get(case.canonical)
    if tm_extractor is not None:
        canonical_fps = tm_extractor(case.fixture)
        doppio_fps = doppio_tags_metadata(case.fixture, dop_bin)
        tags_meta_diff = diff_fingerprints(canonical_fps, doppio_fps)

    # Phase 2: explicit historical-price quotes (#226). Skipped for ledger-cli
    # since its `prices`/`pricedb` output mixes inferred quotes from `@`/`{cost}`.
    prices_diff: str | None = None
    pr_extractor = PRICES_EXTRACTOR.get(case.canonical)
    if pr_extractor is not None:
        canonical_prices = pr_extractor(case.fixture)
        doppio_prices_set = doppio_prices(case.fixture, dop_bin)
        prices_diff = diff_prices(canonical_prices, doppio_prices_set)

    # Phase 3: pad-synthesised transactions (#226). Only Beancount has a
    # `pad` directive; other frontends always return empty sets.
    pad_diff: str | None = None
    pad_extractor = PAD_EXTRACTOR.get(case.canonical)
    if pad_extractor is not None:
        canonical_pads = pad_extractor(case.fixture)
        doppio_pads = doppio_pad_fingerprints(case.fixture, dop_bin)
        pad_diff = diff_pad_fingerprints(canonical_pads, doppio_pads)

    if (
        bal_diff is None
        and tags_meta_diff is None
        and prices_diff is None
        and pad_diff is None
    ):
        print("OK")
        return True
    print("MISMATCH")
    if bal_diff is not None:
        print("  balance:", file=sys.stderr)
        print(bal_diff, file=sys.stderr)
    if tags_meta_diff is not None:
        print("  tags + metadata:", file=sys.stderr)
        print(tags_meta_diff, file=sys.stderr)
    if prices_diff is not None:
        print("  prices:", file=sys.stderr)
        print(prices_diff, file=sys.stderr)
    if pad_diff is not None:
        print("  pad synthesis:", file=sys.stderr)
        print(pad_diff, file=sys.stderr)
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
