import type Decimal from "decimal.js";
import type { MonthKey } from "./views/period.js";

const MONTH_NAMES_SHORT = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const MONTH_NAMES_LONG = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];

/**
 * Render a MonthKey as a short label like "Apr 24". Hand-built rather
 * than going through Date + toLocaleDateString — that path treats the
 * Date as UTC and formats in local time, which off-by-one's the month
 * label for any viewer west of UTC.
 */
export function monthLabelShort(m: MonthKey): string {
  return `${MONTH_NAMES_SHORT[m.month - 1]} ${String(m.year).slice(-2)}`;
}

/**
 * Render a MonthKey as a long label like "April 2024". Same rationale
 * as `monthLabelShort`.
 */
export function monthLabelLong(m: MonthKey): string {
  return `${MONTH_NAMES_LONG[m.month - 1]} ${m.year}`;
}

/**
 * Render a (commodity, value) pair for display. Recognises the bare `$`
 * symbol and renders it as a prefix with no space; other commodities
 * are suffixed with a space, mirroring ledger-cli's typical output.
 *
 * The numeric portion is rendered with 2 decimal places and
 * comma thousands separators (US convention) — e.g.
 * `$10,318.88`, `1,234.50 EUR`. The sign sits outside any currency
 * marker: `-$10,318.88`.
 */
export function formatAmount(commodity: string, value: Decimal): string {
  const sign = value.isNegative() ? "-" : "";
  const body = formatDecimal(value.abs(), 2);
  if (commodity === "$") return `${sign}$${body}`;
  return `${sign}${body} ${commodity}`;
}

function formatDecimal(value: Decimal, decimals: number): string {
  const fixed = value.toFixed(decimals);
  const [intPart, fracPart] = fixed.split(".");
  const withCommas = intPart!.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return fracPart === undefined ? withCommas : `${withCommas}.${fracPart}`;
}
