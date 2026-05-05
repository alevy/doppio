import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import Decimal from "decimal.js";
import { readDop, type Journal } from "@/lib/dop";
import { buildBalanceTree } from "./balance.js";
import { buildRegister } from "./register.js";
import { accountCommodityPairs, buildAccountSeries } from "./chart.js";
import type { ViewFilters } from "./filter.js";

const here = dirname(fileURLToPath(import.meta.url));
const samplePath = resolve(here, "../../../public/sample.dop");
const sampleBytes = (() => {
  const b = readFileSync(samplePath);
  return new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
})();
const journal: Journal = readDop(sampleBytes);

const noFilter: ViewFilters = { pattern: "", clearedOnly: false, begin: null, end: null };

describe("buildBalanceTree", () => {
  it("matches the journal's mid-year checking assertion at the leaf", () => {
    const tree = buildBalanceTree(journal, noFilter, null);
    function find(name: string, nodes = tree): any {
      for (const n of nodes) {
        if (n.fullName === name) return n;
        const hit = find(name, n.children);
        if (hit) return hit;
      }
      return null;
    }
    const checking = find("Assets:Bank:Checking");
    expect(checking).toBeTruthy();
    expect(
      checking.rollupTotals.byCommodity["$"].equals(new Decimal("10318.88")),
    ).toBe(true);
  });

  it("rolls up child totals into their parents", () => {
    const tree = buildBalanceTree(journal, noFilter, null);
    const assets = tree.find((n) => n.fullName === "Assets")!;
    expect(assets).toBeDefined();
    const sumOfChildrenUSD = assets.children.reduce(
      (acc, c) => acc.plus(c.rollupTotals.byCommodity["$"] ?? new Decimal(0)),
      new Decimal(0),
    );
    expect(assets.rollupTotals.byCommodity["$"].equals(sumOfChildrenUSD)).toBe(true);
  });

  it("respects maxDepth by clipping children below the cap", () => {
    const tree = buildBalanceTree(journal, noFilter, 1);
    expect(tree.every((n) => n.children.length === 0)).toBe(true);
    // Rollups must still hold the deep totals — the child data only
    // disappears from the tree, not from the parent's rollup.
    const assets = tree.find((n) => n.fullName === "Assets")!;
    expect(assets.rollupTotals.byCommodity["$"].toFixed(2)).toBe(
      new Decimal("10318.88").plus(new Decimal("13500.00")).toFixed(2),
    );
  });

  it("filters by date range — January only restricts to opening + Jan postings", () => {
    const janOnly = buildBalanceTree(
      journal,
      {
        pattern: "",
        clearedOnly: false,
        begin: { year: 2024, month: 1, day: 1 },
        end: { year: 2024, month: 1, day: 31 },
      },
      null,
    );
    // Some transactions remain (the openings + January activity); not zero,
    // but smaller than the full-period balance on rent.
    const exp = janOnly.find((n) => n.fullName === "Expenses");
    expect(exp).toBeDefined();
    const rent = exp!.children.find((n) => n.fullName === "Expenses:Rent")!;
    expect(rent.rollupTotals.byCommodity["$"].equals(new Decimal("1800.00"))).toBe(true);
  });

  it("substring-filters by account name", () => {
    const tree = buildBalanceTree(
      journal,
      { ...noFilter, pattern: "checking" },
      null,
    );
    // Only the Assets > Bank > Checking branch should contribute.
    const flat: string[] = [];
    function walk(ns: any[]) {
      for (const n of ns) {
        if (Object.keys(n.ownTotals.byCommodity).length > 0) flat.push(n.fullName);
        walk(n.children);
      }
    }
    walk(tree);
    expect(flat).toEqual(["Assets:Bank:Checking"]);
  });

  it("preserves zero-balance accounts (e.g. a credit card cleared each period)", () => {
    const tree = buildBalanceTree(journal, noFilter, null);
    function find(name: string, nodes = tree): any {
      for (const n of nodes) {
        if (n.fullName === name) return n;
        const hit = find(name, n.children);
        if (hit) return hit;
      }
      return null;
    }
    const cc = find("Liabilities:CreditCard");
    expect(cc).toBeTruthy();
    expect(cc.rollupTotals.byCommodity["$"].isZero()).toBe(true);
  });

  it("flips signs for credit-normal subtrees when naturalSigns is on", () => {
    const tree = buildBalanceTree(journal, noFilter, null, true);
    function find(name: string, nodes = tree): any {
      for (const n of nodes) {
        if (n.fullName === name) return n;
        const hit = find(name, n.children);
        if (hit) return hit;
      }
      return null;
    }
    // Income flips: was -23800, displays as +23800.
    expect(find("Income:Consulting").rollupTotals.byCommodity["$"].equals(new Decimal("23800"))).toBe(true);
    // Equity flips: was -18500, displays as +18500.
    expect(find("Equity:OpeningBalances").rollupTotals.byCommodity["$"].equals(new Decimal("18500"))).toBe(true);
    // Assets does NOT flip — Checking stays at +10318.88.
    expect(find("Assets:Bank:Checking").rollupTotals.byCommodity["$"].equals(new Decimal("10318.88"))).toBe(true);
    // Expenses (debit-normal, outside the heuristic table) does NOT flip.
    expect(find("Expenses:Rent").rollupTotals.byCommodity["$"].equals(new Decimal("10800.00"))).toBe(true);
  });
});

