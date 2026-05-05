import Decimal from "decimal.js";
import {
  compareLocalDate,
  type Journal,
  type LocalDate,
} from "@/lib/dop";
import { dateInRange } from "./filter.js";

export interface ChartPoint {
  date: LocalDate;
  value: Decimal;
}

/**
 * Build a per-account balance time series in a chosen commodity.
 *
 * Iterates every transaction (sorted by date), accumulates the running
 * balance contributed by `account` postings in `commodity`, and emits a
 * data point on every date where the balance changes. Dates without a
 * change are not duplicated — Chart.js's line chart spans them
 * naturally.
 *
 * Virtual-unbalanced postings are excluded; they don't move the real
 * balance.
 *
 * Returns an empty array if no postings on `account` ever touch
 * `commodity`.
 */
export function buildAccountSeries(
  journal: Journal,
  account: string,
  commodity: string,
  begin: LocalDate | null,
  end: LocalDate | null,
): ChartPoint[] {
  const sorted = [...journal.transactions].sort((a, b) =>
    compareLocalDate(a.date, b.date),
  );
  let balance = new Decimal(0);
  const points: ChartPoint[] = [];
  let lastDate: LocalDate | null = null;
  for (const t of sorted) {
    if (!dateInRange(t.date, begin, end)) continue;
    let dayChange = new Decimal(0);
    for (const p of t.postings) {
      if (p.kind === "virtualUnbalanced") continue;
      if (p.account !== account) continue;
      const v = p.amount.byCommodity[commodity];
      if (v) dayChange = dayChange.plus(v);
    }
    if (dayChange.isZero()) continue;
    balance = balance.plus(dayChange);
    // Coalesce same-day points to a single end-of-day value.
    if (lastDate && compareLocalDate(lastDate, t.date) === 0) {
      points[points.length - 1] = { date: t.date, value: balance };
    } else {
      points.push({ date: t.date, value: balance });
    }
    lastDate = t.date;
  }
  return points;
}

/**
 * Enumerate the (account, commodity) pairs that have at least one
 * non-virtual posting in the journal. Useful for populating chart
 * controls.
 */
export function accountCommodityPairs(journal: Journal): { account: string; commodity: string }[] {
  const seen = new Set<string>();
  const pairs: { account: string; commodity: string }[] = [];
  for (const t of journal.transactions) {
    for (const p of t.postings) {
      if (p.kind === "virtualUnbalanced") continue;
      for (const c of Object.keys(p.amount.byCommodity)) {
        const key = `${p.account}${c}`;
        if (seen.has(key)) continue;
        seen.add(key);
        pairs.push({ account: p.account, commodity: c });
      }
    }
  }
  pairs.sort(
    (a, b) =>
      a.account.localeCompare(b.account) || a.commodity.localeCompare(b.commodity),
  );
  return pairs;
}
