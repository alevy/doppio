import Decimal from "decimal.js";
import type { Journal, LocalDate, Posting, Transaction } from "@/lib/dop";
import { displaySign } from "./accountType.js";
import { filteredPostings, type ViewFilters } from "./filter.js";

export interface RegisterRow {
  date: LocalDate;
  description: string;
  state: Transaction["state"];
  account: string;
  posting: Posting;
  // Posting amount with the per-account display sign already applied.
  // Identical to `posting.amount.byCommodity` when naturalSigns is off.
  // Zero entries are pruned at build time.
  amount: Record<string, Decimal>;
  // Running total per commodity AFTER this row is applied. Computed
  // from `amount` (sign-aware), not from the raw posting — so swapping
  // naturalSigns at the call site re-computes the running consistently.
  running: Record<string, Decimal>;
}

/**
 * Project the journal into a flat list of register rows: one row per
 * surviving posting, in source order, with running per-commodity totals
 * carried across rows.
 *
 * The running total is computed only over rows that match `filters` —
 * matching ledger-cli's convention that `register Expenses` shows the
 * running total of just-Expenses postings, not of every posting.
 */
export function buildRegister(
  journal: Journal,
  filters: ViewFilters,
  naturalSigns = false,
): RegisterRow[] {
  const running = new Map<string, Decimal>();
  const rows: RegisterRow[] = [];
  for (const { transaction, posting } of filteredPostings(journal, filters)) {
    const sign = displaySign(posting.account, naturalSigns);
    const amount: Record<string, Decimal> = {};
    for (const [c, v] of Object.entries(posting.amount.byCommodity)) {
      const flipped = sign === -1 ? v.neg() : v;
      amount[c] = flipped;
      running.set(c, (running.get(c) ?? new Decimal(0)).plus(flipped));
    }
    const snapshot: Record<string, Decimal> = {};
    for (const [c, v] of running) {
      if (!v.isZero()) snapshot[c] = v;
    }
    rows.push({
      date: transaction.date,
      description: transaction.description,
      state: transaction.state,
      account: posting.account,
      posting,
      amount,
      running: snapshot,
    });
  }
  return rows;
}
