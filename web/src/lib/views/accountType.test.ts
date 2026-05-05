import { describe, it, expect } from "vitest";
import { displaySign, inferAccountType } from "./accountType.js";

describe("inferAccountType", () => {
  it("recognises the four canonical top-level segments", () => {
    expect(inferAccountType("Income:Consulting")).toBe("income");
    expect(inferAccountType("Assets:Bank:Checking")).toBe("assets");
    expect(inferAccountType("Liabilities:CreditCard")).toBe("liabilities");
    expect(inferAccountType("Equity:OpeningBalances")).toBe("equity");
  });

  it("matches on the top-level segment only, not sub-segments", () => {
    expect(inferAccountType("Investments:Income")).toBeNull();
    expect(inferAccountType("Holdings:Assets")).toBeNull();
  });

  it("returns null for top-level segments outside the heuristic table", () => {
    expect(inferAccountType("Expenses:Rent")).toBeNull();
    expect(inferAccountType("MyMadeUpTop:Foo")).toBeNull();
  });

  it("is case-sensitive — the canonical names use TitleCase", () => {
    expect(inferAccountType("income:Salary")).toBeNull();
    expect(inferAccountType("ASSETS:Bank")).toBeNull();
  });

  it("handles accounts with no colon (top-level only)", () => {
    expect(inferAccountType("Income")).toBe("income");
    expect(inferAccountType("Equity")).toBe("equity");
  });
});

describe("displaySign", () => {
  it("returns 1 for every account when naturalSigns is off", () => {
    expect(displaySign("Income:Consulting", false)).toBe(1);
    expect(displaySign("Liabilities:CreditCard", false)).toBe(1);
    expect(displaySign("Assets:Bank:Checking", false)).toBe(1);
    expect(displaySign("Expenses:Rent", false)).toBe(1);
  });

  it("flips signs for credit-normal types when naturalSigns is on", () => {
    expect(displaySign("Income:Consulting", true)).toBe(-1);
    expect(displaySign("Liabilities:CreditCard", true)).toBe(-1);
    expect(displaySign("Equity:OpeningBalances", true)).toBe(-1);
  });

  it("does not flip Assets or Expenses (debit-normal / unknown)", () => {
    expect(displaySign("Assets:Bank:Checking", true)).toBe(1);
    expect(displaySign("Expenses:Rent", true)).toBe(1);
    expect(displaySign("Whatever:Foo", true)).toBe(1);
  });
});
