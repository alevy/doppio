//! Integration tests for the `.dop` binary format header.
//!
//! These tests exercise the [`doppio::dop_write_header`] /
//! [`doppio::dop_read_header`] helpers and verify the full compile → load
//! round-trip via the library's public API.

use std::{
    io::{Seek as _, SeekFrom, Write as _},
    path::Path,
};

use tempfile::NamedTempFile;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Compile a small ledger source string into a temp `.dop` file and return the
/// file handle (positioned at offset 0 so the caller can verify raw bytes).
fn compile_to_dop(source: &str) -> NamedTempFile {
    // Build the journal in memory.
    let mut parser = doppio::parser::Parser {
        opener: |_: &str| String::new(),
        base_path: std::path::PathBuf::new(),
    };
    let ast = parser.parse(source).expect("parse failed");
    let hir: doppio::resolution::HIR = ast.try_into().expect("resolution failed");
    let journal: doppio::elaboration::Journal = hir.try_into().expect("elaboration failed");

    // Write to a named temp file so we can pass its path to `dop_read_header`.
    let mut tmp = NamedTempFile::new().expect("tmp file");
    doppio::dop_write_header(tmp.as_file_mut()).expect("write header");
    let mut xz_enc = xz::write::XzEncoder::new(tmp.as_file_mut(), 6);
    {
        let mut buf = std::io::BufWriter::new(&mut xz_enc);
        postcard::to_io(&journal, &mut buf).expect("postcard");
        buf.flush().expect("flush");
    }
    xz_enc.finish().expect("xz finish");

    // Seek back to the beginning so the file is ready for reading.
    tmp.as_file_mut()
        .seek(SeekFrom::Start(0))
        .expect("seek to start");
    tmp
}

/// Load a journal from a named temp file using the library header reader.
fn load_dop(path: &Path) -> doppio::Journal {
    let mut f = std::fs::File::open(path).expect("open dop");
    doppio::dop_read_header(&mut f, path).expect("read header");
    let input_xz = xz::read::XzDecoder::new(f);
    let mut buf = vec![0u8; 102400];
    postcard::from_io((input_xz, &mut buf))
        .expect("deserialise")
        .0
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// A full compile → load round-trip: the deserialized journal should contain
/// the same transactions as the original source.
#[test]
fn round_trip_compile_and_load() {
    let source = "\
2024-01-15 Groceries
    Expenses:Food  50 $
    Assets:Checking

2024-02-01 Rent
    Expenses:Rent  1200 $
    Assets:Checking
";

    let tmp = compile_to_dop(source);
    let journal = load_dop(tmp.path());

    assert_eq!(
        journal.transactions.len(),
        2,
        "expected 2 transactions after round-trip"
    );
    assert_eq!(
        journal.transactions[0].description, "Groceries",
        "first transaction description mismatch"
    );
    assert_eq!(
        journal.transactions[1].description, "Rent",
        "second transaction description mismatch"
    );
}

/// Writing garbage bytes to a `.dop`-named file and attempting to load it
/// should produce an error whose message contains "missing magic header".
#[test]
fn bad_magic_returns_error() {
    let mut tmp = tempfile::Builder::new()
        .suffix(".dop")
        .tempfile()
        .expect("tmp file");
    tmp.write_all(b"GARBAGE_NOT_DOP_FORMAT")
        .expect("write garbage");
    tmp.flush().expect("flush");

    let path = tmp.path().to_owned();
    let mut f = std::fs::File::open(&path).expect("open");
    let err = doppio::dop_read_header(&mut f, &path).expect_err("expected an error for bad magic");

    assert!(
        err.to_string().contains("missing magic header"),
        "error message should mention 'missing magic header', got: {err}"
    );
}

/// An empty file (or file too short for the header) should also be rejected
/// with the "missing magic header" message.
#[test]
fn empty_file_returns_missing_magic_error() {
    let tmp = tempfile::Builder::new()
        .suffix(".dop")
        .tempfile()
        .expect("tmp file");

    let path = tmp.path().to_owned();
    let mut f = std::fs::File::open(&path).expect("open");
    let err = doppio::dop_read_header(&mut f, &path).expect_err("expected an error for empty file");

    assert!(
        err.to_string().contains("missing magic header"),
        "error message should mention 'missing magic header', got: {err}"
    );
}

/// A file with correct magic but wrong version should produce an error that
/// mentions the version number it found.
#[test]
fn wrong_version_returns_error_with_version_number() {
    let mut tmp = tempfile::Builder::new()
        .suffix(".dop")
        .tempfile()
        .expect("tmp file");

    // Write correct magic but version = 99.
    tmp.write_all(b"DOP\0").expect("write magic");
    tmp.write_all(&99u16.to_le_bytes()).expect("write version");
    tmp.write_all(&0u16.to_le_bytes()).expect("write reserved");
    tmp.flush().expect("flush");

    let path = tmp.path().to_owned();
    let mut f = std::fs::File::open(&path).expect("open");
    // Seek is not needed — file was just written and we opened a fresh handle.
    let err =
        doppio::dop_read_header(&mut f, &path).expect_err("expected an error for wrong version");

    let msg = err.to_string();
    assert!(
        msg.contains("incompatible .dop format version"),
        "error message should mention 'incompatible .dop format version', got: {msg}"
    );
    assert!(
        msg.contains("99"),
        "error message should include the bad version number 99, got: {msg}"
    );
}
