# Exchange rate semantics in doppio

doppio supports converting amounts between commodities using `P` directives
declared in the journal. The sections below cover the **algorithm**, the
**rationale** for the choice, and how it compares to ledger-cli, hledger, and
Beancount. This is the normative spec for cross-language consumers of `.dop`
files reading the `journal.prices` field.

## The algorithm in 6 lines

Given `(from_commodity, to_commodity, as_of)`:

1. If `from == to`, return `1`.
2. Scan `journal.prices` for entries that **directly relate** the pair, in
   either direction:
   - **direct**: `entry.commodity == from` and `entry.price_commodity == to`
   - **inverse**: `entry.commodity == to` and `entry.price_commodity == from`
3. Discard entries whose date is after `as_of` (or all are eligible if
   `as_of` is `None`).
4. Discard entries whose price is zero or absent.
5. From the remainder, pick the **most-recent eligible entry overall** in
   either direction.
6. Return its rate (verbatim if direct; `1 / rate` if inverse). Return
   `None` if no entry survived.

That's it. No graph, no chaining, no path search, no tie-break beyond date
ordering. A consumer in any language can implement this in ~10 lines.

## Reference TypeScript implementation

```ts
function exchangeRateAt(
  prices: HistoricalPrice[],
  from: string,
  to: string,
  asOf: Date | null,
): Decimal | null {
  if (from === to) return new Decimal(1);
  let best: { date: Date; rate: Decimal } | null = null;
  for (const p of prices) {
    const direct = p.commodity === from && p.priceCommodity === to;
    const inverse = p.commodity === to && p.priceCommodity === from;
    if (!direct && !inverse) continue;
    const date = epochDaysToDate(p.date);
    if (asOf && date > asOf) continue;
    const raw = decimalFromProto(p.price);
    if (raw.isZero()) continue;
    const rate = direct ? raw : new Decimal(1).div(raw);
    if (!best || date > best.date) best = { date, rate };
  }
  return best?.rate ?? null;
}
```

10 LOC of substance. Identical structure works in Python, Go, etc. -- the
multilingual recipe in `proto/doppio.proto`'s top comment block expands on
this with idiomatic versions.

## Why direct + inverse only -- no chaining

doppio explicitly **refuses** to compute a rate by chaining through
intermediate commodities. If a journal declares `EUR -> GBP` and
`GBP -> USD` but no direct `EUR <-> USD` quote, `exchange_rate_at("EUR", "USD",
...)` returns `None`. This is intentional:

### 1. PTA exchange rates are inherently estimates

Real markets do not produce a single canonical rate for any pair. EUR->USD
on Tuesday at 09:00 from Vendor A is not the same number as EUR->USD on
Tuesday at 17:00 from Vendor B, and neither equals the inverse of USD->EUR
at the same moment. Bid-ask spread alone creates non-trivial divergence.
The journal records the user's chosen quote at a chosen moment; the
lookup just returns it. Every conversion the journal can express is an
estimate by construction; nobody pretends otherwise.

### 2. Chaining accumulates uncertainty silently

Multi-hop synthesis takes data points the user wrote down -- each subject
to the uncertainty above -- and combines them through multiplication. The
result is a fabricated rate the user never declared. Worse:

- Each chained hop may come from a different vendor or different time of
  day, so the chain mixes incompatible bases.
- Each hop has an implicit transaction cost the journal doesn't model;
  chaining glosses over it.
- "Shortest path" through the graph is not necessarily the cheapest, the
  most liquid, or the path the user actually used to make the conversion.
- A direct path may exist in the real world and just isn't in the ledger
  because nobody recorded it. The chained rate is a guess about a rate
  the user could have looked up.

Calling chained-BFS a "best estimator" makes it sound principled. It
isn't. It's a guess in a graph the user didn't author, and the user has
no signal that it's a guess.

### 3. The "no quote" failure mode is healthier feedback

When a chain doesn't exist, returning `None` and surfacing it ("no
direct quote from EUR to $; leaving 100 EUR unconverted") prompts the
user to record the missing P directive. The journal becomes more
explicit over time. Silently fabricating around the missing data hides
the gap and gradually erodes the user's trust in the report's numbers.

