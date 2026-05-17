/**
 * Exchange-rate lookup mirroring `Journal::exchange_rate_at` in Rust
 * (`crates/doppio/src/elaboration_ext.rs`).
 *
 * Rules (identical to the Rust implementation):
 *  - Same commodity → 1.
 *  - Direct quote `P date from price to` → use the latest quote at-or-before as-of.
 *  - Inverse quote `P date to price from` → use 1/price, same date selection.
 *  - No chaining through intermediates (deliberate; aligns with Beancount).
 *  - No rate found → null.
 */

import Decimal from "decimal.js";
import type { HistoricalPrice, LocalDate } from "doppio-dop";
import { compareLocalDate, epochDaysFromLocalDate } from "doppio-dop";

/**
 * Convert a `LocalDate` to a comparable epoch-days number so we can do
 * straightforward numeric comparisons in the sort below.
 */
function toEpochDays(d: LocalDate): number {
  return epochDaysFromLocalDate(d);
}

/**
 * Look up the exchange rate from `fromCommodity` to `toCommodity`,
 * using historical prices at-or-before `asOf`. When `asOf` is null all
 * available prices are considered (equivalent to using the journal's
 * max date as the cutoff).
 *
 * Returns `null` when no direct or inverse quote is available.
 */
export function exchangeRateAt(
  prices: HistoricalPrice[],
  fromCommodity: string,
  toCommodity: string,
  asOf: LocalDate | null,
): Decimal | null {
  if (fromCommodity === toCommodity) {
    return new Decimal(1);
  }

  const asOfDays = asOf !== null ? toEpochDays(asOf) : null;

  let bestDays: number | null = null;
  let bestRate: Decimal | null = null;

  for (const hp of prices) {
    const direct = hp.commodity === fromCommodity && hp.priceCommodity === toCommodity;
    const inverse = hp.commodity === toCommodity && hp.priceCommodity === fromCommodity;
    if (!direct && !inverse) continue;

    // Date must be at-or-before the cutoff (when one is set).
    const days = toEpochDays(hp.date);
    if (asOfDays !== null && days > asOfDays) continue;

    // Price must be non-zero to avoid degenerate inversions.
    if (hp.price.isZero()) continue;

    const rate = direct ? hp.price : new Decimal(1).div(hp.price);

    // Latest by date wins. Ties resolve in source order: the last equal
    // element wins (matches "more-recently-declared" in the Rust version
    // which uses `max_by_key`, and `max_by_key` returns the last equal).
    if (bestDays === null || days >= bestDays) {
      bestDays = days;
      bestRate = rate;
    }
  }

  return bestRate;
}

/**
 * Convert a `byCommodity` map to the target commodity, accumulating
 * converted amounts. Returns the converted total and a list of commodity
 * symbols that could not be converted (no rate available).
 *
 * Entries already in the target commodity are included as-is (rate = 1).
 */
export function convertByCommodity(
  byCommodity: Record<string, Decimal>,
  toCommodity: string,
  prices: HistoricalPrice[],
  asOf: LocalDate | null,
): { total: Decimal; unconvertible: string[] } {
  let total = new Decimal(0);
  const unconvertible: string[] = [];

  for (const [commodity, amount] of Object.entries(byCommodity)) {
    if (amount.isZero()) continue;
    const rate = exchangeRateAt(prices, commodity, toCommodity, asOf);
    if (rate === null) {
      unconvertible.push(commodity);
    } else {
      total = total.plus(amount.mul(rate));
    }
  }

  return { total, unconvertible };
}

/**
 * Collect every distinct commodity symbol that appears in the price
 * table (both as the priced commodity and as the price commodity).
 * Sorted alphabetically.
 */
export function commoditiesFromPrices(prices: HistoricalPrice[]): string[] {
  const set = new Set<string>();
  for (const hp of prices) {
    set.add(hp.commodity);
    set.add(hp.priceCommodity);
  }
  return [...set].sort();
}

/**
 * Collect every distinct commodity that appears on any posting amount
 * across all transactions. Sorted alphabetically.
 */
export function commoditiesFromPostings(
  transactions: import("doppio-dop").Transaction[],
): string[] {
  const set = new Set<string>();
  for (const t of transactions) {
    for (const p of t.postings) {
      for (const commodity of Object.keys(p.amount.byCommodity)) {
        set.add(commodity);
      }
    }
  }
  return [...set].sort();
}

/**
 * Union of commodities from prices and from postings, sorted
 * alphabetically. This is the full candidate list for the display
 * commodity dropdown.
 */
export function allCommodities(
  prices: HistoricalPrice[],
  transactions: import("doppio-dop").Transaction[],
): string[] {
  const set = new Set([
    ...commoditiesFromPrices(prices),
    ...commoditiesFromPostings(transactions),
  ]);
  return [...set].sort();
}

// Re-export compareLocalDate so callers don't need to import it separately.
export { compareLocalDate };
