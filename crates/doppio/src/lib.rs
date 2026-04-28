//! doppio — a compiler and query library for the Ledger plain-text
//! accounting format.
//!
//! # `.dop` binary format
//!
//! The `dop compile` command serialises an elaborated journal to a `.dop`
//! file. The file begins with an 8-byte header followed by the payload:
//!
//! ```text
//! Offset  Length  Content
//! 0       4       Magic: b"DOP\0"
//! 4       2       Format version: u16 LE (currently 3)
//! 6       1       Compression: u8  (0 = none, 1 = deflate)
//! 7       1       Reserved (write 0, ignore on read)
//! 8       N       Payload (protobuf, optionally deflate-compressed per byte 6)
//! ```
//!
//! Use [`dop_write_header`] / [`dop_read_header`] for portable, tested I/O of
//! this header.  Use [`write_dop`] / [`read_dop`] for the full
//! header + (optional) compression + protobuf round-trip.
//!
//! # Pipeline
//!
//! Source text is processed through four stages:
//!
//! ```text
//! source text
//!   → [parser]      ast::Journal        (PEG grammar + Pratt expressions)
//!   → [resolution]  resolution::HIR     (dates, aliases, metadata)
//!   → [elaboration] elaboration_pipeline::Journal (evaluation, balancing)
//!   → serialisation                     (protobuf + optional deflate → .dop)
//! ```
//!
//! The top-level entry point is [`compile`], which runs all three in-memory
//! stages and returns the elaborated [`Journal`]. For CLI usage see the
//! `dop` binary in `src/main.rs`.
//!
//! # Modules
//!
//! - [`ast`] — abstract syntax tree produced by the parser.
//! - [`frontend`] — the [`Frontend`] trait for pluggable file-format support.
//! - [`grammars`] — grammar implementations ([`grammars::ledger`] for
//!   ledger-cli; future: hledger).
//! - [`parser`] — re-exported ledger parser types for backwards compatibility.
//! - [`resolution`] — alias resolution, date normalisation, metadata
//!   extraction.
//! - [`elaboration`] — expression evaluation, transaction balancing, and the
//!   final serialisable [`Journal`] type.
//! - [`proto`] — prost-generated Protocol Buffers types (canonical wire shape).
//!
//! # Serialising transactions as Ledger text
//!
//! Use [`write_ledger`] to serialise a sequence of [`resolution::Transaction`]
//! values back to canonical Ledger source text:
//!
//! ```rust
//! # use doppio::resolution::{Transaction, Posting};
//! # use chrono::NaiveDate;
//! let txns = vec![
//!     Transaction::new(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
//!         .with_posting(Posting::new("Expenses:Food").with_amount((
//!             rust_decimal::Decimal::from(50u32), "$",
//!         )))
//!         .with_posting(Posting::new("Assets:Checking")),
//! ];
//! let mut out = Vec::new();
//! doppio::write_ledger(txns, &mut out).unwrap();
//! ```

pub mod ast;
pub mod elaboration_pipeline;
pub mod frontend;
pub mod grammars;
pub mod resolution;

/// Re-export of the ledger parser module for backwards compatibility.
///
/// All items previously at `doppio::parser::*` remain accessible here.
/// New code should prefer [`grammars::ledger`] directly.
pub mod parser {
    pub use crate::grammars::ledger::{LedgerParser, Parser, Rule, parse_ledger};
    // parse_expr is crate-internal; re-exported with the same visibility so
    // ast.rs can still use it via `crate::parser::parse_expr`.
    pub(crate) use crate::grammars::ledger::parse_expr;
}

/// Prost-generated Protocol Buffers types — canonical wire shape of `.dop` bodies.
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/doppio.rs"));
}

mod proto_ext;

pub use elaboration_pipeline::Journal;
pub use frontend::Frontend;
pub use grammars::hledger::HledgerFrontend;
pub use grammars::ledger::LedgerFrontend;

/// Select a frontend by file extension.
///
/// Returns the appropriate [`Frontend`] implementation for `ext`.
/// Dispatch table:
///
/// | Extension | Frontend |
/// |-----------|----------|
/// | `"ledger"` | [`LedgerFrontend`] |
/// | `"hledger"` | [`HledgerFrontend`] |
/// | `"journal"` | [`HledgerFrontend`] |
/// | anything else / `None` | [`LedgerFrontend`] (default) |
///
/// # Example
///
/// ```rust
/// let fe = doppio::frontend_for_extension(Some("ledger"));
/// assert!(fe.extensions().contains(&"ledger"));
///
/// let fe2 = doppio::frontend_for_extension(Some("hledger"));
/// assert!(fe2.extensions().contains(&"hledger"));
///
/// let fe3 = doppio::frontend_for_extension(Some("journal"));
/// assert!(fe3.extensions().contains(&"journal"));
///
/// // Unknown extensions fall back to the ledger frontend.
/// let fe4 = doppio::frontend_for_extension(None);
/// assert!(fe4.extensions().contains(&"ledger"));
/// ```
pub fn frontend_for_extension(ext: Option<&str>) -> Box<dyn Frontend> {
    let frontends: &[&dyn Frontend] = &[&HledgerFrontend, &LedgerFrontend];
    for fe in frontends {
        if ext.is_some_and(|e| fe.extensions().contains(&e)) {
            // Construct an owned Box of the concrete type matched above.
            // We iterate over trait-object refs to find the right frontend,
            // then return a fresh Box of the concrete type.  This avoids
            // cloning or storing the Frontend behind Arc.
            if fe.extensions().contains(&"hledger") || fe.extensions().contains(&"journal") {
                return Box::new(HledgerFrontend);
            } else {
                return Box::new(LedgerFrontend);
            }
        }
    }
    // Default: ledger frontend (preserves existing behaviour for unknown extensions).
    Box::new(LedgerFrontend)
}

