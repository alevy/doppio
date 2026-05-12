//! wasm-bindgen shim that exposes doppio's compile pipeline to JavaScript.
//!
//! # JS API
//!
//! ```js
//! import init, { compile, compile_multi } from "./doppio_wasm.js";
//!
//! await init();
//!
//! // Single-file (paste) flow:
//! const bytes = compile(source, "ledger"); // Returns Uint8Array (.dop bytes)
//!
//! // Multi-file (upload) flow:
//! const files = { "main.ledger": "...", "sub/accounts.ledger": "..." };
//! const bytes = compile_multi("main.ledger", files, "ledger");
//! ```
//!
//! See the crate README for full documentation.

use std::collections::HashMap;

use wasm_bindgen::prelude::*;

/// Compile plain-text accounting source to a `.dop` binary blob.
///
/// # Parameters
///
/// - `source`: the complete source text of the journal.
/// - `frontend`: `"ledger"`, `"hledger"`, or `"beancount"`.
///
/// # Returns
///
/// A `Uint8Array` containing the full `.dop` file (8-byte header + deflate-compressed
/// protobuf body). Pass it to the browser's `URL.createObjectURL` / a `<a download>`
/// link or hand it to doppio's `readDop` loader.
///
/// # Throws
///
/// Throws a JavaScript `Error` on any failure (unknown frontend, parse error,
/// elaboration error). Parse errors include `line` and `col` annotations in the
/// message when the underlying error carries that information.
///
/// # Known v1 limitations
///
/// `include` directives in the source are silently ignored — the file-opener is a
/// no-op stub. All source text must be passed inline in `source`. This will be
/// addressed in a future release.
#[wasm_bindgen]
pub fn compile(source: &str, frontend: &str) -> Result<Vec<u8>, JsError> {
    // Install the panic hook on first call so panics surface as JS errors
    // instead of "RuntimeError: unreachable" with no stack trace.
    console_error_panic_hook::set_once();

    let fe: Box<dyn doppio::Frontend> = match frontend {
        "ledger" => Box::new(doppio::LedgerFrontend),
        "hledger" => Box::new(doppio::HledgerFrontend),
        "beancount" => Box::new(doppio::BeancountFrontend),
        other => {
            return Err(JsError::new(&format!(
                "unknown frontend {:?}; valid values are \"ledger\", \"hledger\", \"beancount\"",
                other
            )));
        }
    };

    // No-op opener: `include` directives silently return empty content.
    // See crate README for the v1 limitation note.
    let opener: &doppio::frontend::Opener = &|_| Ok(String::new());

    let base = std::path::Path::new("");

    let hir = fe
        .parse(source, base, opener)
        .map_err(|e| format_error(e.as_ref()))?;

    let config = fe.elaboration_defaults();
    let journal = doppio::elaborate(hir, &config).map_err(|e| JsError::new(&e.to_string()))?;

    let mut buf = Vec::new();
    doppio::write_dop(&journal, &mut buf, doppio::Compression::Deflate)
        .map_err(|e| JsError::new(&e.to_string()))?;

    Ok(buf)
}

