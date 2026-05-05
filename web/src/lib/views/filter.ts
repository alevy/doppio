import { compareLocalDate, type Journal, type Posting, type Transaction, type LocalDate } from "@/lib/dop";

/**
 * Filters shared across the three views. The filter store carries the same
 * fields; this type makes the pure-function utilities testable without a
 * Pinia harness.
 */
export interface ViewFilters {
  pattern: string;
  clearedOnly: boolean;
  begin: LocalDate | null;
  end: LocalDate | null;
}

/**
 * Test whether a transaction's date sits inside the [begin, end] window.
 * Either bound may be null to leave that side unbounded.
 */
export function dateInRange(
  date: LocalDate,
  begin: LocalDate | null,
  end: LocalDate | null,
): boolean {
  if (begin && compareLocalDate(date, begin) < 0) return false;
  if (end && compareLocalDate(date, end) > 0) return false;
  return true;
}

/**
 * Lower-cased substring match against an account name. Empty pattern
 * matches everything.
 */
export function accountMatches(account: string, pattern: string): boolean {
  if (!pattern) return true;
  return account.toLowerCase().includes(pattern.toLowerCase());
}

/**
 * Walk every (transaction, posting) pair that survives the filters, in
 * source order. Yields a (txn, posting) tuple per surviving posting.
 *
 * - Date-range filter applies to the transaction date.
 * - clearedOnly drops the entire transaction unless its state is "cleared".
 * - account-name pattern is per-posting (a transaction may contribute some
 *   postings and not others).
 * - virtual-unbalanced postings are excluded by default (they don't
 *   participate in real balances).
 */
export function* filteredPostings(
  journal: Journal,
  filters: ViewFilters,
): Generator<{ transaction: Transaction; posting: Posting }> {
  for (const t of journal.transactions) {
    if (!dateInRange(t.date, filters.begin, filters.end)) continue;
    if (filters.clearedOnly && t.state !== "cleared") continue;
    for (const p of t.postings) {
      if (p.kind === "virtualUnbalanced") continue;
      if (!accountMatches(p.account, filters.pattern)) continue;
      yield { transaction: t, posting: p };
    }
  }
}