// ──────────────────────────────────────────────────────────────────────────────
// Proto conversion: elaboration types ↔ proto wire types
// ──────────────────────────────────────────────────────────────────────────────

/// Convert a `rust_decimal::Decimal` to the proto [`proto::Decimal`] encoding.
///
/// The mantissa is split into low (u64) and high (i64, sign-extended) halves of
/// the 128-bit two's-complement integer, with the scale preserved as-is.
fn decimal_to_proto(d: rust_decimal::Decimal) -> proto::Decimal {
    let mantissa: i128 = d.mantissa();
    let scale = d.scale();
    let mantissa_low = mantissa as u64;
    let mantissa_high = (mantissa >> 64) as i64;
    proto::Decimal {
        mantissa_low,
        mantissa_high,
        scale,
    }
}

/// Reconstruct a `rust_decimal::Decimal` from its [`proto::Decimal`] encoding.
///
/// This is the inverse of the private `decimal_to_proto` helper. It is exposed
/// publicly so that callers working directly with `proto::Journal` (e.g. via
/// [`read_dop_proto`]) can materialise `Decimal` values on demand without
/// going through the full `elaboration_pipeline::Journal` conversion.
pub fn decimal_from_proto(p: &proto::Decimal) -> rust_decimal::Decimal {
    let mantissa = ((p.mantissa_high as i128) << 64) | (p.mantissa_low as i128);
    rust_decimal::Decimal::from_i128_with_scale(mantissa, p.scale)
}

/// Convert an [`elaboration_pipeline::TransactionState`] to its proto enum value (i32).
fn state_to_proto(s: &elaboration_pipeline::TransactionState) -> i32 {
    match s {
        elaboration_pipeline::TransactionState::Uncleared => proto::TransactionState::Uncleared as i32,
        elaboration_pipeline::TransactionState::Pending => proto::TransactionState::Pending as i32,
        elaboration_pipeline::TransactionState::Cleared => proto::TransactionState::Cleared as i32,
    }
}

/// Convert a proto enum i32 to [`elaboration_pipeline::TransactionState`].
///
/// `Unspecified` (0) and unknown values both map to `Uncleared`.
fn state_from_proto(v: i32) -> elaboration_pipeline::TransactionState {
    match proto::TransactionState::try_from(v) {
        Ok(proto::TransactionState::Cleared) => elaboration_pipeline::TransactionState::Cleared,
        Ok(proto::TransactionState::Pending) => elaboration_pipeline::TransactionState::Pending,
        _ => elaboration_pipeline::TransactionState::Uncleared,
    }
}

impl From<&elaboration_pipeline::Journal> for proto::Journal {
    fn from(j: &elaboration_pipeline::Journal) -> Self {
        proto::Journal {
            transactions: j
                .transactions
                .iter()
                .map(|t| proto::Transaction {
                    date: t.date,
                    secondary_date: t.secondary_date,
                    state: state_to_proto(&t.state),
                    code: t.code.clone(),
                    description: t.description.clone(),
                    tags: t.tags.clone(),
                    metadata: t
                        .metadata
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    postings: t
                        .postings
                        .iter()
                        .map(|p| proto::Posting {
                            account: p.account.clone(),
                            payee: p.payee.clone(),
                            amount: Some(proto::Amount {
                                by_commodity: p
                                    .amount
                                    .0
                                    .iter()
                                    .map(|(k, v)| (k.clone(), decimal_to_proto(*v)))
                                    .collect(),
                            }),
                            state: state_to_proto(&p.state),
                            tags: p.tags.clone(),
                            metadata: p
                                .metadata
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
            accounts: j
                .accounts
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        proto::AccountProperties {
                            note: v.note.clone(),
                        },
                    )
                })
                .collect(),
            commodities: j
                .commodities
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        proto::CommodityProperties {
                            format: v.format.clone(),
                            no_market: v.no_market,
                            note: v.note.clone(),
                        },
                    )
                })
                .collect(),
            prices: j
                .prices
                .iter()
                .map(|hp| proto::HistoricalPrice {
                    date: hp.date,
                    time: hp.time.clone(),
                    commodity: hp.commodity.clone(),
                    price: Some(decimal_to_proto(hp.price)),
                    price_commodity: hp.price_commodity.clone(),
                })
                .collect(),
        }
    }
}

