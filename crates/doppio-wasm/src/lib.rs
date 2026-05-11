//! wasm-bindgen shim that exposes doppio's compile pipeline to JavaScript.
//!
//! # JS API
//!
//! ```js
//! import init, { compile } from "./doppio_wasm.js";
//!
//! await init();
//!
//! const bytes = compile(source, "ledger"); // Returns Uint8Array (.dop bytes)
//! ```
//!
//! See the crate README for full documentation.

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
