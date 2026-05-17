import { fromBinary } from "@bufbuild/protobuf";
import { inflateRaw } from "pako";
import {
  JournalSchema,
  PostingKind as WirePostingKind,
  TransactionState as WireTransactionState,
  type Amount as WireAmount,
  type Journal as WireJournal,
  type Lot as WireLot,
  type Posting as WirePosting,
  type Transaction as WireTransaction,
  type HistoricalPrice as WireHistoricalPrice,
} from "./proto/generated/doppio_pb.js";
import { decimalFromWire } from "./decimal.js";
import { localDateFromEpochDays } from "./date.js";
import { DopError } from "./errors.js";
import type {
  Amount,
  Journal,
  Lot,
  Posting,
  Transaction,
  TransactionState,
  PostingKind,
  HistoricalPrice,
} from "./types.js";

// 8-byte header layout, matching `dop_write_header` in the Rust crate:
//   bytes 0..4  -- magic "DOP\0"
//   bytes 4..6  -- format version, little-endian u16
//   byte  6     -- compression method (0 = none, 1 = deflate)
//   byte  7     -- reserved
const MAGIC = Uint8Array.of(0x44, 0x4f, 0x50, 0x00); // "DOP\0"
const SUPPORTED_VERSION = 3;
const HEADER_LEN = 8;

/**
 * Decode a `.dop` artifact into the public-shape `Journal`.
 *
 * The flow is fixed by the file format:
 *   1. parse and validate the 8-byte header,
 *   2. inflate the body if the header declares deflate,
 *   3. decode the body as `proto::Journal` via Buf,
 *   4. walk the wire shape into the public TS shape (Decimal/LocalDate
 *      substituted; everything else passthrough).
 *
 * Errors thrown are always `DopError` with a `kind` discriminator so the
 * UI can branch on header/version/inflate/protobuf failure modes
 * separately.
 */
export function readDop(buf: Uint8Array): Journal {
  if (buf.length < HEADER_LEN) {
    throw new DopError(
      "header-too-short",
      `not a .dop file: input is only ${buf.length} bytes (header alone is ${HEADER_LEN})`,
    );
  }

  for (let i = 0; i < MAGIC.length; i++) {
    if (buf[i] !== MAGIC[i]) {
      throw new DopError(
        "magic-mismatch",
        "not a .dop file: magic bytes do not match \"DOP\\0\"",
      );
    }
  }

  const version = buf[4]! | (buf[5]! << 8); // little-endian u16
  if (version !== SUPPORTED_VERSION) {
    throw new DopError(
      "version-mismatch",
      `incompatible .dop format version ${version} (this build supports version ${SUPPORTED_VERSION}); recompile from source with \`dop compile\``,
    );
  }

  const compression = buf[6]!;
  // byte 7 is reserved; ignored on read.

  const compressed = buf.subarray(HEADER_LEN);
  let body: Uint8Array;
  if (compression === 0) {
    body = compressed;
  } else if (compression === 1) {
    try {
      body = inflateRaw(compressed);
    } catch (cause) {
      throw new DopError("inflate-failed", "deflate decompression failed", { cause });
    }
  } else {
    throw new DopError(
      "compression-unknown",
      `unknown compression byte ${compression} in .dop header`,
    );
  }

  let wire: WireJournal;
  try {
    wire = fromBinary(JournalSchema, body);
  } catch (cause) {
    throw new DopError("protobuf-decode-failed", "protobuf body failed to decode", { cause });
  }

  return convertJournal(wire);
}

function convertJournal(j: WireJournal): Journal {
  return {
    transactions: j.transactions.map(convertTransaction),
    accounts: Object.fromEntries(
      Object.entries(j.accounts).map(([name, a]) => [
        name,
        { note: a.note, metadata: { ...a.metadata } },
      ]),
    ),
    commodities: Object.fromEntries(
      Object.entries(j.commodities).map(([name, c]) => [
        name,
        { format: c.format, noMarket: c.noMarket, note: c.note },
      ]),
    ),
    prices: j.prices.map(convertHistoricalPrice),
  };
}

function convertTransaction(t: WireTransaction): Transaction {
  return {
    date: localDateFromEpochDays(t.date),
    secondaryDate:
      t.secondaryDate !== undefined ? localDateFromEpochDays(t.secondaryDate) : undefined,
    state: convertTransactionState(t.state),
    code: t.code,
    description: t.description,
    tags: [...t.tags],
    metadata: { ...t.metadata },
    postings: t.postings.map(convertPosting),
  };
}

function convertPosting(p: WirePosting): Posting {
  if (!p.amount) {
    throw new DopError(
      "missing-required-field",
      `posting on account "${p.account}" is missing its amount`,
    );
  }
  return {
    account: p.account,
    payee: p.payee,
    amount: convertAmount(p.amount),
    state: convertTransactionState(p.state),
    tags: [...p.tags],
    metadata: { ...p.metadata },
    kind: convertPostingKind(p.kind),
    lot: p.lot ? convertLot(p.lot) : undefined,
  };
}

function convertAmount(a: WireAmount): Amount {
  const byCommodity: Record<string, import("decimal.js").default> = {};
  for (const [commodity, decimal] of Object.entries(a.byCommodity)) {
    byCommodity[commodity] = decimalFromWire(decimal);
  }
  return { byCommodity };
}

function convertLot(l: WireLot): Lot {
  return {
    cost: l.cost ? convertAmount(l.cost) : undefined,
    date: l.date !== undefined ? localDateFromEpochDays(l.date) : undefined,
    note: l.note,
  };
}

function convertHistoricalPrice(p: WireHistoricalPrice): HistoricalPrice {
  if (!p.price) {
    throw new DopError(
      "missing-required-field",
      `historical price for ${p.commodity} -> ${p.priceCommodity} is missing its price`,
    );
  }
  return {
    date: localDateFromEpochDays(p.date),
    time: p.time,
    commodity: p.commodity,
    price: decimalFromWire(p.price),
    priceCommodity: p.priceCommodity,
  };
}

function convertTransactionState(s: WireTransactionState): TransactionState {
  switch (s) {
    case WireTransactionState.PENDING:
      return "pending";
    case WireTransactionState.CLEARED:
      return "cleared";
    // UNSPECIFIED is treated as UNCLEARED by all consumers (matches Rust).
    default:
      return "uncleared";
  }
}

function convertPostingKind(k: WirePostingKind): PostingKind {
  switch (k) {
    case WirePostingKind.VIRTUAL_UNBALANCED:
      return "virtualUnbalanced";
    case WirePostingKind.VIRTUAL_BALANCED:
      return "virtualBalanced";
    // UNSPECIFIED defaults to REAL per the schema's evolution rule.
    default:
      return "real";
  }
}