impl From<elaboration_pipeline::Journal> for proto::Journal {
    fn from(j: elaboration_pipeline::Journal) -> Self {
        (&j).into()
    }
}

impl From<proto::Journal> for elaboration_pipeline::Journal {
    fn from(p: proto::Journal) -> Self {
        use std::collections::BTreeMap;

        let transactions = p
            .transactions
            .into_iter()
            .map(|t| elaboration_pipeline::ResolvedTransaction {
                date: t.date,
                secondary_date: t.secondary_date,
                state: state_from_proto(t.state),
                code: t.code,
                description: t.description,
                tags: t.tags,
                metadata: t.metadata.into_iter().collect(),
                postings: t
                    .postings
                    .into_iter()
                    .map(|posting| elaboration_pipeline::ResolvedPosting {
                        account: posting.account,
                        payee: posting.payee,
                        amount: elaboration_pipeline::Amount(
                            posting
                                .amount
                                .map(|a| {
                                    a.by_commodity
                                        .into_iter()
                                        .map(|(k, v)| (k, decimal_from_proto(&v)))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        ),
                        state: state_from_proto(posting.state),
                        tags: posting.tags,
                        metadata: posting.metadata.into_iter().collect(),
                    })
                    .collect(),
            })
            .collect();

        let accounts: BTreeMap<_, _> = p
            .accounts
            .into_iter()
            .map(|(k, v)| (k, elaboration_pipeline::AccountProperties { note: v.note }))
            .collect();

        let commodities: BTreeMap<_, _> = p
            .commodities
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    elaboration_pipeline::CommodityProperties {
                        format: v.format,
                        no_market: v.no_market,
                        note: v.note,
                    },
                )
            })
            .collect();

        let prices = p
            .prices
            .into_iter()
            .map(|hp| elaboration_pipeline::HistoricalPrice {
                date: hp.date,
                time: hp.time,
                commodity: hp.commodity,
                price: hp
                    .price
                    .as_ref()
                    .map(decimal_from_proto)
                    .unwrap_or_default(),
                price_commodity: hp.price_commodity,
            })
            .collect();

        elaboration_pipeline::Journal {
            transactions,
            accounts,
            commodities,
            prices,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Public write/read API
// ──────────────────────────────────────────────────────────────────────────────

/// Compression algorithm used in the `.dop` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// No compression — raw protobuf bytes.
    None,
    /// Deflate compression via `miniz_oxide`.
    Deflate,
}

impl Compression {
    fn as_byte(self) -> u8 {
        match self {
            Compression::None => 0,
            Compression::Deflate => 1,
        }
    }

    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Compression::None),
            1 => Some(Compression::Deflate),
            _ => std::option::Option::None,
        }
    }
}

/// Serialise `journal` to `writer` as a complete `.dop` file
/// (8-byte header + optional deflate + protobuf body).
///
/// # Errors
///
/// Propagates any [`std::io::Error`] from `writer`.
pub fn write_dop<W: std::io::Write>(
    journal: &elaboration_pipeline::Journal,
    writer: &mut W,
    compression: Compression,
) -> std::io::Result<()> {
    use prost::Message as _;

    let wire: proto::Journal = journal.into();
    let encoded = wire.encode_to_vec();

    dop_write_header(writer, compression)?;

    let payload = match compression {
        Compression::None => encoded,
        Compression::Deflate => miniz_oxide::deflate::compress_to_vec(&encoded, 6),
    };

    writer.write_all(&payload)
}

/// Deserialise a `.dop` file from `reader` into a [`Journal`].
///
/// `path` is used only in error messages.
///
/// # Errors
///
/// Returns a boxed error if the header is invalid, the compression byte is
/// unrecognised, decompression fails, or protobuf decoding fails.
pub fn read_dop<R: std::io::Read>(
    reader: &mut R,
    path: &std::path::Path,
) -> Result<elaboration_pipeline::Journal, Box<dyn std::error::Error>> {
    use prost::Message as _;

    let compression = dop_read_header(reader, path)?;

    let mut payload = Vec::new();
    reader.read_to_end(&mut payload)?;

    let proto_bytes = match compression {
        Compression::None => payload,
        Compression::Deflate => miniz_oxide::inflate::decompress_to_vec(&payload)
            .map_err(|e| format!("{}: deflate decompression failed: {e:?}", path.display()))?,
    };

    let wire = proto::Journal::decode(proto_bytes.as_slice())
        .map_err(|e| format!("{}: protobuf decode failed: {e}", path.display()))?;

    Ok(elaboration_pipeline::Journal::from(wire))
}

