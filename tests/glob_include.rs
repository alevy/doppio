//! Integration tests for glob pattern support in `include` directives.
//!
//! These tests exercise [`doppio::file_opener`] and the include-handling path
//! of [`doppio::parser::Parser`] against real files on disk, covering:
//!
//! - single-file (literal path) includes — unchanged behaviour
//! - glob patterns matching multiple files — lexicographic ordering
//! - recursive glob (`**`) — deep directory traversal
//! - glob with no matches — must produce an error

use std::io::Write as _;

use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Create a file at `dir/name` with `content` and return its path.
fn write_file(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    // Support sub-directories in `name` (e.g. "sub/a.ledger").
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    let mut f = std::fs::File::create(&path).expect("create file");
    f.write_all(content.as_bytes()).expect("write");
    path
}

/// Parse and compile a ledger string that uses `include` directives, with
/// `base_path` set to `dir`. Returns the elaborated journal.
fn compile_with_dir(
    source: &str,
    dir: &std::path::Path,
) -> Result<doppio::Journal, Box<dyn std::error::Error>> {
    let parser = doppio::parser::Parser {
        opener: doppio::file_opener,
        base_path: dir.to_path_buf(),
    };
    let mut input = source.to_owned();
    doppio::compile(&mut input, parser)
}

// ── single-file include ───────────────────────────────────────────────────────

/// A literal path `include` should work exactly as before: one file, one entry.
#[test]
fn single_file_include() {
    let dir = TempDir::new().unwrap();

    write_file(
        dir.path(),
        "accounts.ledger",
        "2024-01-01 Single file
    Expenses:Food  10 $
    Assets:Checking
",
    );

    let source = "include accounts.ledger\n";
    let journal = compile_with_dir(source, dir.path()).expect("compile");
    assert_eq!(journal.transactions.len(), 1);
    assert_eq!(journal.transactions[0].description, "Single file");
}

// ── glob: multiple matches, lexicographic order ───────────────────────────────

/// `include *.ledger` should include all matching files in sorted order so the
/// transaction sequence is deterministic regardless of filesystem ordering.
#[test]
fn glob_multiple_files_lexicographic_order() {
    let dir = TempDir::new().unwrap();

    // Write files in reverse alphabetical order to detect any "first-found" bugs.
    write_file(
        dir.path(),
        "c.ledger",
        "2024-03-01 Charlie
    Expenses:Food  30 $
    Assets:Checking
",
    );
    write_file(
        dir.path(),
        "a.ledger",
        "2024-01-01 Alpha
    Expenses:Food  10 $
    Assets:Checking
",
    );
    write_file(
        dir.path(),
        "b.ledger",
        "2024-02-01 Bravo
    Expenses:Food  20 $
    Assets:Checking
",
    );

    let source = "include *.ledger\n";
    let journal = compile_with_dir(source, dir.path()).expect("compile");

    // Files are sorted a.ledger → b.ledger → c.ledger, so transactions appear
    // in that order.
    assert_eq!(journal.transactions.len(), 3);
    assert_eq!(journal.transactions[0].description, "Alpha");
    assert_eq!(journal.transactions[1].description, "Bravo");
    assert_eq!(journal.transactions[2].description, "Charlie");
}

// ── glob: no matches is an error ─────────────────────────────────────────────

/// A glob pattern that matches no files must return an error containing the
/// pattern text, not silently include nothing.
#[test]
fn glob_no_matches_is_error() {
    let dir = TempDir::new().unwrap();
    // No files in the directory — the glob will match nothing.

    let source = "include *.ledger\n";
    let result = compile_with_dir(source, dir.path());

    assert!(
        result.is_err(),
        "glob with no matches should produce an error"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("matched no files") || msg.contains("*.ledger"),
        "error message should mention the unmatched glob; got: {msg}"
    );
}

// ── literal path not found is an error ───────────────────────────────────────

/// A literal `include` path that does not exist must return an error, not
/// silently include nothing.
#[test]
fn literal_path_not_found_is_error() {
    let dir = TempDir::new().unwrap();
    // No file named "missing.ledger" exists.

    let source = "include missing.ledger\n";
    let result = compile_with_dir(source, dir.path());

    assert!(
        result.is_err(),
        "include of a non-existent literal path should produce an error"
    );
}

// ── recursive glob (**) ───────────────────────────────────────────────────────

/// `include people/**/*.ledger` should recurse into subdirectories and include
/// all matching files in lexicographic (path-sorted) order.
#[test]
fn recursive_glob_includes_subdirectories() {
    let dir = TempDir::new().unwrap();

    write_file(
        dir.path(),
        "people/alice.ledger",
        "2024-01-01 Alice salary
    Income:Salary  -1000 $
    Assets:Checking
",
    );
    write_file(
        dir.path(),
        "people/bob.ledger",
        "2024-01-02 Bob salary
    Income:Salary  -2000 $
    Assets:Checking
",
    );
    write_file(
        dir.path(),
        "people/contractors/carol.ledger",
        "2024-01-03 Carol contract
    Income:Consulting  -500 $
    Assets:Checking
",
    );

    let source = "include people/**/*.ledger\n";
    let journal = compile_with_dir(source, dir.path()).expect("compile");

    // Paths sorted: people/alice.ledger, people/bob.ledger, people/contractors/carol.ledger
    assert_eq!(
        journal.transactions.len(),
        3,
        "expected 3 transactions from recursive glob"
    );
    assert_eq!(journal.transactions[0].description, "Alice salary");
    assert_eq!(journal.transactions[1].description, "Bob salary");
    assert_eq!(journal.transactions[2].description, "Carol contract");
}
