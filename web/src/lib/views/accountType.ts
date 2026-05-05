// Heuristic mapping of an account's top-level segment to one of the
// four canonical PTA account types. Anything outside this table
// (Expenses, Investments, freeform tops) is treated as an unknown type.
//
// Future work: an explicit `; type: X` tag on `account` directives
// (mirroring hledger's convention) should override this heuristic. That
// requires plumbing structured metadata through doppio's elaborator —
// tracked as #168.

export type AccountType = "income" | "assets" | "liabilities" | "equity";

const TOP_LEVEL_TYPE: Record<string, AccountType> = {
  Income: "income",
  Assets: "assets",
  Liabilities: "liabilities",
  Equity: "equity",
};

/**
 * Infer the canonical account type from the first colon-separated
 * segment of the account name. Returns null if the segment is not in
 * the heuristic table — callers should treat that as unknown.
 */
export function inferAccountType(account: string): AccountType | null {
  const top = account.split(":", 1)[0];
  if (top === undefined) return null;
  return TOP_LEVEL_TYPE[top] ?? null;
}