## Comparison: how the other PTA tools handle this

| | doppio (this project) | Beancount | hledger | ledger-cli |
|---|---|---|---|---|
| Direct quote | ✓ | ✓ | ✓ tier 1 | ✓ |
| Inverse quote (1/B->A) | ✓ (most-recent overall wins) | ✓ | ✓ tier 2 | ✓ |
| Chain through intermediates | ✗ -- explicit decision | ✗ -- explicit decision | ✓ tier 3-4 with depth limit | ✓ "as long as desired" per docs |
| Failure mode when no rate | `None` returned | `None` (varies by tool path) | "gave up" message at depth limit | implementation-defined |
| Algorithm complexity | linear scan | linear scan | bounded BFS | under-documented |

**Beancount** explicitly treats currency conversion as **non-transitive**:
"Commodity conversion in beancount is not transitive, so even though you
might have mapping for XXX->GBP and GBP->EUR, beancount won't map XXX to
EUR via GBP." This is the design choice doppio inherits.

**hledger** chains with a tiered fallback (direct -> inverse -> forward
chain -> mixed forward/reverse chain), depth-limited with a "gave up"
failure mode. Documented at <https://hledger.org/currency-conversion.html>.
hledger's depth limit acknowledges what doppio takes a step further:
chain confidence drops with each hop.

**ledger-cli** chains without an obvious depth limit per the official
docs ("equivalence chains can be as long as desired"), but the precise
algorithm is under-documented in the manual; community discussions treat
it as a "ledger does the right thing for typical cases" black box.

doppio's choice aligns with Beancount, *not* with hledger/ledger-cli. A
journal authored against ledger-cli that relies on chained conversion
will surface differently in doppio: the unchained pairs return `None`,
prompting the user to add explicit P directives.

## CLI behaviour

`dop balance --exchange COMMODITY` and `dop register --exchange COMMODITY`
convert each posting amount via `exchange_rate_at(amount.commodity,
target, as_of)`. As-of is the report's `--end` if specified, else `None`
(latest). Commodities for which `exchange_rate_at` returns `None` are left
unconverted in the report, with one stderr warning per missing pair.
Example:

```text
$ dop balance --exchange USD --end 2024-12-31
no direct quote from JPY to USD; leaving 1,000.00 JPY unconverted
                $1,250.00  Assets:Brokerage
                  €100.00 → $110.00  Assets:Cash:Eurozone
                1,000.00 JPY  Assets:Cash:Tokyo
                ────────────
                $1,360.00 + 1,000.00 JPY
```

The mixed-commodity total is the truthful answer when conversion is
incomplete, not a fabricated USD figure. Consumers who want chaining
behaviour can implement it on top of `journal.prices`; the format ships
raw quotes and the default lookup is conservative, leaving the trade-offs
documented above as an explicit opt-in.

## History

This document supersedes earlier doppio behaviour: through v0.4, the
Rust `Journal::exchange_rate_at` did unbounded BFS chaining with inverse
edges and alphabetical tie-breaking -- strictly more aggressive than
hledger's bounded version. The decision to drop chaining was driven by:

- **The "is this is good idea?" pressure test in
  [#158](https://github.com/alevy/doppio/issues/158)**: explored five
  encoding strategies for embedding pre-resolved rates into `.dop` so
  cross-language consumers wouldn't re-implement BFS. The synthetic-data
  measurements (committed to
  [`alevy/doppio-research:prototypes/exchange-rates-baking/FINDINGS.md`](https://github.com/alevy/doppio-research/blob/main/prototypes/exchange-rates-baking/FINDINGS.md))
  surfaced both the wire-cost penalty of correct embedding strategies
  and the semantic divergence of compact ones. None was clearly better
  than baseline.
- **Recognising the BFS itself was over-engineered**, not just hard to
  port to other languages. The reframing question -- "what does the user
  actually want when they write a P directive?" -- pointed at the
  Beancount answer: don't fabricate beyond what was written.

The PR that landed this change closed [#158](https://github.com/alevy/doppio/issues/158)
with the conclusion documented above.
