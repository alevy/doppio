import { describe, it, expect } from "vitest";
import Decimal from "decimal.js";
import type { HistoricalPrice, LocalDate } from "doppio-dop";
import { exchangeRateAt } from "./exchange.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function date(year: number, month: number, day: number): LocalDate {
  return { year, month, day };
}

function price(
  commodity: string,
  priceCommodity: string,
  value: string,
  d: LocalDate,
): HistoricalPrice {
  return { commodity, priceCommodity, price: new Decimal(value), date: d };
}

// ---------------------------------------------------------------------------
// exchangeRateAt
// ---------------------------------------------------------------------------

describe("exchangeRateAt", () => {
  it("same commodity returns exactly 1", () => {
    const rate = exchangeRateAt([], "USD", "USD", null);
    expect(rate).not.toBeNull();
    expect(rate!.equals(new Decimal(1))).toBe(true);
  });

  it("returns null when no prices exist for the pair", () => {
    const prices: HistoricalPrice[] = [
      price("EUR", "GBP", "0.85", date(2024, 1, 1)),
    ];
    expect(exchangeRateAt(prices, "USD", "EUR", null)).toBeNull();
  });

  it("direct quote: P date USD 1.10 EUR → USD→EUR rate = 1.10", () => {
    // "P 2024-01-01 USD 1.10 EUR" → commodity=USD, price=1.10, priceCommodity=EUR
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "1.10", date(2024, 1, 1)),
    ];
    const rate = exchangeRateAt(prices, "USD", "EUR", null);
    expect(rate).not.toBeNull();
    expect(rate!.equals(new Decimal("1.10"))).toBe(true);
  });

  it("inverse quote: P date EUR 1.10 USD → USD→EUR rate = 1/1.10", () => {
    // Direct quote for EUR→USD; invert to get USD→EUR
    const prices: HistoricalPrice[] = [
      price("EUR", "USD", "1.10", date(2024, 1, 1)),
    ];
    const rate = exchangeRateAt(prices, "USD", "EUR", null);
    expect(rate).not.toBeNull();
    // 1 / 1.10 ≈ 0.909090...
    expect(rate!.minus(new Decimal(1).div("1.10")).abs().lessThan("0.000001")).toBe(true);
  });

  it("two direct quotes on different dates: latest at-or-before asOf wins", () => {
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "1.05", date(2024, 1, 1)),
      price("USD", "EUR", "1.12", date(2024, 6, 1)),
      price("USD", "EUR", "1.20", date(2024, 12, 1)),
    ];
    // asOf = 2024-06-15 → picks the 2024-06-01 quote (1.12), not the Dec one
    const rate = exchangeRateAt(prices, "USD", "EUR", date(2024, 6, 15));
    expect(rate).not.toBeNull();
    expect(rate!.equals(new Decimal("1.12"))).toBe(true);
  });

  it("date cutoff: excludes quotes strictly after asOf", () => {
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "1.05", date(2024, 1, 1)),
      price("USD", "EUR", "1.12", date(2024, 6, 1)),
    ];
    // asOf = 2024-02-01 → only the Jan quote is visible
    const rate = exchangeRateAt(prices, "USD", "EUR", date(2024, 2, 1));
    expect(rate).not.toBeNull();
    expect(rate!.equals(new Decimal("1.05"))).toBe(true);
  });

  it("date cutoff: all quotes after asOf → null", () => {
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "1.05", date(2024, 6, 1)),
    ];
    const rate = exchangeRateAt(prices, "USD", "EUR", date(2024, 1, 1));
    expect(rate).toBeNull();
  });

  it("quote exactly on asOf date is included", () => {
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "1.08", date(2024, 3, 15)),
    ];
    const rate = exchangeRateAt(prices, "USD", "EUR", date(2024, 3, 15));
    expect(rate).not.toBeNull();
    expect(rate!.equals(new Decimal("1.08"))).toBe(true);
  });

  it("null asOf means no date cutoff — all prices considered", () => {
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "1.05", date(2024, 1, 1)),
      price("USD", "EUR", "1.20", date(2030, 1, 1)),
    ];
    const rate = exchangeRateAt(prices, "USD", "EUR", null);
    expect(rate).not.toBeNull();
    // Latest wins when asOf is null
    expect(rate!.equals(new Decimal("1.20"))).toBe(true);
  });

  it("among same-date quotes the later one in source order wins (ties)", () => {
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "1.05", date(2024, 1, 1)),
      price("USD", "EUR", "1.09", date(2024, 1, 1)),
    ];
    const rate = exchangeRateAt(prices, "USD", "EUR", null);
    expect(rate).not.toBeNull();
    // The second (later in array) wins on ties
    expect(rate!.equals(new Decimal("1.09"))).toBe(true);
  });

  it("zero-price entries are skipped", () => {
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "0", date(2024, 1, 1)),
      price("USD", "EUR", "1.10", date(2024, 2, 1)),
    ];
    const rate = exchangeRateAt(prices, "USD", "EUR", null);
    expect(rate).not.toBeNull();
    expect(rate!.equals(new Decimal("1.10"))).toBe(true);
  });

  it("zero-price only → null", () => {
    const prices: HistoricalPrice[] = [
      price("USD", "EUR", "0", date(2024, 1, 1)),
    ];
    expect(exchangeRateAt(prices, "USD", "EUR", null)).toBeNull();
  });

  it("unrelated prices are ignored", () => {
    const prices: HistoricalPrice[] = [
      price("EUR", "GBP", "0.85", date(2024, 1, 1)),
      price("CHF", "USD", "1.11", date(2024, 1, 1)),
    ];
    expect(exchangeRateAt(prices, "USD", "EUR", null)).toBeNull();
  });
});
