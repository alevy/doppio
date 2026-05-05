// Public surface of the JS-native .dop reader. Implementation lands in #151.
//
// Intended shape (subject to refinement during #151):
//   export function readDop(buf: Uint8Array): Journal;
//
// where Journal is a TS-native projection of proto::Journal with:
//   - Decimal values eagerly converted to decimal.js Decimal
//   - Dates carried as { year, month, day } LocalDate triples
//   - oneof fields normalised to discriminated unions
//
// The wire-shape types (proto::Journal etc.) are generated into ./generated/
// at build time via `buf generate`. The decoder layer translates between the
// wire shape and the public TS shape so views never depend on protobuf
// internals.
export {};