/// Deserialise a `.dop` file from `reader` into a raw [`proto::Journal`],
/// skipping the conversion to [`elaboration_pipeline::Journal`].
///
/// This is the fast path for CLI read-only commands: it performs the header
/// check, optional decompression, and prost decode, but does **not** allocate
/// the `BTreeMap`s, `String` clones, and `Amount` wrappers that
/// `elaboration_pipeline::Journal` requires. Callers iterate `proto::Journal::transactions`
/// directly.
///
/// `path` is used only in error messages.
///
/// # Errors
///
/// Returns a boxed error if the header is invalid, the compression byte is
/// unrecognised, decompression fails, or protobuf decoding fails.
pub fn read_dop_proto<R: std::io::Read>(
    reader: &mut R,
    path: &std::path::Path,
) -> Result<proto::Journal, Box<dyn std::error::Error>> {
    use prost::Message as _;

    let compression = dop_read_header(reader, path)?;

    let mut payload = Vec::new();
    reader.read_to_end(&mut payload)?;

    let proto_bytes = match compression {
        Compression::None => payload,
        Compression::Deflate => miniz_oxide::inflate::decompress_to_vec(&payload)
            .map_err(|e| format!("{}: deflate decompression failed: {e:?}", path.display()))?,
    };

    proto::Journal::decode(proto_bytes.as_slice())
        .map_err(|e| format!("{}: protobuf decode failed: {e}", path.display()))
        .map_err(Into::into)
}

/// Write a sequence of [`resolution::Transaction`] values to `writer` in
/// canonical Ledger source text format.
///
/// Each transaction is formatted using its [`std::fmt::Display`] impl and
/// separated from the next by a blank line. The output is suitable for
/// appending to or creating a `.ledger` source file and round-trips correctly
/// through the parser: `write_ledger(txns)` → parse → resolve should yield
/// semantically equivalent transactions.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if any write to `writer` fails.
///
/// # Example
///
/// ```rust
/// # use doppio::resolution::{Transaction, Posting};
/// # use chrono::NaiveDate;
/// let txns = vec![
///     Transaction::new(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(), "Groceries")
///         .with_posting(Posting::new("Expenses:Food").with_amount((
///             rust_decimal::Decimal::from(50u32), "$",
///         )))
///         .with_posting(Posting::new("Assets:Checking")),
/// ];
/// let mut out = Vec::new();
/// doppio::write_ledger(txns, &mut out).unwrap();
/// let text = String::from_utf8(out).unwrap();
/// assert!(text.starts_with("2024-01-15 Groceries"));
/// ```
pub fn write_ledger<W>(
    entries: impl IntoIterator<Item = resolution::Transaction>,
    writer: &mut W,
) -> std::io::Result<()>
where
    W: std::io::Write,
{
    let mut first = true;
    for txn in entries {
        if !first {
            writeln!(writer)?;
        }
        first = false;
        write!(writer, "{txn}")?;
    }
    Ok(())
}

/// Load and concatenate all files matching a glob pattern.
///
/// This is the default file-opener used by the CLI when processing `include`
/// directives. It is passed to [`parser::Parser`] as the `opener` field.
///
/// ## Glob patterns
///
/// Any path containing `*`, `?`, or `[` is treated as a glob pattern. Matched
/// files are read in **lexicographic order** (sorted by path after expansion)
/// and concatenated into a single string.
///
/// A glob pattern that matches **zero** files is an error — it almost always
/// indicates a misconfigured `include` directive or a missing file tree.
///
/// ## Literal paths
///
/// A path with no glob metacharacters is treated as a single-file include. If
/// the file does not exist, an I/O error is returned.
///
/// ## Errors
///
/// Returns a boxed error if:
/// - `pattern` is not a valid glob expression,
/// - the pattern contains glob metacharacters but matches no files,
/// - a matched path cannot be read (I/O error), or
/// - a literal path does not exist (I/O error).
#[cfg(not(target_family = "wasm"))]
pub fn file_opener(pattern: &str) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Read as _;

    // Collect and sort all matching paths. Sorting ensures lexicographic,
    // deterministic ordering regardless of filesystem traversal order.
    let mut paths: Vec<_> = glob::glob(pattern)?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("glob match error for {pattern:?}: {e}"))?;
    paths.sort();

    // A glob with metacharacters that resolves to nothing is always an error.
    // A plain literal path that doesn't exist is caught below by the file open.
    let is_glob = pattern.contains(['*', '?', '[']);
    if is_glob && paths.is_empty() {
        return Err(format!("include glob {pattern:?} matched no files").into());
    }

    // Literal path: glob returns empty when the file doesn't exist (glob
    // silently skips non-existent literal paths). Detect this early.
    if !is_glob && paths.is_empty() {
        return Err(format!("include: file not found: {pattern}").into());
    }

    let mut buf = String::new();
    for path in &paths {
        // Ensure each appended file starts on a fresh line. If the previous
        // file didn't end with a newline, gluing the next file's first line
        // onto the previous one can change parse meaning (e.g. attach a
        // posting to the wrong transaction).
        if !buf.is_empty() && !buf.ends_with('\n') {
            buf.push('\n');
        }
        std::fs::File::open(path)
            .map_err(|e| format!("include: cannot open {}: {e}", path.display()))?
            .read_to_string(&mut buf)
            .map_err(|e| format!("include: cannot read {}: {e}", path.display()))?;
    }

    Ok(buf)
}

