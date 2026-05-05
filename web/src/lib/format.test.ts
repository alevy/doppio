import { describe, it, expect } from "vitest";
import Decimal from "decimal.js";
import { formatAmount, monthLabelLong, monthLabelShort } from "./format.js";

describe("formatAmount", () => {
  it("renders zero with two decimals", () => {
    expect(formatAmount("$", new Decimal(0))).toBe("$0.00");
  });

  it("uses comma thousands separators for values >= 1,000", () => {
    expect(formatAmount("$", new Decimal("1000"))).toBe("$1,000.00");
    expect(formatAmount("$", new Decimal("10318.88"))).toBe("$10,318.88");
    expect(formatAmount("$", new Decimal("1234567.89"))).toBe("$1,234,567.89");
  });

  it("does not insert a separator below 1,000", () => {
    expect(formatAmount("$", new Decimal("999.99"))).toBe("$999.99");
    expect(formatAmount("$", new Decimal("0.50"))).toBe("$0.50");
  });

  it("places the negative sign before the currency marker", () => {
    expect(formatAmount("$", new Decimal("-10318.88"))).toBe("-$10,318.88");
    expect(formatAmount("$", new Decimal("-0.01"))).toBe("-$0.01");
  });

  it("formats non-USD commodities with a trailing space and commodity symbol", () => {
    expect(formatAmount("EUR", new Decimal("1234.50"))).toBe("1,234.50 EUR");
    expect(formatAmount("EUR", new Decimal("-1234.50"))).toBe("-1,234.50 EUR");
    expect(formatAmount("AAPL", new Decimal("30"))).toBe("30.00 AAPL");
  });
});

describe("monthLabelShort / monthLabelLong", () => {
  it("renders all 12 months without going through Date", () => {
    for (let m = 1; m <= 12; m++) {
      expect(monthLabelShort({ year: 2024, month: m })).toMatch(/^[A-Z][a-z]{2} 24$/);
      expect(monthLabelLong({ year: 2024, month: m })).toMatch(/^[A-Z][a-z]+ 2024$/);
    }
  });

  it("matches expected string for known months", () => {
    expect(monthLabelShort({ year: 2024, month: 1 })).toBe("Jan 24");
    expect(monthLabelShort({ year: 2024, month: 6 })).toBe("Jun 24");
    expect(monthLabelLong({ year: 2024, month: 4 })).toBe("April 2024");
    expect(monthLabelLong({ year: 2024, month: 12 })).toBe("December 2024");
  });

  it("handles years outside 2000–2099 sensibly", () => {
    expect(monthLabelShort({ year: 1999, month: 12 })).toBe("Dec 99");
    expect(monthLabelLong({ year: 1999, month: 12 })).toBe("December 1999");
  });
});
