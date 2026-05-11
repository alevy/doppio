/**
 * Smoke test for the doppio-wasm Node.js binding.
 *
 * Verifies that:
 *   1. A valid ledger journal compiles to a non-empty Uint8Array that starts
 *      with the .dop magic header bytes.
 *   2. An unknown frontend name throws an error.
 *   3. A syntactically invalid journal throws a parse error.
 *
 * Run via: bash crates/doppio-wasm/test-smoke.sh
 */

import { createRequire } from "module";
import { fileURLToPath } from "url";
import path from "path";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// The nodejs CJS binding lives in pkg-node/ relative to the crate root.
const require = createRequire(import.meta.url);
const { compile } = require(path.join(__dirname, "../pkg-node/doppio_wasm.js"));

// --- Helpers ----------------------------------------------------------------

function assert(condition, message) {
  if (!condition) {
    throw new Error(`Assertion failed: ${message}`);
  }
}

function assertThrows(fn, msgSubstring) {
  try {
    fn();
    throw new Error(
      `Expected an error containing "${msgSubstring}" but no error was thrown`,
    );
  } catch (e) {
    if (e.message.includes("Expected an error containing")) throw e; // re-throw assertion errors
    if (msgSubstring && !e.message.includes(msgSubstring)) {
      throw new Error(
        `Expected error message containing "${msgSubstring}", got: "${e.message}"`,
      );
    }
  }
}

// --- .dop magic bytes -------------------------------------------------------
//
// From doppio/src/lib.rs:
//   const DOP_MAGIC: [u8; 4] = *b"DOP\0";
//   const DOP_FORMAT_VERSION: u16 = 3;
//
// Header layout (8 bytes):
//   bytes 0..4  -- "DOP\0"
//   bytes 4..6  -- version LE u16 (3)
//   byte  6     -- compression (1 = deflate)
//   byte  7     -- reserved (0)
const MAGIC = [0x44, 0x4f, 0x50, 0x00]; // "DOP\0"
const DOP_VERSION = 3;

function checkDopHeader(bytes) {
  assert(bytes instanceof Uint8Array, "result should be Uint8Array");
  assert(bytes.length > 8, "result should be longer than 8 bytes");

  for (let i = 0; i < MAGIC.length; i++) {
    assert(bytes[i] === MAGIC[i], `magic byte ${i}: expected ${MAGIC[i]}, got ${bytes[i]}`);
  }

  // Version is little-endian u16 at bytes 4..6.
  const version = bytes[4] | (bytes[5] << 8);
  assert(version === DOP_VERSION, `expected format version ${DOP_VERSION}, got ${version}`);
}

// --- Tests ------------------------------------------------------------------

console.log("Test 1: valid ledger journal compiles to .dop bytes");
{
  const src = `\
2024-01-15 Groceries
    Expenses:Food    $50
    Assets:Checking
`;
  const result = compile(src, "ledger");
  checkDopHeader(result);
  console.log(`  OK — output ${result.length} bytes`);
}

console.log("Test 2: valid hledger journal compiles to .dop bytes");
{
  const src = `\
2024-01-15 Groceries
    Expenses:Food    $50
    Assets:Checking
`;
  const result = compile(src, "hledger");
  checkDopHeader(result);
  console.log(`  OK — output ${result.length} bytes`);
}

console.log("Test 3: unknown frontend throws");
{
  assertThrows(() => compile("anything", "csv"), "unknown frontend");
  console.log("  OK");
}

console.log("Test 4: parse error throws");
{
  const bad = "this is not valid ledger syntax @@@@\n";
  assertThrows(() => compile(bad, "ledger"), "");
  console.log("  OK — parse error propagated");
}

console.log("Test 5: empty source compiles (empty journal)");
{
  const result = compile("", "ledger");
  checkDopHeader(result);
  console.log(`  OK — output ${result.length} bytes`);
}

console.log("\nAll smoke tests passed.");
