import { describe, it, expect } from "vitest";
import Decimal from "decimal.js";
import { decimalFromWire } from "./decimal.js";

// Helper to construct a wire Decimal as the loader sees it: mantissa is
// already split into low (uint64, 0..2^64-1) and high (sint64, signed).
// JS bigint literals are arbitrary-precision; we only require that the
// shape match the Buf-generated type.
function wire(low: bigint, high: bigint, scale: number) {
  return { mantissaLow: low, mantissaHigh: high, scale } as never;
}

describe("decimalFromWire", () => {
  it("decodes zero", () => {
    expect(decimalFromWire(wire(0n, 0n, 0)).toString()).toBe("0");
    expect(decimalFromWire(wire(0n, 0n, 2)).toString()).toBe("0");
  });

  it("decodes simple positive values at scale 0", () => {
    expect(decimalFromWire(wire(1n, 0n, 0)).toString()).toBe("1");
    expect(decimalFromWire(wire(110n, 0n, 0)).toString()).toBe("110");
  });

  it("decodes simple positive values at scale 2", () => {
    // $1.10 -- mantissa 110 with scale 2 means 110 * 10^-2 = 1.1
    expect(decimalFromWire(wire(110n, 0n, 2)).equals(new Decimal("1.10"))).toBe(true);
    // $1,825.00 from sample.ledger
    expect(decimalFromWire(wire(182500n, 0n, 2)).equals(new Decimal("1825.00"))).toBe(true);
  });

  it("decodes negative values via sign-extended high half", () => {
    // -1: mantissaHigh = -1 (all bits set), mantissaLow = max u64 (all bits
    // set). Two's complement for -1 over 128 bits.
    const allOnesU64 = (1n << 64n) - 1n;
    expect(decimalFromWire(wire(allOnesU64, -1n, 0)).toString()).toBe("-1");
    // -1.10 is mantissa -110 at scale 2. Two's complement of 110 in 128
    // bits: low half = 2^64 - 110, high half = -1.
    const low = (1n << 64n) - 110n;
    expect(decimalFromWire(wire(low, -1n, 2)).equals(new Decimal("-1.10"))).toBe(true);
  });

  it("decodes large mantissas that exceed 64 bits", () => {
    // Mantissa = 2^65 = 36893488147419103232; high = 2, low = 0.
    expect(decimalFromWire(wire(0n, 2n, 0)).toString()).toBe("36893488147419103232");
  });

  it("masks low half defensively as unsigned", () => {
    // If a malformed encoder wrote mantissaLow with the high bit set (top
    // bit of the uint64) and mantissaHigh = 0, the result is 2^63, not -2^63.
    expect(decimalFromWire(wire(1n << 63n, 0n, 0)).toString()).toBe("9223372036854775808");
  });

  it("respects the scale (10^-scale)", () => {
    // 12345 with scale 4 -> 1.2345
    expect(decimalFromWire(wire(12345n, 0n, 4)).equals(new Decimal("1.2345"))).toBe(true);
  });
});
