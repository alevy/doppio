// Account-type classification. Two layers:
//
//   1. An explicit `; type: <letter>` (or `; type: <name>`) tag on the
//      `account` directive. doppio's elaborator denormalises this
//      across the colon-separated hierarchy, so a sub-account whose
//      ancestor declared `; type: A` reports the same metadata.
//   2. A four-name fallback heuristic on the top-level segment for
//      journals that don't declare types.
//
// Layer (1) wins when present. The heuristic is the safety net.

import type { AccountProperties } from "@/lib/dop";

export type AccountType = "income" | "assets" | "liabilities" | "equity";

const TOP_LEVEL_TYPE: Record<string, AccountType> = {
  Income: "income",
  Assets: "assets",
  Liabilities: "liabilities",
  Equity: "equity",
};

// hledger uses one-letter codes (A/L/E/R/X) and full words; doppio
// itself doesn't assign meaning to the value of `type:` — the
// dashboard interprets it. We accept the codes hledger documents plus
// the spelled-out forms (case-insensitive) so a user reading the
// hledger manual gets the result they expect.
const TYPE_TAG_VALUES: Record<string, AccountType> = {
  a: "assets",
  asset: "assets",
  assets: "assets",
  l: "liabilities",
  liability: "liabilities",
  liabilities: "liabilities",
  e: "equity",
  equity: "equity",
  r: "income",
  revenue: "income",
  revenues: "income",
  income: "income",
};

function typeFromTag(value: string | undefined): AccountType | null {
  if (!value) return null;
  return TYPE_TAG_VALUES[value.trim().toLowerCase()] ?? null;
}

/**
 * Resolve an account's type from an explicit `; type:` tag on its
 * AccountProperties (or an ancestor's, since the compiler denormalises
 * the metadata) — falling back to the top-level-segment heuristic when
 * no tag is present.
 *
 * Returns null when neither layer recognises the account, leaving the
 * caller to decide on a default behaviour (typically: don't flip
 * signs; render as-is).
 */
export function inferAccountType(
  account: string,
  properties: AccountProperties | undefined,
): AccountType | null {
  const explicit = typeFromTag(properties?.metadata.type);
  if (explicit) return explicit;
  const top = account.split(":", 1)[0];
  if (top === undefined) return null;
  return TOP_LEVEL_TYPE[top] ?? null;
}
