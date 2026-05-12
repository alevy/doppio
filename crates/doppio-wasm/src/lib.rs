//! wasm-bindgen shim that exposes doppio's compile pipeline to JavaScript.
//!
//! # JS API
//!
//! ```js
//! import init, { compile } from "./doppio_wasm.js";
//!
//! await init();
//!
//! // Single-file flow:
//! const bytes = compile(source, "ledger"); // Returns Uint8Array (.dop bytes)
//!
//! // Multi-file flow — resolve `include` directives via a JS callback:
//! const bytes = compile(entrySource, "ledger", {
//!   basePath: "sub",                          // optional; "" by default
//!   opener:   (path) => fileMap.get(path),    // throws or returns null for missing
//! });
//! ```
//!
//! The third argument is an optional `config` dictionary. New options can be
//! added without changing the function signature; callers that pass nothing
//! get the legacy single-file behaviour (`include` directives silently return
//! empty content).
//!
//! See the crate README for full documentation.

use wasm_bindgen::prelude::*;

/// Compile plain-text accounting source to a `.dop` binary blob.
///
/// # Parameters
///
/// - `source`: the complete source text of the entry file.
/// - `frontend`: `"ledger"`, `"hledger"`, or `"beancount"`.
/// - `config` (optional): a dictionary with optional keys:
///   - `basePath`: the directory the parser treats as the entry file's base
///     when resolving `include` paths. Defaults to `""`.
///   - `opener`: a `(path: string) => string` callback invoked for each
///     `include` directive. The callback may `throw` (surfaced as the parse
///     error) or return `null`/`undefined` (treated as "not found"). Without
///     an opener, `include` directives silently return empty content.
///
/// # Returns
///
/// A `Uint8Array` containing the full `.dop` file (8-byte header + deflate-compressed
/// protobuf body).
///
/// # Throws
///
/// A JavaScript `Error` on any failure — unknown frontend, parse error,
/// elaboration error, malformed `config`, or an opener that threw or returned
/// a non-string. Parse errors include `line` and `col` annotations in the
/// message when the underlying error carries that information.
#[wasm_bindgen]
pub fn compile(source: &str, frontend: &str, config: JsValue) -> Result<Vec<u8>, JsError> {
    console_error_panic_hook::set_once();

    let (base_path_str, js_opener) = parse_config(&config)?;

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

    // Without an opener, behaviour matches the legacy two-arg call: `include`
    // directives silently return empty content.
    let opener = move |path: &str| -> Result<String, Box<dyn std::error::Error>> {
        let Some(f) = js_opener.as_ref() else {
            return Ok(String::new());
        };
        let res = f
            .call1(&JsValue::NULL, &JsValue::from_str(path))
            .map_err(|err| format!("opener threw for {:?}: {}", path, js_error_message(&err)))?;
        if res.is_undefined() || res.is_null() {
            return Err(format!(
                "include file {:?} not found (opener returned null/undefined)",
                path
            )
            .into());
        }
        res.as_string()
            .ok_or_else(|| format!("opener returned non-string for include {:?}", path).into())
    };

    let base = std::path::Path::new(&base_path_str);

    let hir = fe
        .parse(source, base, &opener)
        .map_err(|e| format_error(e.as_ref()))?;

    let elaboration_config = fe.elaboration_defaults();
    let journal =
        doppio::elaborate(hir, &elaboration_config).map_err(|e| JsError::new(&e.to_string()))?;

    let mut buf = Vec::new();
    doppio::write_dop(&journal, &mut buf, doppio::Compression::Deflate)
        .map_err(|e| JsError::new(&e.to_string()))?;

    Ok(buf)
}

/// Extract `(basePath, opener)` from the JS `config` argument.
///
/// `config` may be `undefined`, `null`, or an object with optional `basePath`
/// (string) and `opener` (function) keys. Unknown keys are ignored.
fn parse_config(config: &JsValue) -> Result<(String, Option<js_sys::Function>), JsError> {
    if config.is_undefined() || config.is_null() {
        return Ok((String::new(), None));
    }

    let base_path = js_sys::Reflect::get(config, &JsValue::from_str("basePath"))
        .map_err(|_| JsError::new("failed to read config.basePath"))?
        .as_string()
        .unwrap_or_default();

    let opener_val = js_sys::Reflect::get(config, &JsValue::from_str("opener"))
        .map_err(|_| JsError::new("failed to read config.opener"))?;
    let opener = if opener_val.is_undefined() || opener_val.is_null() {
        None
    } else {
        Some(
            opener_val
                .dyn_into::<js_sys::Function>()
                .map_err(|_| JsError::new("config.opener must be a function"))?,
        )
    };

    Ok((base_path, opener))
}

/// Extract a human-readable message from a JS-thrown value.
///
/// Most thrown values are `Error` instances whose `.message` is what users
/// want to see; strings come through as-is; anything else falls back to
/// `Debug`.
fn js_error_message(value: &JsValue) -> String {
    if let Some(s) = value.as_string() {
        return s;
    }
    if let Ok(msg) = js_sys::Reflect::get(value, &JsValue::from_str("message"))
        && let Some(s) = msg.as_string()
    {
        return s;
    }
    format!("{:?}", value)
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
    let marker = " --> ";
    let start = msg.find(marker)? + marker.len();
    let rest = &msg[start..];
    let colon = rest.find(':')?;
    let line: u32 = rest[..colon].trim().parse().ok()?;
    let after_colon = &rest[colon + 1..];
    let end = after_colon
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_colon.len());
    let col: u32 = after_colon[..end].trim().parse().ok()?;
    Some((line, col))
}
