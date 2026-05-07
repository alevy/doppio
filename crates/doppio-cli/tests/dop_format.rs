//! Integration tests for the `.dop` binary format header and round-trip.
//!
//! These tests exercise the public [`doppio::write_dop`] /
//! [`doppio::read_dop`] API: header parsing, error paths for bad
//! magic / empty file / wrong version, and the full compile -> load
//! round-trip.

use std::{io::Write as _, path::Path};

use tempfile::NamedTempFile;

// -- helpers ------------------------------------------------------------------

/// Compile a small ledger source string into a temp `.dop` file (deflate
/// compressed) and return the file handle (positioned at offset 0).
fn compile_to_dop(source: &str) -> NamedTempFile {
    compile_to_dop_with_compression(source, doppio::Compression::Deflate)
}

/// Compile with an explicit compression setting.
fn compile_to_dop_with_compression(
    source: &str,
    compression: doppio::Compression,
) -> NamedTempFile {
    // Build the journal in memory.
    let mut parser = doppio::grammars::ledger::Parser {
        opener: |_: &str| Ok(String::new()),
        base_path: std::path::PathBuf::new(),
    };
    let ast = parser.parse(source).expect("parse failed");
    let hir: doppio::resolution::HIR = ast.try_into().expect("resolution failed");
    let journal: doppio::elaboration::Journal =
        doppio::elaborate(hir, &doppio::grammars::ledger::ledger_defaults())
            .expect("elaboration failed");

    // Write to a named temp file so we can pass its path to `read_dop`.
    let mut tmp = NamedTempFile::new().expect("tmp file");
    doppio::write_dop(&journal, tmp.as_file_mut(), compression).expect("write_dop");

    // Seek back to the beginning so the file is ready for reading.
    use std::io::Seek as _;
    tmp.as_file_mut()
        .seek(std::io::SeekFrom::Start(0))
        .expect("seek to start");
    tmp
}

/// Load a journal from a named temp file using the library's public reader.
fn load_dop(path: &Path) -> doppio::Journal {
    let mut f = std::fs::File::open(path).expect("open dop");
    doppio::read_dop(&mut f, path).expect("read_dop")
}

// -- tests --------------------------------------------------------------------─

/// A full compile -> load round-trip using deflate compression: the deserialized
/// journal should contain the same transactions as the original source.
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

/// Round-trip with no compression (`Compression::None`) also works.
#[test]
fn round_trip_no_compression() {
    let source = "\
2024-03-01 Coffee
    Expenses:Coffee  5 $
    Assets:Checking
";

    let tmp = compile_to_dop_with_compression(source, doppio::Compression::None);
    let journal = load_dop(tmp.path());

    assert_eq!(journal.transactions.len(), 1);
    assert_eq!(journal.transactions[0].description, "Coffee");
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
    let err = doppio::read_dop(&mut f, &path).expect_err("expected an error for bad magic");

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
    let err = doppio::read_dop(&mut f, &path).expect_err("expected an error for empty file");

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

    // Write correct magic, version=99, compression=0 (none), reserved=0.
    tmp.write_all(b"DOP\0").expect("write magic");
    tmp.write_all(&99u16.to_le_bytes()).expect("write version");
    tmp.write_all(&[0u8, 0u8])
        .expect("write compression+reserved");
    tmp.flush().expect("flush");

    let path = tmp.path().to_owned();
    let mut f = std::fs::File::open(&path).expect("open");
    let err = doppio::read_dop(&mut f, &path).expect_err("expected an error for wrong version");

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
