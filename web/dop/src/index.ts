// Public surface of the JS-native .dop reader.
//
// Usage:
//   import { readDop } from "doppio-dop";
//   const journal = readDop(new Uint8Array(await (await fetch("/sample.dop")).arrayBuffer()));
//
// All decode errors throw `DopError` with a `kind` discriminator. See
// `errors.ts` for the kinds.
export { readDop } from "./loader.js";
export { DopError } from "./errors.js";
export type { DopErrorKind } from "./errors.js";
export {
  localDateFromEpochDays,
  epochDaysFromLocalDate,
  localDateToString,
  compareLocalDate,
  localDateToJSDate,
  localDateFromJSDate,
} from "./date.js";
export type { LocalDate } from "./date.js";
export type {
  Journal,
  Transaction,
  Posting,
  Amount,
  Lot,
  AccountProperties,
  CommodityProperties,
  HistoricalPrice,
  PostingKind,
  TransactionState,
} from "./types.js";