/// Compile Ledger source text into a fully elaborated [`Journal`].
///
/// Runs the three in-memory pipeline stages in sequence:
///
/// 1. [`parser::Parser::parse`] — tokenise `input` into an [`ast::Journal`].
/// 2. [`resolution::HIR::try_from`] — resolve dates, aliases, and metadata.
/// 3. [`elaboration_pipeline::Journal::try_from`] — evaluate amounts and balance
///    transactions.
///
/// The `parser` argument supplies the file-opener for `include` directives and
/// the base path for relative path resolution. For single-file inputs without
/// includes, use [`parser::parse_ledger`] instead.
///
/// # Errors
///
/// Returns a boxed error from the first failing stage (parse error, resolution
/// error, or elaboration error).
pub fn compile<F>(
    input: &str,
    mut parser: parser::Parser<F>,
) -> Result<elaboration_pipeline::Journal, Box<dyn std::error::Error>>
where
    F: Fn(&str) -> Result<String, Box<dyn std::error::Error>>,
{
    let output = parser.parse(input)?;
    let hir: resolution::HIR = output.try_into()?;
    Ok(hir.try_into()?)
}

/// Evaluate a single [`resolution::Transaction`] through the elaboration stage.
///
/// This is the bridge between programmatic transaction construction (via the
/// [`resolution::Transaction`] builder API) and full elaboration. It resolves
/// aliases, evaluates amount expressions, balances postings, and applies cost
/// basis — returning a fully resolved transaction or an error.
///
/// The `context` parameter supplies alias definitions, commodity aliases, and
/// the default commodity. Use [`resolution::Context::default()`] when no
/// aliases or default commodity are needed.
///
/// Internally this constructs a minimal [`resolution::HIR`] containing the
/// single transaction, runs the elaboration pipeline, and extracts the result.
///
/// # Errors
///
/// Returns an [`elaboration_pipeline::ElaborationError`] if the transaction cannot be
/// elaborated (e.g. unbalanced postings, expression evaluation failure, or
/// too many null postings).
///
/// # Example
///
/// ```rust
/// use doppio::resolution::{Context, Transaction, Posting};
/// use chrono::NaiveDate;
/// use rust_decimal::Decimal;
///
/// let txn = Transaction::new(
///     NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
///     "Groceries",
/// )
/// .with_posting(
///     Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "$")),
/// )
/// .with_posting(Posting::new("Assets:Checking"));
///
/// let resolved = doppio::eval_transaction(txn, &Context::default()).unwrap();
/// assert_eq!(resolved.description, "Groceries");
/// assert_eq!(resolved.postings.len(), 2);
/// ```
pub fn eval_transaction(
    txn: resolution::Transaction,
    context: &resolution::Context,
) -> Result<elaboration_pipeline::ResolvedTransaction, elaboration_pipeline::ElaborationError> {
    let hir = resolution::HIR {
        entries: vec![resolution::ResolutionEntry {
            context_id: 0,
            data: resolution::Entry::Transaction(txn),
        }],
        contexts: vec![context.clone()],
        ..Default::default()
    };
    let journal = elaboration_pipeline::Journal::try_from(hir)?;
    // The HIR contained exactly one transaction, so the journal has exactly one.
    Ok(journal
        .transactions
        .into_iter()
        .next()
        .expect("journal should contain exactly one transaction"))
}

// ──────────────────────────────────────────────────────────────────────────────
// .dop header helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Four-byte magic that identifies every `.dop` file.
pub const DOP_MAGIC: [u8; 4] = *b"DOP\0";

/// Format version embedded in every `.dop` header.
///
/// Bump this constant (and update [`dop_read_header`]) whenever the
/// serialisation format changes in a breaking way.
pub const DOP_FORMAT_VERSION: u16 = 3;

/// Write the 8-byte `.dop` header to `writer`.
///
/// Layout: magic (4 bytes) + version LE u16 (2 bytes) +
///         compression byte (1 byte) + reserved byte (1 byte).
///
/// # Errors
///
/// Propagates any [`std::io::Error`] from `writer`.
pub fn dop_write_header<W: std::io::Write>(
    writer: &mut W,
    compression: Compression,
) -> std::io::Result<()> {
    writer.write_all(&DOP_MAGIC)?;
    writer.write_all(&DOP_FORMAT_VERSION.to_le_bytes())?;
    writer.write_all(&[compression.as_byte(), 0u8])?;
    Ok(())
}

