import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import Decimal from "decimal.js";
import { readDop, type Journal } from "doppio-dop";
import {
  avgMonthlyExpense,
  cashOnHand,
  expensesByCategory,
  incomeExpenseByMonth,
  latestMonth,
  netWorthAsOfLatest,
  netWorthByMonth,
  periodNet,
} from "./period.js";

const here = dirname(fileURLToPath(import.meta.url));
const sampleBytes = (() => {
  const b = readFileSync(resolve(here, "../../../public/sample.dop"));
  return new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
})();
const journal: Journal = readDop(sampleBytes);

describe("incomeExpenseByMonth", () => {
  it("emits one bucket per month in the journal's range", () => {
    const buckets = incomeExpenseByMonth(journal, null, null);
    expect(buckets.length).toBe(6); // Jan-Jun 2024
    expect(buckets[0]!.month).toEqual({ year: 2024, month: 1 });
    expect(buckets[5]!.month).toEqual({ year: 2024, month: 6 });
  });

  it("flips income to natural signs so it reads positive", () => {
    const buckets = incomeExpenseByMonth(journal, null, null);
    // Jan has two invoices (3400 each = 6800); the rest have one (3400).
    expect(buckets[0]!.income.equals(new Decimal("6800"))).toBe(true);
    for (let i = 1; i <= 5; i++) {
      expect(buckets[i]!.income.equals(new Decimal("3400"))).toBe(true);
    }
  });

  it("aggregates expenses per month against ledger-cli-equivalent totals", () => {
    const buckets = incomeExpenseByMonth(journal, null, null);
    // Total expenses across all six months sums to the journal's grand
    // expense total -- cross-checks against dop balance Expenses output.
    const total = buckets.reduce((acc, b) => acc.plus(b.expense), new Decimal(0));
    // Sample journal aggregate expenses (USD only): rent 6×1800 + groceries
    // 474.57 + restaurants 153.35 + utilities 434 + transit 528 + travel
    // 683 + entertainment 48.20 = 13121.12.
    expect(total.equals(new Decimal("13121.12"))).toBe(true);
  });

  it("respects begin/end window", () => {
    const buckets = incomeExpenseByMonth(
      journal,
      { year: 2024, month: 2, day: 1 },
      { year: 2024, month: 3, day: 31 },
    );
    expect(buckets.map((b) => b.month)).toEqual([
      { year: 2024, month: 2 },
      { year: 2024, month: 3 },
    ]);
  });
});

describe("expensesByCategory", () => {
  it("returns top-level expense subcategories sorted descending by total", () => {
    const cats = expensesByCategory(journal, { year: 2024, month: 1 });
    expect(cats.length).toBeGreaterThan(0);
    // Rent (1800) should top the list in January.
    expect(cats[0]!.label).toBe("Rent");
    expect(cats[0]!.total.equals(new Decimal("1800"))).toBe(true);
    // Sorted descending.
    for (let i = 1; i < cats.length; i++) {
      expect(cats[i - 1]!.total.gte(cats[i]!.total)).toBe(true);
    }
  });
});

describe("netWorthByMonth", () => {
  it("emits one snapshot per month in chronological order", () => {
    const series = netWorthByMonth(journal);
    expect(series.length).toBe(6);
    for (let i = 1; i < series.length; i++) {
      const a = series[i - 1]!.month;
      const b = series[i]!.month;
      expect(a.year * 12 + a.month).toBeLessThan(b.year * 12 + b.month);
    }
  });

  it("first snapshot reflects the opening balances", () => {
    const series = netWorthByMonth(journal);
    const jan = series[0]!;
    // Opening cash + savings - CC = 4250 + 12000 - 750 = 15500. Then
    // January activity nets out further (rent + utilities out, two
    // salaries in, CC payments). Just verify the asset rollup is at
    // least the opening cash positions.
    expect(jan.assets.gte(new Decimal(15500))).toBe(true);
  });

  it("net worth at the final month matches netWorthAsOfLatest", () => {
    const series = netWorthByMonth(journal);
    const last = series[series.length - 1]!;
    const latest = netWorthAsOfLatest(journal);
    expect(last.netWorth.equals(latest.netWorth)).toBe(true);
  });
});

describe("cashOnHand", () => {
  it("sums Assets:Bank:* and Assets:Cash:* in the primary commodity", () => {
    // Sample: Checking 10318.88 + Savings 13500 = 23818.88. Cash:EUR is
    // tracked in EUR so contributes nothing in USD.
    expect(cashOnHand(journal).equals(new Decimal("23818.88"))).toBe(true);
  });
});

describe("periodNet", () => {
  it("equals income − expense over the journal", () => {
    const net = periodNet(journal, null, null);
    // Income flipped: 7 × 3400 = 23800. Expenses: 13121.12.
    // Net: 23800 - 13121.12 = 10678.88.
    expect(net.equals(new Decimal("10678.88"))).toBe(true);
  });
});

describe("avgMonthlyExpense", () => {
  it("equals total expense / number of months in window", () => {
    const avg = avgMonthlyExpense(journal, null, null);
    expect(avg.toFixed(2)).toBe(new Decimal("13121.12").div(6).toFixed(2));
  });
});

describe("latestMonth", () => {
  it("is the last month covered by the journal", () => {
    expect(latestMonth(journal, null, null)).toEqual({ year: 2024, month: 6 });
  });

  it("respects an end-of-window cap", () => {
    expect(
      latestMonth(journal, null, { year: 2024, month: 3, day: 15 }),
    ).toEqual({ year: 2024, month: 3 });
  });
});
