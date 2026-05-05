// Heuristic mapping of an account's top-level segment to one of the four
// canonical PTA account types. Anything outside this table (Expenses,
// Investments, freeform tops) is treated as an unknown type for which
// no sign correction is applied.
//
// The four names are deliberate. Expenses is intentionally absent: it is
// debit-normal like Assets, so leaving it out has the same display effect
// (no sign flip) as classifying it explicitly. The shorter table is
// easier to maintain and to reason about.
//
// Future work: an explicit `; type: X` tag on `account` directives
// (mirroring hledger's convention) should override this heuristic. That
// requires plumbing structured metadata through doppio's elaborator,
// which the current proto schema doesn't yet support — tracked as a
// follow-up issue.

export type AccountType = "income" | "assets" | "liabilities" | "equity";

const TOP_LEVEL_TYPE: Record<string, AccountType> = {
  Income: "income",
  Assets: "assets",
  Liabilities: "liabilities",
  Equity: "equity",
};

/**
 * Infer the canonical account type from the first colon-separated segment
 * of the account name. Returns null if the segment is not in the
 * heuristic table — callers should treat that as "unknown / debit-normal".
 */
export function inferAccountType(account: string): AccountType | null {
  const top = account.split(":", 1)[0];
  if (top === undefined) return null;
  return TOP_LEVEL_TYPE[top] ?? null;
}

/**
 * Sign multiplier to apply to an account's posted amounts when the user
 * has asked for "natural signs" display (the default in the demo UI).
 *
 * Returns -1 for credit-normal account types so that the displayed
 * value matches what a non-accountant intuits ("Income: \$3,400" reads
 * as money earned, not money lost). Returns 1 otherwise — including for
 * unknown account types so we err on the side of not flipping signs we
 * aren't confident about.
 *
 * When `naturalSigns` is false, always returns 1 — the raw double-entry
 * convention. This is the "accountant view" useful for verifying that
 * a balance equation holds.
 */
export function displaySign(account: string, naturalSigns: boolean): 1 | -1 {
  if (!naturalSigns) return 1;
  const t = inferAccountType(account);
  if (t === "income" || t === "liabilities" || t === "equity") return -1;
  return 1;
}
