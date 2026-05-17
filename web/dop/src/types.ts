// Public TS shape of an elaborated journal. Mirrors the Rust
// `doppio::elaboration::Journal` structure, with the only substitutions:
//   - protobuf Decimal     -> decimal.js Decimal
//   - epoch-days sint32    -> LocalDate
//   - protobuf maps        -> Record<string, T>  (already what Buf emits)
// Field names follow camelCase per TypeScript convention; the wire is
// lower_snake_case but Buf's TS plugin already converts it for us.
//
// All structural decisions and naming follow `proto/doppio.proto`. Any
// field-level documentation lives there; this file intentionally keeps
// types lean so the schema's comment block stays the single source of
// truth.

import type Decimal from "decimal.js";
import type { LocalDate } from "./date.js";

export type TransactionState = "uncleared" | "pending" | "cleared";

export type PostingKind = "real" | "virtualUnbalanced" | "virtualBalanced";

export interface Amount {
  byCommodity: Record<string, Decimal>;
}

export interface Lot {
  cost?: Amount;
  date?: LocalDate;
  note?: string;
}

export interface Posting {
  account: string;
  payee: string;
  amount: Amount;
  state: TransactionState;
  tags: string[];
  metadata: Record<string, string>;
  kind: PostingKind;
  lot?: Lot;
}

export interface Transaction {
  date: LocalDate;
  secondaryDate?: LocalDate;
  state: TransactionState;
  code?: string;
  description: string;
  tags: string[];
  metadata: Record<string, string>;
  postings: Posting[];
}

export interface AccountProperties {
  note?: string;
  // Inherited key-value metadata. The compiler denormalises by walking
  // colon-separated ancestors at elaboration time, so consumers see a
  // fully-resolved map per account and never need to do the inheritance
  // walk themselves.
  metadata: Record<string, string>;
}

export interface CommodityProperties {
  format?: string;
  noMarket: boolean;
  note?: string;
}

export interface HistoricalPrice {
  date: LocalDate;
  time?: string;
  commodity: string;
  price: Decimal;
  priceCommodity: string;
}

export interface Journal {
  transactions: Transaction[];
  accounts: Record<string, AccountProperties>;
  commodities: Record<string, CommodityProperties>;
  prices: HistoricalPrice[];
}
