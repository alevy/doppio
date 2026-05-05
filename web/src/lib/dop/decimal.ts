import Decimal from "decimal.js";
import type { Decimal as WireDecimal } from "../proto/generated/doppio_pb.js";

const MASK_64 = (1n << 64n) - 1n;

/**
 * Reassemble a wire-shape Decimal (split 128-bit mantissa + scale) into
 * a `decimal.js` Decimal. The `mantissaHigh` half carries sign; the
 * `mantissaLow` half is masked defensively so that a uint64 wire value
 * is treated as the lower 64 bits of two's complement, not as a signed
 * value.
 *
 * See `proto/doppio.proto`'s top comment for the canonical algorithm and
 * its language ports.
 */
export function decimalFromWire(p: WireDecimal): Decimal {
  const mantissa = (p.mantissaHigh << 64n) | (p.mantissaLow & MASK_64);
  // decimal.js parses the bigint string directly, then we apply the scale.
  // (Decimal accepts a signed string, including "-" prefix, of arbitrary length.)
  const d = new Decimal(mantissa.toString());
  return p.scale === 0 ? d : d.div(new Decimal(10).pow(p.scale));
}