/// Compile a multi-file journal to a `.dop` binary blob, resolving `include` directives.
///
/// # Parameters
///
/// - `entry_path`: the relative path of the root file (e.g. `"main.ledger"`). Must be present
///   in `files`.
/// - `files`: a JavaScript `Record<string, string>` mapping relative paths to file contents.
///   Keys must match the strings that appear in `include` directives after path joining
///   (i.e. strip any common ancestor prefix before passing). Path normalisation is the
///   caller's responsibility.
/// - `frontend`: `"ledger"`, `"hledger"`, or `"beancount"`.
///
/// # Returns
///
/// A `Uint8Array` containing the full `.dop` file. Same format as [`compile`].
///
/// # Throws
///
/// Throws a JavaScript `Error` if:
/// - `entry_path` is not found in `files`,
/// - `files` cannot be deserialised as `Record<string, string>`,
/// - `frontend` is unrecognised,
/// - the journal fails to parse or elaborate,
/// - an `include` path is not found in `files` (the error message names the missing path).
#[wasm_bindgen]
pub fn compile_multi(entry_path: &str, files: JsValue, frontend: &str) -> Result<Vec<u8>, JsError> {
    console_error_panic_hook::set_once();

    let file_map: HashMap<String, String> = serde_wasm_bindgen::from_value(files)
        .map_err(|e| JsError::new(&format!("failed to deserialise files map: {}", e)))?;

    let entry_source = file_map.get(entry_path).cloned().ok_or_else(|| {
        JsError::new(&format!(
            "entry file {:?} not found in the uploaded file set",
            entry_path
        ))
    })?;

    let fe: Box<dyn doppio::Frontend> = match frontend {
        "ledger" => Box::new(doppio::LedgerFrontend),
        "hledger" => Box::new(doppio::HledgerFrontend),
        "beancount" => Box::new(doppio::BeancountFrontend),
        other => {
            return Err(JsError::new(&format!(
                "unknown frontend {:?}; valid values are \"ledger\", \"hledger\", \"beancount\"",
                other
            )));
        }
    };

    // Build an opener that looks up included paths in the file map.
    // The parser joins the current base_path with the include argument before
    // calling the opener, so the key we receive already has the correct relative
    // path (e.g. "sub/expenses.ledger").
    let opener = move |path: &str| -> Result<String, Box<dyn std::error::Error>> {
        file_map.get(path).cloned().ok_or_else(|| {
            format!("include file {:?} not found in the uploaded file set", path).into()
        })
    };

    // Use the directory portion of entry_path as the base so that the parser
    // correctly joins relative include paths.
    let base = std::path::Path::new(entry_path)
        .parent()
        .unwrap_or(std::path::Path::new(""));

    let hir = fe
        .parse(&entry_source, base, &opener)
        .map_err(|e| format_error(e.as_ref()))?;

    let config = fe.elaboration_defaults();
    let journal = doppio::elaborate(hir, &config).map_err(|e| JsError::new(&e.to_string()))?;

    let mut buf = Vec::new();
    doppio::write_dop(&journal, &mut buf, doppio::Compression::Deflate)
        .map_err(|e| JsError::new(&e.to_string()))?;

    Ok(buf)
}

/// Format a boxed error into a JS-friendly error message.
///
/// When the error message matches the pattern emitted by doppio's pest-based
/// parsers (` --> N:M`) we extract the line/column and prepend them so the
/// caller sees `"parse error (line N, col M): <message>"`.
fn format_error(e: &dyn std::error::Error) -> JsError {
    let msg = e.to_string();

    // pest parse errors include a ` --> line:col` annotation in their Display.
    // Example: `" --> 3:1\n  |\n3 | ..."`
    if let Some((line, col)) = extract_line_col(&msg) {
        JsError::new(&format!("parse error (line {line}, col {col}): {msg}"))
    } else {
        JsError::new(&msg)
    }
}

/// Attempt to extract `(line, col)` from a pest-formatted error string.
///
/// Pest errors render as `" --> LINE:COL\n..."`. We look for the ` --> `
/// marker and parse the numbers that follow.
fn extract_line_col(msg: &str) -> Option<(u32, u32)> {
    // Find the " --> " marker that pest includes in its Display output.
    let marker = " --> ";
    let start = msg.find(marker)? + marker.len();
    let rest = &msg[start..];
    // The marker is followed by "LINE:COL" (then typically '\n' or end).
    let colon = rest.find(':')?;
    let line: u32 = rest[..colon].trim().parse().ok()?;
    let after_colon = &rest[colon + 1..];
    let end = after_colon
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_colon.len());
    let col: u32 = after_colon[..end].trim().parse().ok()?;
    Some((line, col))
}