describe("buildRegister", () => {
  it("emits one row per matching posting in source order", () => {
    const rows = buildRegister(journal, noFilter);
    // Total postings in the journal = sum of all transactions' posting counts.
    const expected = journal.transactions.reduce((acc, t) => acc + t.postings.length, 0);
    expect(rows.length).toBe(expected);
    // First row date matches first transaction date.
    expect(rows[0]!.date).toEqual(journal.transactions[0]!.date);
  });

  it("running total at the last filtered row matches a flat sum", () => {
    const rows = buildRegister(journal, { ...noFilter, pattern: "expenses:rent" });
    const last = rows[rows.length - 1]!;
    // Expenses:Rent = 6 monthly $1,800 entries.
    expect(last.running["$"].equals(new Decimal("10800.00"))).toBe(true);
  });

  it("clearedOnly drops pending or uncleared transactions", () => {
    const all = buildRegister(journal, noFilter);
    const cleared = buildRegister(journal, { ...noFilter, clearedOnly: true });
    expect(cleared.length).toBeLessThanOrEqual(all.length);
    expect(cleared.every((r) => r.state === "cleared")).toBe(true);
  });

  it("naturalSigns flips per-row amounts and the running total for credit-normal accounts", () => {
    const rows = buildRegister(journal, { ...noFilter, pattern: "income:consulting" }, true);
    expect(rows.length).toBeGreaterThan(0);
    // Each Income row was -3400 raw; with natural signs, it displays as +3400.
    expect(rows[0]!.amount["$"].equals(new Decimal("3400"))).toBe(true);
    // Final running = 7 invoices × 3400 = 23800 (two in January, then monthly Feb-Jun).
    const last = rows[rows.length - 1]!;
    expect(last.running["$"].equals(new Decimal("23800.00"))).toBe(true);
  });

  it("naturalSigns leaves debit-normal accounts unchanged", () => {
    const rows = buildRegister(journal, { ...noFilter, pattern: "expenses:rent" }, true);
    const last = rows[rows.length - 1]!;
    expect(last.running["$"].equals(new Decimal("10800.00"))).toBe(true);
  });
});

describe("buildAccountSeries", () => {
  it("ends at the journal's mid-year checking assertion", () => {
    const series = buildAccountSeries(journal, "Assets:Bank:Checking", "$", null, null);
    expect(series.length).toBeGreaterThan(0);
    const last = series[series.length - 1]!;
    expect(last.value.equals(new Decimal("10318.88"))).toBe(true);
  });

  it("is monotonically dated", () => {
    const series = buildAccountSeries(journal, "Assets:Bank:Checking", "$", null, null);
    for (let i = 1; i < series.length; i++) {
      const a = series[i - 1]!.date;
      const b = series[i]!.date;
      const cmp = a.year - b.year || a.month - b.month || a.day - b.day;
      expect(cmp).toBeLessThan(0);
    }
  });

  it("returns empty when the account never touches the commodity", () => {
    const series = buildAccountSeries(journal, "Assets:Bank:Checking", "EUR", null, null);
    expect(series).toEqual([]);
  });

  it("naturalSigns flips the entire series for credit-normal accounts", () => {
    const raw = buildAccountSeries(journal, "Income:Consulting", "$", null, null);
    const flipped = buildAccountSeries(journal, "Income:Consulting", "$", null, null, true);
    expect(flipped.length).toBe(raw.length);
    for (let i = 0; i < raw.length; i++) {
      expect(flipped[i]!.value.equals(raw[i]!.value.neg())).toBe(true);
    }
    // And the closing point for income reads positive under natural signs.
    expect(flipped[flipped.length - 1]!.value.equals(new Decimal("23800.00"))).toBe(true);
  });

  it("naturalSigns leaves Assets series unchanged", () => {
    const raw = buildAccountSeries(journal, "Assets:Bank:Checking", "$", null, null);
    const flipped = buildAccountSeries(journal, "Assets:Bank:Checking", "$", null, null, true);
    for (let i = 0; i < raw.length; i++) {
      expect(flipped[i]!.value.equals(raw[i]!.value)).toBe(true);
    }
  });
});

describe("accountCommodityPairs", () => {
  it("includes Assets:Bank:Checking $ and Assets:Cash:EUR EUR", () => {
    const pairs = accountCommodityPairs(journal);
    expect(pairs).toContainEqual({ account: "Assets:Bank:Checking", commodity: "$" });
    expect(pairs).toContainEqual({ account: "Assets:Cash:EUR", commodity: "EUR" });
  });

  it("returns pairs sorted by (account, commodity)", () => {
    const pairs = accountCommodityPairs(journal);
    for (let i = 1; i < pairs.length; i++) {
      const cmp =
        pairs[i - 1]!.account.localeCompare(pairs[i]!.account) ||
        pairs[i - 1]!.commodity.localeCompare(pairs[i]!.commodity);
      expect(cmp).toBeLessThanOrEqual(0);
    }
  });
});
