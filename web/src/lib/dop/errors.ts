export type DopErrorKind =
  | "header-too-short"
  | "magic-mismatch"
  | "version-mismatch"
  | "compression-unknown"
  | "inflate-failed"
  | "protobuf-decode-failed"
  | "missing-required-field";

/**
 * A typed error produced by `readDop`. The `kind` discriminator lets
 * callers branch on the failure mode (e.g. "ask user to recompile" for
 * version-mismatch vs "the file is corrupt" for the inflate cases).
 */
export class DopError extends Error {
  readonly kind: DopErrorKind;

  constructor(kind: DopErrorKind, message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "DopError";
    this.kind = kind;
  }
}
