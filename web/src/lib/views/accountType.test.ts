import { describe, it, expect } from "vitest";
import { inferAccountType } from "./accountType.js";

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
