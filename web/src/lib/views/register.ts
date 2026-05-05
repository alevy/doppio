import Decimal from "decimal.js";
import type { Journal, LocalDate, Posting, Transaction } from "@/lib/dop";
import { filteredPostings, type ViewFilters } from "./filter.js";

export interface RegisterRow {
  date: LocalDate;
  description: string;
  state: Transaction["state"];
  account: string;
  posting: Posting;
  // Running total per commodity AFTER this row is applied. Zero entries
  // are pruned at build time.
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
export function buildRegister(journal: Journal, filters: ViewFilters): RegisterRow[] {
  const running = new Map<string, Decimal>();
  const rows: RegisterRow[] = [];
  for (const { transaction, posting } of filteredPostings(journal, filters)) {
    for (const [c, v] of Object.entries(posting.amount.byCommodity)) {
      running.set(c, (running.get(c) ?? new Decimal(0)).plus(v));
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
      running: snapshot,
    });
  }
  return rows;
}
