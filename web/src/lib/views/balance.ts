import Decimal from "decimal.js";
import type { Journal } from "@/lib/dop";
import { displaySign } from "./accountType.js";
import { filteredPostings, type ViewFilters } from "./filter.js";

export interface BalanceTotals {
  // commodity → total. Zero entries are pruned at build time.
  byCommodity: Record<string, Decimal>;
}

export interface BalanceNode {
  // Last segment of the account name (e.g. "Checking" for "Assets:Bank:Checking").
  segment: string;
  // Full colon-joined account name.
  fullName: string;
  // Depth from the root (top-level account = 1).
  depth: number;
  // Postings landing on THIS account exactly.
  ownTotals: BalanceTotals;
  // Own + all descendants, rolled up.
  rollupTotals: BalanceTotals;
  children: BalanceNode[];
}

/**
 * Build a hierarchical balance tree from a Journal under the given filters.
 *
 * - Account names are split on `:` and assembled into a tree where every
 *   intermediate segment becomes a node, even if no posting lands on it
 *   directly.
 * - Each node carries its own posting totals (`ownTotals`) and its
 *   subtree-rolled-up totals (`rollupTotals`).
 * - Children are returned in lexicographic order by segment.
 * - `maxDepth` (if non-null) limits the tree to that many levels —
 *   anything deeper is rolled into the closest ancestor's `rollupTotals`
 *   (which already includes it) and not emitted as its own node.
 */
export function buildBalanceTree(
  journal: Journal,
  filters: ViewFilters,
  maxDepth: number | null,
  naturalSigns = false,
): BalanceNode[] {
  // Step 1: accumulate own-totals per full account name.
  const own = new Map<string, Map<string, Decimal>>();
  for (const { posting: p } of filteredPostings(journal, filters)) {
    let totals = own.get(p.account);
    if (!totals) {
      totals = new Map();
      own.set(p.account, totals);
    }
    const sign = displaySign(p.account, naturalSigns);
    for (const [c, v] of Object.entries(p.amount.byCommodity)) {
      const flipped = sign === -1 ? v.neg() : v;
      totals.set(c, (totals.get(c) ?? new Decimal(0)).plus(flipped));
    }
  }

  // Step 2: assemble nodes for every segment, including ancestors with no
  // direct postings.
  const nodes = new Map<string, BalanceNode>(); // fullName → node
  function ensureNode(fullName: string): BalanceNode {
    const existing = nodes.get(fullName);
    if (existing) return existing;
    const segments = fullName.split(":");
    const node: BalanceNode = {
      segment: segments[segments.length - 1]!,
      fullName,
      depth: segments.length,
      ownTotals: { byCommodity: {} },
      rollupTotals: { byCommodity: {} },
      children: [],
    };
    nodes.set(fullName, node);
    if (segments.length > 1) {
      const parentName = segments.slice(0, -1).join(":");
      const parent = ensureNode(parentName);
      parent.children.push(node);
    }
    return node;
  }
  for (const [fullName, totals] of own) {
    const node = ensureNode(fullName);
    for (const [c, v] of totals) {
      node.ownTotals.byCommodity[c] = v;
    }
  }

  // Step 3: roll up. Post-order traversal sums own + children.
  function rollup(node: BalanceNode) {
    const acc = new Map<string, Decimal>();
    for (const [c, v] of Object.entries(node.ownTotals.byCommodity)) {
      acc.set(c, (acc.get(c) ?? new Decimal(0)).plus(v));
    }
    for (const child of node.children) {
      rollup(child);
      for (const [c, v] of Object.entries(child.rollupTotals.byCommodity)) {
        acc.set(c, (acc.get(c) ?? new Decimal(0)).plus(v));
      }
    }
    // Keep zero rollups so accounts that net to zero (e.g. a credit card
    // whose charges and payments cancel within the period) still surface
    // with an explicit "$0.00" instead of disappearing. Commodities the
    // subtree never touched are simply absent from `acc`.
    for (const [c, v] of acc) {
      node.rollupTotals.byCommodity[c] = v;
    }
  }

  // Sort children by segment.
  for (const node of nodes.values()) {
    node.children.sort((a, b) => a.segment.localeCompare(b.segment));
  }

  // Top-level nodes are those whose fullName has no `:` parent; equivalently,
  // those that no other node lists as a child.
  const tops: BalanceNode[] = [];
  for (const node of nodes.values()) {
    if (node.depth === 1) tops.push(node);
  }
  tops.sort((a, b) => a.segment.localeCompare(b.segment));
  for (const t of tops) rollup(t);

  // Step 4: apply maxDepth by pruning children below the cap.
  if (maxDepth !== null) {
    function prune(node: BalanceNode) {
      if (node.depth >= maxDepth!) {
        node.children = [];
      } else {
        for (const c of node.children) prune(c);
      }
    }
    for (const t of tops) prune(t);
  }

  return tops;
}
