# doppio-wasm

A [wasm-bindgen](https://rustwasm.github.io/wasm-bindgen/) shim that exposes
doppio's compile pipeline to JavaScript. This is the write-side reference
implementation of the `.dop` format for browser and Node.js environments —
given plain-text accounting source (ledger, hledger, or beancount), it returns
a complete `.dop` binary blob that the doppio reader can decode.

Part of [issue #299](https://github.com/alevy/doppio/issues/299) — PR1 of the
browser-based "compile ledger/hledger/beancount → downloadable .dop" demo.
PR2 will ship the browser UI that calls this shim.

## JS API

```js
import init, { compile } from "./pkg/doppio_wasm.js";

await init(); // loads the .wasm module

// Returns Uint8Array — a complete .dop binary blob.
const bytes = compile(source, "ledger");

// Supported frontends: "ledger", "hledger", "beancount"
// Unknown frontend name throws an Error.
// Parse or elaboration failures also throw an Error.
```

The `compile` function signature is:

```ts
function compile(source: string, frontend: string): Uint8Array;
```

A third `config` parameter is reserved for future use and will be added as an
optional argument when the configuration API stabilises.

### Error handling

Errors thrown by `compile` are plain JavaScript `Error` instances with a
`.message` property. Parse errors include `(line N, col M)` annotations in the
message when the underlying error provides that information.

```js
try {
  const bytes = compile(badSource, "ledger");
} catch (e) {
  console.error(e.message); // e.g. "parse error (line 3, col 1): ..."
}
```

## Building

### Prerequisites

- Rust toolchain with the `wasm32-unknown-unknown` target:
  ```
  rustup target add wasm32-unknown-unknown
  ```
- `wasm-bindgen-cli` version **0.2.117** (must match the Cargo dep exactly):
  ```
  # via nix:
  nix shell nixpkgs#wasm-bindgen-cli

  # or via cargo:
  cargo install wasm-bindgen-cli --version 0.2.117
  ```

### Build command

```bash
bash crates/doppio-wasm/build-wasm.sh
```

This produces two output directories under `crates/doppio-wasm/`:

| Directory  | Target  | Use case |
|------------|---------|----------|
| `pkg/`     | `web`   | Browser apps (PR2 / static site) — uses `import.meta.url` |
| `pkg-node/`| `nodejs`| Node.js tooling and smoke tests — uses `require` |

Both directories contain:
- `doppio_wasm.js` — JS glue module
- `doppio_wasm_bg.wasm` — the compiled WebAssembly binary
- `doppio_wasm.d.ts` — TypeScript declarations

Both `pkg/` and `pkg-node/` are listed in `.gitignore` — regenerate them
locally with `build-wasm.sh`.

### Bundle size (workspace release defaults, opt-level=3)

Measured with `--release` and workspace `lto=fat`, `codegen-units=1`, `strip=true`:

| Encoding | Size |
|----------|------|
| Raw      | ~2.33 MB |
| gzip     | ~653 KB  |
| brotli   | ~431 KB  |

Size optimisations (`opt-level = "z"`) are available as a follow-up; the prior
probe (without bindgen glue) measured ~285 KB brotli. See [issue #299
comment](https://github.com/alevy/doppio/issues/299) for the full size table.

## Smoke test

```bash
bash crates/doppio-wasm/test-smoke.sh
```

This builds the `pkg-node/` binding (if not present) then runs
`tests/smoke.mjs` against Node.js. The test checks:
- A valid ledger journal compiles to a non-empty `Uint8Array` with the `.dop`
  magic header (`DOP\0` + version 3).
- Unknown frontend names throw.
- Syntactically invalid source throws a parse error.

## Known v1 limitations

- **`include` directives are silently ignored.** The file-opener is a no-op
  stub — all source text must be passed inline in the `source` argument.
  Plumbing async JS opener callbacks is deferred to a future release.

- **No `config` parameter yet.** The `compile` function currently uses each
  frontend's default elaboration settings (tolerance, balance mode, assertion
  scope). An optional third `config` parameter will be added in a future
  release without breaking the existing two-argument call sites.