/// Read and validate the 8-byte `.dop` header from `reader`.
///
/// Returns the [`Compression`] method declared in the header.
///
/// `path` is used only for error messages; it should be the path of the file
/// being opened so diagnostics point to the right location.
///
/// # Errors
///
/// Returns a boxed error with a user-actionable message if:
/// - the magic bytes are missing or incorrect,
/// - the format version is not [`DOP_FORMAT_VERSION`], or
/// - the compression byte is unrecognised.
pub fn dop_read_header<R: std::io::Read>(
    reader: &mut R,
    path: &std::path::Path,
) -> Result<Compression, Box<dyn std::error::Error>> {
    let mut magic = [0u8; 4];
    // A short read here means the file is too small to be valid.
    reader.read_exact(&mut magic).map_err(|_| {
        format!(
            "{}: not a valid .dop file (missing magic header); \
             recompile from source with `dop compile`",
            path.display()
        )
    })?;
    if magic != DOP_MAGIC {
        return Err(format!(
            "{}: not a valid .dop file (missing magic header); \
             recompile from source with `dop compile`",
            path.display()
        )
        .into());
    }

    let mut version_bytes = [0u8; 2];
    reader.read_exact(&mut version_bytes)?;
    let version = u16::from_le_bytes(version_bytes);
    if version != DOP_FORMAT_VERSION {
        return Err(format!(
            "{}: incompatible .dop format version {} \
             (this binary supports version {}); \
             recompile from source with `dop compile`",
            path.display(),
            version,
            DOP_FORMAT_VERSION,
        )
        .into());
    }

    let mut compression_reserved = [0u8; 2];
    reader.read_exact(&mut compression_reserved)?;
    let compression = Compression::from_byte(compression_reserved[0]).ok_or_else(|| {
        format!(
            "{}: unknown compression byte {} in .dop header",
            path.display(),
            compression_reserved[0],
        )
    })?;
    // byte 7 is reserved — ignored on read.

    Ok(compression)
}

#[cfg(test)]
mod write_ledger_tests {
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Parse a Ledger-format source string and return the resolved transactions.
    fn parse_transactions(source: &str) -> Vec<resolution::Transaction> {
        let mut p = parser::Parser {
            opener: |_: &str| Ok(String::new()),
            base_path: std::path::PathBuf::new(),
        };
        let ast_journal = p.parse(&source.to_string()).expect("parse failed");
        let hir: resolution::HIR = ast_journal.try_into().expect("resolution failed");
        hir.transactions().collect()
    }

