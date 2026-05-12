import Decimal from "decimal.js";
import {
  compareLocalDate,
  type Journal,
  type LocalDate,
} from "@/lib/dop";
import { inferAccountType, type AccountType } from "./accountType.js";
import { dateInRange } from "./filter.js";
import { exchangeRateAt } from "./exchange.js";
import type { HistoricalPrice } from "@/lib/dop";

export interface MonthKey {
  year: number;
  month: number; // 1..12
}

export interface IncomeExpenseBucket {
  month: MonthKey;
  income: Decimal;
  expense: Decimal;
}

export interface CategoryBucket {
  // Top-level expense subcategory (e.g. "Rent" for "Expenses:Rent").
  // For postings on the bare "Expenses" account, the bucket is "Expenses".
  label: string;
  total: Decimal;
}

export interface NetWorthPoint {
  month: MonthKey;
  netWorth: Decimal;
  assets: Decimal;
  liabilities: Decimal;
}

/**
 * When non-null, converts multi-commodity postings to a single display
 * commodity using the journal's P price directives. Passed through to
 * period functions so the same code path serves both "as recorded" (null)
 * and "converted" (non-null) modes.
 */
export interface ConversionContext {
  /** The commodity to convert into. */
  toCommodity: string;
  /** Available historical prices from the journal. */
  prices: HistoricalPrice[];
  /** Upper date cutoff for rate lookups (null = no cutoff). */
  asOf: LocalDate | null;
}

const PRIMARY_COMMODITY = "$";

function monthKey(d: LocalDate): MonthKey {
  return { year: d.year, month: d.month };
}

function compareMonth(a: MonthKey, b: MonthKey): number {
  return a.year - b.year || a.month - b.month;
}

function isInMonth(d: LocalDate, m: MonthKey): boolean {
  return d.year === m.year && d.month === m.month;
}

function endOfMonth(m: MonthKey): LocalDate {
  // Use the JS Date trick: the 0th day of the next month is the last day
  // of this month.
  const next = m.month === 12 ? { year: m.year + 1, month: 1 } : { year: m.year, month: m.month + 1 };
  const d = new Date(Date.UTC(next.year, next.month - 1, 0));
  return { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: d.getUTCDate() };
}

/**
 * Extract the scalar value for a posting amount. When no conversion
 * context is given, uses the primary commodity (USD). When a context is
 * given, sums all commodity values after applying exchange rates; amounts
 * for which no rate is available are excluded from the sum (the caller
 * surfaces those separately).
 */
function resolveAmount(
  byCommodity: Record<string, Decimal>,
  ctx: ConversionContext | null,
): Decimal {
  if (ctx === null) {
    return byCommodity[PRIMARY_COMMODITY] ?? new Decimal(0);
  }
  let total = new Decimal(0);
  for (const [commodity, amount] of Object.entries(byCommodity)) {
    if (amount.isZero()) continue;
    const rate = exchangeRateAt(ctx.prices, commodity, ctx.toCommodity, ctx.asOf);
    if (rate !== null) {
      total = total.plus(amount.mul(rate));
    }
    // Unconvertible amounts are silently excluded here; callers that want
    // to surface them use convertByCommodity from exchange.ts directly.
  }
  return total;
}

/**
 * Iterate the months covered by the journal, in date order.
 * If `begin` / `end` are provided, restrict to months that intersect
 * the [begin, end] window.
 */
function monthsInJournal(
  journal: Journal,
  begin: LocalDate | null,
  end: LocalDate | null,
): MonthKey[] {
  if (journal.transactions.length === 0) return [];
  const dates = journal.transactions.map((t) => t.date);
  const sorted = [...dates].sort(compareLocalDate);
  const first = monthKey(sorted[0]!);
  const last = monthKey(sorted[sorted.length - 1]!);
  const lo = begin ? { year: Math.max(first.year, begin.year), month: 1 } : first;
  const hi = end ? { year: Math.min(last.year, end.year), month: 12 } : last;
  const out: MonthKey[] = [];
  let cur = begin ? { year: begin.year, month: begin.month } : first;
  const stop = end ? { year: end.year, month: end.month } : last;
  while (compareMonth(cur, stop) <= 0) {
    if (compareMonth(cur, lo) >= 0 && compareMonth(cur, hi) <= 0) out.push({ ...cur });
    cur = cur.month === 12 ? { year: cur.year + 1, month: 1 } : { year: cur.year, month: cur.month + 1 };
  }
  return out;
}

