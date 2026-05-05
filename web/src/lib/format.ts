import type Decimal from "decimal.js";

/**
 * Render a (commodity, value) pair for display. Recognises the bare `$`
 * symbol and renders it as a prefix with no space; other commodities are
 * suffixed with a space, mirroring ledger-cli's typical output.
 */
export function formatAmount(commodity: string, value: Decimal): string {
  const sign = value.isNegative() ? "-" : "";
  const abs = value.abs().toFixed(2);
  if (commodity === "$") return `${sign}$${abs}`;
  return `${sign}${abs} ${commodity}`;
}
