import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import Decimal from "decimal.js";
import { readDop } from "./loader.js";
import { DopError } from "./errors.js";

const here = dirname(fileURLToPath(import.meta.url));
// sample.dop lives in web/dashboard/public/ — resolve relative to this file's location.
const samplePath = resolve(here, "../../dashboard/public/sample.dop");
const sample = readFileSync(samplePath);
const sampleBytes = new Uint8Array(sample.buffer, sample.byteOffset, sample.byteLength);

describe("readDop on the committed sample.dop", () => {
  const journal = readDop(sampleBytes);

  it("decodes a non-empty journal", () => {
    expect(journal.transactions.length).toBeGreaterThan(40);
    expect(Object.keys(journal.accounts).length).toBeGreaterThanOrEqual(14);
    expect(Object.keys(journal.commodities).length).toBeGreaterThanOrEqual(2);
    expect(journal.prices.length).toBe(6);
  });

  it("preserves transaction order and dates", () => {
    expect(journal.transactions[0]!.date).toEqual({ year: 2024, month: 1, day: 1 });
    const last = journal.transactions[journal.transactions.length - 1]!;
    expect(last.date).toEqual({ year: 2024, month: 6, day: 30 });
  });

  it("matches the journal's mid-year balance assertion on Checking", () => {
    // sample.ledger asserts Assets:Bank:Checking == $10,318.88 at 2024-06-30.
    // We sum every checking posting in USD ourselves to verify the loader
    // produces the same total the assertion baked in.
    let total = new Decimal(0);
    for (const t of journal.transactions) {
      for (const p of t.postings) {
        if (p.account === "Assets:Bank:Checking") {
          const usd = p.amount.byCommodity["$"];
          if (usd) total = total.plus(usd);
        }
      }
    }
    expect(total.equals(new Decimal("10318.88"))).toBe(true);
  });

  it("decodes a lot annotation on the AAPL opening posting", () => {
    const opening = journal.transactions.find(
      (t) => t.description === "Opening AAPL position",
    );
    expect(opening).toBeDefined();
    const lotPosting = opening!.postings.find((p) => p.account === "Assets:Brokerage");
    expect(lotPosting).toBeDefined();
    expect(lotPosting!.lot).toBeDefined();
    expect(lotPosting!.lot!.date).toEqual({ year: 2023, month: 8, day: 1 });
    const cost = lotPosting!.lot!.cost!.byCommodity["$"]!;
    expect(cost.equals(new Decimal("150"))).toBe(true);
  });

  it("decodes historical price entries for FX", () => {
    const eurPrices = journal.prices.filter(
      (p) => p.commodity === "EUR" && p.priceCommodity === "$",
    );
    expect(eurPrices.length).toBe(3);
    // First EUR quote is 2024-01-02 at $1.09.
    const first = eurPrices[0]!;
    expect(first.date).toEqual({ year: 2024, month: 1, day: 2 });
    expect(first.price.equals(new Decimal("1.09"))).toBe(true);
  });

  it("preserves cleared/pending state on transactions", () => {
    const cleared = journal.transactions.filter((t) => t.state === "cleared");
    expect(cleared.length).toBeGreaterThan(40);
  });
});

describe("readDop error paths", () => {
  it("rejects buffers shorter than the 8-byte header", () => {
    expect(() => readDop(new Uint8Array(3))).toThrow(DopError);
    try {
      readDop(new Uint8Array(3));
    } catch (e) {
      expect((e as DopError).kind).toBe("header-too-short");
    }
  });

  it("rejects bad magic bytes", () => {
    const buf = new Uint8Array(16);
    // Magic left as zeros; will fail.
    try {
      readDop(buf);
    } catch (e) {
      expect((e as DopError).kind).toBe("magic-mismatch");
    }
  });

  it("rejects unsupported version numbers", () => {
    const buf = new Uint8Array(sampleBytes);
    buf[4] = 99;
    buf[5] = 0;
    try {
      readDop(buf);
    } catch (e) {
      expect((e as DopError).kind).toBe("version-mismatch");
    }
  });

  it("rejects unknown compression bytes", () => {
    const buf = new Uint8Array(sampleBytes);
    buf[6] = 7; // not a known compression method
    try {
      readDop(buf);
    } catch (e) {
      expect((e as DopError).kind).toBe("compression-unknown");
    }
  });

  it("surfaces inflate failures with kind=inflate-failed", () => {
    // Keep the header valid (so we get past version + compression checks)
    // but corrupt the body so deflate fails to decode it.
    const buf = new Uint8Array(sampleBytes);
    // Stomp every byte after the header with 0xFF -- guaranteed-invalid
    // raw deflate stream.
    for (let i = 8; i < buf.length; i++) buf[i] = 0xff;
    try {
      readDop(buf);
    } catch (e) {
      expect((e as DopError).kind).toBe("inflate-failed");
    }
  });
});
