import type Decimal from "decimal.js";

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