/**
 * Bucket postings into per-month income / expense totals. Income postings
 * are flipped to natural signs so income reads positive. When `clearedOnly`
 * is true, pending and uncleared transactions are excluded.
 *
 * When `ctx` is non-null, amounts are converted to `ctx.toCommodity`;
 * unconvertible entries are excluded from the totals.
 */
export function incomeExpenseByMonth(
  journal: Journal,
  begin: LocalDate | null,
  end: LocalDate | null,
  clearedOnly = false,
  ctx: ConversionContext | null = null,
): IncomeExpenseBucket[] {
  const months = monthsInJournal(journal, begin, end);
  const buckets = new Map<string, IncomeExpenseBucket>();
  for (const m of months) {
    buckets.set(`${m.year}-${m.month}`, {
      month: m,
      income: new Decimal(0),
      expense: new Decimal(0),
    });
  }
  for (const t of journal.transactions) {
    if (!dateInRange(t.date, begin, end)) continue;
    if (clearedOnly && t.state !== "cleared") continue;
    const k = `${t.date.year}-${t.date.month}`;
    const b = buckets.get(k);
    if (!b) continue;
    for (const p of t.postings) {
      if (p.kind === "virtualUnbalanced") continue;
      const v = resolveAmount(p.amount.byCommodity, ctx);
      if (v.isZero()) continue;
      const type = inferAccountType(p.account, journal.accounts[p.account]);
      if (type === "income") {
        // Income is credit-normal; flip to read positive.
        b.income = b.income.plus(v.neg());
      } else if (p.account.startsWith("Expenses:") || p.account === "Expenses") {
        // Expenses live outside the four-name heuristic table -- match
        // explicitly so a journal that puts non-expense items under a
        // top-level "Expenses" account is handled the same way doppio's
        // text reports do.
        b.expense = b.expense.plus(v);
      }
    }
  }
  return [...buckets.values()];
}

/**
 * Sum expenses for the given month, grouped by the top-level
 * subcategory (the second segment of the account name). Postings on
 * the bare "Expenses" account roll into a single "Expenses" bucket.
 *
 * Returns categories sorted by total descending, with zero-total
 * categories pruned.
 */
export function expensesByCategory(
  journal: Journal,
  month: MonthKey,
  clearedOnly = false,
  ctx: ConversionContext | null = null,
): CategoryBucket[] {
  const totals = new Map<string, Decimal>();
  for (const t of journal.transactions) {
    if (!isInMonth(t.date, month)) continue;
    if (clearedOnly && t.state !== "cleared") continue;
    for (const p of t.postings) {
      if (p.kind === "virtualUnbalanced") continue;
      if (!(p.account === "Expenses" || p.account.startsWith("Expenses:"))) continue;
      const v = resolveAmount(p.amount.byCommodity, ctx);
      if (v.isZero()) continue;
      const segments = p.account.split(":");
      const label = segments.length === 1 ? "Expenses" : segments[1]!;
      totals.set(label, (totals.get(label) ?? new Decimal(0)).plus(v));
    }
  }
  return [...totals.entries()]
    .filter(([_, v]) => !v.isZero())
    .map(([label, total]) => ({ label, total }))
    .sort((a, b) => b.total.minus(a.total).toNumber());
}

/**
 * Compute month-end snapshots of net worth (assets − liabilities) for
 * every month in the journal's range, in chronological order.
 *
 * Both sides are accumulated using natural signs: assets are debit-normal
 * so their sum is reported as-is; liabilities are credit-normal so their
 * internal sum is negated to read as the amount you owe.
 */
