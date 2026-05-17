import { describe, it, expect } from "vitest";
import { inferAccountType } from "./accountType.js";
import type { AccountProperties } from "doppio-dop";

const noProps: undefined = undefined;
function withType(value: string): AccountProperties {
  return { metadata: { type: value } };
}

describe("inferAccountType — top-level heuristic (no explicit metadata)", () => {
  it("recognises the four canonical top-level segments", () => {
    expect(inferAccountType("Income:Consulting", noProps)).toBe("income");
    expect(inferAccountType("Assets:Bank:Checking", noProps)).toBe("assets");
    expect(inferAccountType("Liabilities:CreditCard", noProps)).toBe("liabilities");
    expect(inferAccountType("Equity:OpeningBalances", noProps)).toBe("equity");
  });

  it("matches on the top-level segment only, not sub-segments", () => {
    expect(inferAccountType("Investments:Income", noProps)).toBeNull();
    expect(inferAccountType("Holdings:Assets", noProps)).toBeNull();
  });

  it("returns null for top-level segments outside the heuristic table", () => {
    expect(inferAccountType("Expenses:Rent", noProps)).toBeNull();
    expect(inferAccountType("MyMadeUpTop:Foo", noProps)).toBeNull();
  });

  it("is case-sensitive — the canonical names use TitleCase", () => {
    expect(inferAccountType("income:Salary", noProps)).toBeNull();
    expect(inferAccountType("ASSETS:Bank", noProps)).toBeNull();
  });

  it("handles accounts with no colon (top-level only)", () => {
    expect(inferAccountType("Income", noProps)).toBe("income");
    expect(inferAccountType("Equity", noProps)).toBe("equity");
  });
});

describe("inferAccountType — explicit `type:` metadata", () => {
  it("accepts hledger one-letter codes", () => {
    expect(inferAccountType("Whatever", withType("A"))).toBe("assets");
    expect(inferAccountType("Whatever", withType("L"))).toBe("liabilities");
    expect(inferAccountType("Whatever", withType("E"))).toBe("equity");
    expect(inferAccountType("Whatever", withType("R"))).toBe("income");
  });

  it("accepts spelled-out forms case-insensitively", () => {
    expect(inferAccountType("Whatever", withType("Asset"))).toBe("assets");
    expect(inferAccountType("Whatever", withType("LIABILITIES"))).toBe("liabilities");
    expect(inferAccountType("Whatever", withType(" income "))).toBe("income");
    expect(inferAccountType("Whatever", withType("Revenue"))).toBe("income");
  });

  it("overrides the heuristic when both apply", () => {
    // Top-level is "Income" (heuristic would say income), but explicit
    // type:A says it's actually an asset. Explicit wins.
    expect(inferAccountType("Income:Holdings", withType("A"))).toBe("assets");
  });

  it("falls back to the heuristic when the value is unrecognised", () => {
    expect(inferAccountType("Income:Salary", withType("garbage"))).toBe("income");
    expect(inferAccountType("Whatever", withType(""))).toBeNull();
  });
});
