import { describe, it, expect } from "vitest";
import {
  compareLocalDate,
  epochDaysFromLocalDate,
  localDateFromEpochDays,
  localDateFromJSDate,
  localDateToJSDate,
  localDateToString,
} from "./date.js";

describe("localDateFromEpochDays", () => {
  it("decodes the Unix epoch as 1970-01-01", () => {
    expect(localDateFromEpochDays(0)).toEqual({ year: 1970, month: 1, day: 1 });
  });

  it("decodes negative epoch days as pre-1970 dates", () => {
    expect(localDateFromEpochDays(-1)).toEqual({ year: 1969, month: 12, day: 31 });
    // 1900-01-01: 70 years before 1970, with 17 leap years interspersed.
    expect(localDateFromEpochDays(-25567)).toEqual({ year: 1900, month: 1, day: 1 });
  });

  it("matches a known date around the sample journal's window", () => {
    // 2024-01-01: 54 years after epoch including 13 leap days.
    // Cross-check against `Date.UTC` to avoid hand-computing.
    const epochDaysOf2024Jan1 = Date.UTC(2024, 0, 1) / 86_400_000;
    expect(localDateFromEpochDays(epochDaysOf2024Jan1)).toEqual({
      year: 2024,
      month: 1,
      day: 1,
    });
  });

  it("handles end-of-month and month-end transitions", () => {
    expect(localDateFromEpochDays(Date.UTC(2024, 1, 29) / 86_400_000)).toEqual({
      year: 2024,
      month: 2,
      day: 29,
    }); // 2024 is a leap year
    expect(localDateFromEpochDays(Date.UTC(2023, 1, 28) / 86_400_000)).toEqual({
      year: 2023,
      month: 2,
      day: 28,
    }); // 2023 is not
  });
});

describe("epochDaysFromLocalDate (round-trip with localDateFromEpochDays)", () => {
  it("round-trips zero and small offsets", () => {
    for (const days of [0, 1, -1, 365, -365, 36500]) {
      const ld = localDateFromEpochDays(days);
      expect(epochDaysFromLocalDate(ld)).toBe(days);
    }
  });

  it("round-trips a range spanning 200 years", () => {
    // ~73,000 days; sample 5,000 of them deterministically.
    for (let days = -25_000; days <= 50_000; days += 137) {
      const ld = localDateFromEpochDays(days);
      expect(epochDaysFromLocalDate(ld)).toBe(days);
    }
  });
});

describe("localDateToString", () => {
  it("zero-pads month, day, and year", () => {
    expect(localDateToString({ year: 2024, month: 1, day: 5 })).toBe("2024-01-05");
    expect(localDateToString({ year: 7, month: 12, day: 31 })).toBe("0007-12-31");
  });
});

describe("compareLocalDate", () => {
  it("orders by year, then month, then day", () => {
    const a = { year: 2024, month: 1, day: 1 };
    const b = { year: 2024, month: 1, day: 2 };
    const c = { year: 2024, month: 2, day: 1 };
    const d = { year: 2025, month: 1, day: 1 };
    expect(compareLocalDate(a, a)).toBe(0);
    expect(compareLocalDate(a, b)).toBeLessThan(0);
    expect(compareLocalDate(b, c)).toBeLessThan(0);
    expect(compareLocalDate(c, d)).toBeLessThan(0);
    expect(compareLocalDate(d, a)).toBeGreaterThan(0);
  });
});

describe("localDateToJSDate / localDateFromJSDate", () => {
  it("round-trips through a UTC-midnight Date", () => {
    const original = { year: 2024, month: 5, day: 17 };
    expect(localDateFromJSDate(localDateToJSDate(original))).toEqual(original);
  });

  it("anchors to UTC, not local time", () => {
    const d = localDateToJSDate({ year: 2024, month: 1, day: 1 });
    expect(d.getUTCFullYear()).toBe(2024);
    expect(d.getUTCMonth()).toBe(0);
    expect(d.getUTCDate()).toBe(1);
    expect(d.getUTCHours()).toBe(0);
  });
});