    #[test]
    fn write_empty_iterator_produces_no_output() {
        let mut out: Vec<u8> = Vec::new();
        write_ledger(std::iter::empty::<resolution::Transaction>(), &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn write_single_transaction_basic() {
        let txn = resolution::Transaction::new(date(2024, 1, 15), "Groceries")
            .with_posting(
                resolution::Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([txn], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert_eq!(
            text,
            "2024-01-15 Groceries\n    Expenses:Food  50 $\n    Assets:Checking\n"
        );
    }

    #[test]
    fn multiple_transactions_separated_by_blank_line() {
        let txns = vec![
            resolution::Transaction::new(date(2024, 1, 1), "First"),
            resolution::Transaction::new(date(2024, 1, 2), "Second"),
        ];

        let mut out: Vec<u8> = Vec::new();
        write_ledger(txns, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert_eq!(text, "2024-01-01 First\n\n2024-01-02 Second\n");
    }

    #[test]
    fn round_trip_preserves_date_and_description() {
        let original = resolution::Transaction::new(date(2024, 3, 15), "Salary payment")
            .with_state(ast::TransactionState::Cleared)
            .with_posting(
                resolution::Posting::new("Income:Salary")
                    .with_amount((Decimal::from(5000u32), "USD")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([original], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        let parsed = parse_transactions(&text);
        assert_eq!(parsed.len(), 1);
        let roundtripped = &parsed[0];

        assert_eq!(roundtripped.date, date(2024, 3, 15));
        assert_eq!(roundtripped.description, "Salary payment");
        assert!(matches!(roundtripped.state, ast::TransactionState::Cleared));
        assert_eq!(roundtripped.postings.len(), 2);
        assert_eq!(roundtripped.postings[0].account, "Income:Salary");
        assert_eq!(roundtripped.postings[1].account, "Assets:Checking");
    }

    #[test]
    fn round_trip_preserves_metadata_and_tags() {
        let original = resolution::Transaction::new(date(2024, 6, 1), "Grant revenue")
            .with_tag("income")
            .with_comment("Q2 payment")
            .with_comment("approved")
            .with_metadata("program", "Grant:UW:HARVEST")
            .with_metadata("ref", "INV-001")
            .with_posting(
                resolution::Posting::new("Income:Grants")
                    .with_amount((Decimal::from(10_000u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([original], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        let parsed = parse_transactions(&text);
        assert_eq!(parsed.len(), 1);
        let rt = &parsed[0];

        assert!(
            rt.tags.contains(&"income".to_string()),
            "tag 'income' missing from {rt:?}"
        );
        assert!(
            rt.comments.contains(&"Q2 payment".to_string()),
            "comment 'Q2 payment' missing from {rt:?}",
        );
        assert!(
            rt.comments.contains(&"approved".to_string()),
            "comment 'approved' missing from {rt:?}",
        );
        assert_eq!(
            rt.metadata.get("program").map(String::as_str),
            Some("Grant:UW:HARVEST")
        );
        assert_eq!(rt.metadata.get("ref").map(String::as_str), Some("INV-001"));
    }

    #[test]
    fn round_trip_multiple_transactions() {
        let txns = vec![
            resolution::Transaction::new(date(2024, 1, 10), "Food")
                .with_posting(
                    resolution::Posting::new("Expenses:Food")
                        .with_amount((Decimal::from(30u32), "$")),
                )
                .with_posting(resolution::Posting::new("Assets:Checking")),
            resolution::Transaction::new(date(2024, 1, 20), "Rent")
                .with_state(ast::TransactionState::Cleared)
                .with_posting(
                    resolution::Posting::new("Expenses:Rent")
                        .with_amount((Decimal::from(1200u32), "$")),
                )
                .with_posting(resolution::Posting::new("Assets:Checking")),
        ];

        let mut out: Vec<u8> = Vec::new();
        write_ledger(txns, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        let parsed = parse_transactions(&text);
        assert_eq!(parsed.len(), 2);

        assert_eq!(parsed[0].description, "Food");
        assert_eq!(parsed[0].date, date(2024, 1, 10));

        assert_eq!(parsed[1].description, "Rent");
        assert_eq!(parsed[1].date, date(2024, 1, 20));
        assert!(matches!(parsed[1].state, ast::TransactionState::Cleared));
    }

    #[test]
    fn round_trip_posting_with_metadata() {
        let original = resolution::Transaction::new(date(2024, 4, 1), "Payroll")
            .with_posting(
                resolution::Posting::new("Expenses:Salary")
                    .with_amount((Decimal::from(3000u32), "$"))
                    .with_metadata("employee", "alice")
                    .with_tag("payroll"),
            )
            .with_posting(resolution::Posting::new("Assets:Bank"));

        let mut out: Vec<u8> = Vec::new();
        write_ledger([original], &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        let parsed = parse_transactions(&text);
        assert_eq!(parsed.len(), 1);
        let posting = &parsed[0].postings[0];

        assert_eq!(posting.account, "Expenses:Salary");
        assert_eq!(
            posting.metadata.get("employee").map(String::as_str),
            Some("alice")
        );
        assert!(posting.tags.contains(&"payroll".to_string()));
    }
}

#[cfg(test)]
mod eval_transaction_tests {
    use chrono::NaiveDate;
    use rust_decimal::{Decimal, dec};

    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn simple_two_posting_transaction() {
        let txn = resolution::Transaction::new(date(2024, 1, 15), "Groceries")
            .with_posting(
                resolution::Posting::new("Expenses:Food").with_amount((Decimal::from(50u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let resolved = eval_transaction(txn, &resolution::Context::default()).unwrap();

        assert_eq!(resolved.description, "Groceries");
        assert_eq!(resolved.postings.len(), 2);

        let food = resolved
            .postings
            .iter()
            .find(|p| p.account == "Expenses:Food")
            .unwrap();
        assert_eq!(food.amount.0.get("$").copied(), Some(dec!(50)));

        let checking = resolved
            .postings
            .iter()
            .find(|p| p.account == "Assets:Checking")
            .unwrap();
        assert_eq!(checking.amount.0.get("$").copied(), Some(dec!(-50)));
    }

    #[test]
    fn null_posting_inferred() {
        let txn = resolution::Transaction::new(date(2024, 2, 1), "Rent")
            .with_posting(
                resolution::Posting::new("Expenses:Rent")
                    .with_amount((Decimal::from(1200u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let resolved = eval_transaction(txn, &resolution::Context::default()).unwrap();

        let checking = resolved
            .postings
            .iter()
            .find(|p| p.account == "Assets:Checking")
            .unwrap();
        assert_eq!(
            checking.amount.0.get("$").copied(),
            Some(dec!(-1200)),
            "null posting should be inferred as -$1200"
        );
    }

    #[test]
    fn explicit_balanced_amounts() {
        let txn = resolution::Transaction::new(date(2024, 3, 1), "Transfer")
            .with_posting(
                resolution::Posting::new("Assets:Savings")
                    .with_amount((Decimal::from(500u32), "$")),
            )
            .with_posting(
                resolution::Posting::new("Assets:Checking")
                    .with_amount((Decimal::from(-500i32), "$")),
            );

        let resolved = eval_transaction(txn, &resolution::Context::default()).unwrap();
        assert_eq!(resolved.postings.len(), 2);
    }

    #[test]
    fn unbalanced_transaction_returns_error() {
        let txn = resolution::Transaction::new(date(2024, 4, 1), "Bad")
            .with_posting(
                resolution::Posting::new("Expenses:Food").with_amount((Decimal::from(100u32), "$")),
            )
            .with_posting(
                resolution::Posting::new("Assets:Checking")
                    .with_amount((Decimal::from(-50i32), "$")),
            );

        let result = eval_transaction(txn, &resolution::Context::default());
        assert!(
            result.is_err(),
            "unbalanced transaction should return an error"
        );
        assert!(matches!(
            result.unwrap_err(),
            elaboration_pipeline::ElaborationError::TransactionDoesNotBalance(_)
        ));
    }

    #[test]
    fn account_alias_resolved_via_context() {
        let mut context = resolution::Context::default();
        context
            .account_aliases
            .insert("Checking".into(), "Assets:Checking:Mercury:7920".into());

        let txn = resolution::Transaction::new(date(2024, 5, 1), "Deposit")
            .with_posting(
                resolution::Posting::new("Income:Salary")
                    .with_amount((Decimal::from(5000u32), "$")),
            )
            .with_posting(resolution::Posting::new("Checking"));

        let resolved = eval_transaction(txn, &context).unwrap();

        let checking = resolved
            .postings
            .iter()
            .find(|p| p.account == "Assets:Checking:Mercury:7920")
            .expect("alias should resolve to canonical account name");
        assert_eq!(checking.amount.0.get("$").copied(), Some(dec!(-5000)));
    }

    #[test]
    fn default_commodity_from_context() {
        let mut context = resolution::Context::default();
        context.default_commodity = Some("USD".into());

        let bare = ast::ValueExpr::Amount {
            value: Decimal::from(25u32),
            commodity: None,
        };
        let txn = resolution::Transaction::new(date(2024, 6, 1), "Bare amount")
            .with_posting(resolution::Posting::new("Expenses:Food").with_amount(bare))
            .with_posting(resolution::Posting::new("Assets:Cash"));

        let resolved = eval_transaction(txn, &context).unwrap();

        let food = resolved
            .postings
            .iter()
            .find(|p| p.account == "Expenses:Food")
            .unwrap();
        assert_eq!(
            food.amount.0.get("USD").copied(),
            Some(dec!(25)),
            "bare amount should use default commodity from context"
        );
    }

    #[test]
    fn resolved_transaction_preserves_fields() {
        let txn = resolution::Transaction::new(date(2024, 7, 4), "Independence Day")
            .with_state(ast::TransactionState::Cleared)
            .with_code("IND-04")
            .with_secondary_date(date(2024, 7, 5))
            .with_tag("holiday")
            .with_metadata("ref", "USA")
            .with_posting(
                resolution::Posting::new("Expenses:Celebration")
                    .with_amount((Decimal::from(200u32), "$")),
            )
            .with_posting(resolution::Posting::new("Assets:Checking"));

        let resolved = eval_transaction(txn, &resolution::Context::default()).unwrap();

        assert_eq!(resolved.description, "Independence Day");
        assert!(matches!(
            resolved.state,
            elaboration_pipeline::TransactionState::Cleared
        ));
        assert_eq!(resolved.code.as_deref(), Some("IND-04"));
        assert!(resolved.secondary_date.is_some());
        assert!(resolved.tags.contains(&"holiday".to_string()));
        assert_eq!(
            resolved.metadata.get("ref").map(String::as_str),
            Some("USA")
        );
    }

    #[test]
    fn too_many_null_postings_returns_error() {
        let txn = resolution::Transaction::new(date(2024, 8, 1), "Ambiguous")
            .with_posting(resolution::Posting::new("Expenses:A"))
            .with_posting(resolution::Posting::new("Expenses:B"));

        let result = eval_transaction(txn, &resolution::Context::default());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            elaboration_pipeline::ElaborationError::TooManyNullPostings
        ));
    }
}

#[cfg(test)]
mod proto_from_journal_tests {
    use super::*;

    const SOURCE: &str = "\
2024-03-01 Coffee Shop
    Expenses:Food  5 $
    Assets:Cash
";

    fn make_journal() -> elaboration_pipeline::Journal {
        let mut p = parser::Parser {
            opener: |_: &str| Ok(String::new()),
            base_path: std::path::PathBuf::new(),
        };
        let ast = p.parse(&SOURCE.to_string()).expect("parse");
        let hir: resolution::HIR = ast.try_into().expect("resolution");
        hir.try_into().expect("elaboration")
    }

    /// Owned `From` produces the same result as the borrowed form.
    #[test]
    fn owned_and_borrowed_forms_agree() {
        let from_borrow: proto::Journal = (&make_journal()).into();
        let from_owned: proto::Journal = make_journal().into();

        assert_eq!(
            from_owned.transactions.len(),
            from_borrow.transactions.len()
        );
        assert_eq!(
            from_owned.transactions[0].description,
            from_borrow.transactions[0].description
        );
    }

    /// The owned form produces a non-trivial result with the expected description.
    #[test]
    fn owned_form_converts_description() {
        let p: proto::Journal = make_journal().into();

        assert_eq!(p.transactions.len(), 1);
        assert_eq!(p.transactions[0].description, "Coffee Shop");
    }
}