export function netWorthByMonth(
  journal: Journal,
  clearedOnly = false,
  ctx: ConversionContext | null = null,
): NetWorthPoint[] {
  const months = monthsInJournal(journal, null, null);
  const sorted = [...journal.transactions].sort((a, b) => compareLocalDate(a.date, b.date));
  let assets = new Decimal(0);
  let liabilities = new Decimal(0);
  const series: NetWorthPoint[] = [];
  let i = 0;
  for (const m of months) {
    const eom = endOfMonth(m);
    while (i < sorted.length && compareLocalDate(sorted[i]!.date, eom) <= 0) {
      const t = sorted[i]!;
      if (clearedOnly && t.state !== "cleared") {
        i++;
        continue;
      }
      for (const p of t.postings) {
        if (p.kind === "virtualUnbalanced") continue;
        const v = resolveAmount(p.amount.byCommodity, ctx);
        if (v.isZero()) continue;
        const type = inferAccountType(p.account, journal.accounts[p.account]);
        if (type === "assets") {
          assets = assets.plus(v);
        } else if (type === "liabilities") {
          // Liabilities accumulate negatively in raw form; flip so the
          // displayed "amount you owe" reads positive.
          liabilities = liabilities.plus(v.neg());
        }
      }
      i++;
    }
    series.push({ month: m, assets, liabilities, netWorth: assets.minus(liabilities) });
  }
  return series;
}

/**
 * KPI: net worth as of the latest transaction in the journal.
 */
export function netWorthAsOfLatest(
  journal: Journal,
  clearedOnly = false,
  ctx: ConversionContext | null = null,
): {
  netWorth: Decimal;
  assets: Decimal;
  liabilities: Decimal;
} {
  const series = netWorthByMonth(journal, clearedOnly, ctx);
  if (series.length === 0) {
    return { netWorth: new Decimal(0), assets: new Decimal(0), liabilities: new Decimal(0) };
  }
  const last = series[series.length - 1]!;
  return { netWorth: last.netWorth, assets: last.assets, liabilities: last.liabilities };
}

/**
 * KPI: cash on hand -- sum of postings on accounts under
 * `Assets:Bank:` or `Assets:Cash` in the primary commodity. The leaf
 * "checking-style" subset; deliberately excludes brokerage / 401k /
 * crypto so the number reflects actually-spendable balance.
 */
export function cashOnHand(
  journal: Journal,
  clearedOnly = false,
  ctx: ConversionContext | null = null,
): Decimal {
  let total = new Decimal(0);
  for (const t of journal.transactions) {
    if (clearedOnly && t.state !== "cleared") continue;
    for (const p of t.postings) {
      if (p.kind === "virtualUnbalanced") continue;
      if (!isCashAccount(p.account)) continue;
      const v = resolveAmount(p.amount.byCommodity, ctx);
      total = total.plus(v);
    }
  }
  return total;
}

function isCashAccount(account: string): boolean {
  return (
    account === "Assets:Bank" ||
    account.startsWith("Assets:Bank:") ||
    account === "Assets:Cash" ||
    account.startsWith("Assets:Cash:")
  );
}

/**
 * KPI: net (income − expense) over the journal's full span (or the
 * supplied [begin, end] window). Returns the natural-sign sum so
 * positive means money saved.
 */
export function periodNet(
  journal: Journal,
  begin: LocalDate | null,
  end: LocalDate | null,
  clearedOnly = false,
  ctx: ConversionContext | null = null,
): Decimal {
  const buckets = incomeExpenseByMonth(journal, begin, end, clearedOnly, ctx);
  return buckets.reduce(
    (acc, b) => acc.plus(b.income).minus(b.expense),
    new Decimal(0),
  );
}

/**
 * KPI: average monthly expenses over the journal's full span (or
 * supplied [begin, end] window). Months with zero expense activity
 * still count toward the denominator -- the goal is "what's typical
 * monthly burn", not "average of months that had any spending".
 */
export function avgMonthlyExpense(
  journal: Journal,
  begin: LocalDate | null,
  end: LocalDate | null,
  clearedOnly = false,
  ctx: ConversionContext | null = null,
): Decimal {
  const buckets = incomeExpenseByMonth(journal, begin, end, clearedOnly, ctx);
  if (buckets.length === 0) return new Decimal(0);
  const total = buckets.reduce((acc, b) => acc.plus(b.expense), new Decimal(0));
  return total.div(buckets.length);
}

/**
 * The latest month covered by the journal (or the supplied window).
 * Useful for picking which month the category donut renders.
 */
export function latestMonth(
  journal: Journal,
  begin: LocalDate | null,
  end: LocalDate | null,
): MonthKey | null {
  const months = monthsInJournal(journal, begin, end);
  return months.length === 0 ? null : months[months.length - 1]!;
}

export type { AccountType };
