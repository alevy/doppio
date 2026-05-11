# doppio · compile

Browser demo that compiles a ledger / hledger / beancount source journal into
a downloadable `.dop` binary, entirely client-side. The doppio compiler ships
as a WebAssembly module (`crates/doppio-wasm`) loaded directly by the page —
no server, no build step required to author this demo.

This is the **write-side** companion to the
[`../dashboard/`](../dashboard/) read-side demo, and together they cover both
halves of doppio's "format-as-API reference implementation" story. See
[issue #299](https://github.com/alevy/doppio/issues/299) for the design notes.

## Stack

Deliberately minimal — the point of this app is to demonstrate that embedding
doppio's compiler requires almost no tooling:

- **Plain `index.html`** — no bundler, no `package.json`, no `node_modules`.
- **Vue 3** loaded directly from the unpkg.com CDN as an ES module
  (`vue@3.5.13/dist/vue.esm-browser.prod.js`) via `<script type="importmap">`.
  The runtime template compiler is included, so the root component declares
  its template inline as a string.
- **`doppio-wasm`** loaded from `./pkg/doppio_wasm.js` (a wasm-bindgen-generated
  ES module + WebAssembly binary). The `pkg/` directory is **generated**; see
  the build steps below.

The sibling [`../dashboard/`](../dashboard/) uses a full Vite + TS + SFC
pipeline. That's a deliberate contrast: the dashboard is a serious app, the
compile demo is a single-page "look how easy this is" reference.

## Building locally

1. Build the `doppio-wasm` shim and stage `pkg/` next to the demo:

   ```sh
   bash crates/doppio-wasm/build-wasm.sh
   rm -rf web/compile/pkg
   cp -r crates/doppio-wasm/pkg web/compile/pkg
   ```

2. Serve `web/compile/` over HTTP (the browser refuses to load `.wasm` from
   `file://` URLs). Any static server works; the only requirement is that
   `.wasm` files are served with the `application/wasm` MIME type, which the
   bundled Python server handles correctly:

   ```sh
   python3 -m http.server --directory web/compile 8000
   ```

   Then open `http://localhost:8000/`.

## CI build

The `Web compile demo` job in
[`.github/workflows/web.yml`](../../.github/workflows/web.yml) reproduces the
above steps on every PR that touches `web/compile/**` or
`crates/doppio-wasm/**`. It builds the WASM shim from source, assembles
`web/compile/dist/`, and uploads it as a GitHub Pages artifact.

Wiring up the GitHub Pages **deploy** for the compile demo alongside the
dashboard is deferred — see the workflow's `TODO` for the followup task.

## UX overview

- **Source input.** Textarea for paste-in (primary), file picker (header
  button), and full-page drag-and-drop overlay. The drop overlay is purple
  to distinguish it from the dashboard's blue overlay.
- **Frontend selector.** Dropdown with `ledger | hledger | beancount`.
  Default `ledger`. When a file is picked or dropped, the extension
  (`.ledger`, `.hledger`, `.journal`, `.beancount`) auto-sets the dropdown
  — unless the user has explicitly changed it, in which case their choice
  wins.
- **Compile button.** Manual trigger; `Ctrl/Cmd+Enter` is also bound.
  Disabled while empty or compiling.
- **Output panel.** Shows the byte count and a download button (uses
  `Blob` + `URL.createObjectURL` + `<a download>`, then revokes the URL).
- **Error panel.** Parse errors from doppio include `(line N, col M)`
  annotations when available; those are surfaced in the header, with the
  full pest-style multi-line message in a `<pre>` below.
- **Round-trip note.** After a successful compile, the panel reminds the
  user that they can verify the output by dropping it into the dashboard
  at [`../dashboard/`](../dashboard/).

## Bundle size

The compiler ships as a single `.wasm` file plus a small amount of glue.
Measured at workspace release defaults (`lto = "fat"`, `codegen-units = 1`,
`strip = true`, `opt-level = 3`):

| Asset                       | Raw      | gzip    | brotli  |
|-----------------------------|----------|---------|---------|
| `doppio_wasm_bg.wasm`       | ~2.33 MB | ~639 KB | ~431 KB |
| `doppio_wasm.js` (glue)     |  ~10 KB  |   ~3 KB |   ~3 KB |
| Vue 3 prod ESM              | ~160 KB  |  ~58 KB |  ~50 KB |
| `index.html` + `app.js`     |  ~15 KB  |   ~5 KB |   ~4 KB |
| **Total cold load**         | **~2.50 MB** | **~705 KB** | **~488 KB** |

The WASM size is by far the dominant cost. Size optimisations
(`opt-level = "z"`, eliminating panic infrastructure, splitting the parser
generators) are tracked as a follow-up — the prior probe without bindgen
glue measured ~285 KB brotli, so there is meaningful headroom.

Per [issue #299](https://github.com/alevy/doppio/issues/299)'s acceptance
criteria, this README is the documented bundle-size record; substantial
regressions in future PRs should be flagged here.

## v1 limitations

- **`include` directives are silently no-op.** The wasm shim ships a stub
  opener that returns empty content. If the source contains an `include`,
  the demo shows a small in-page notice; the compile will succeed using
  only the visible source. Plumbing async opener callbacks across the
  JS/WASM boundary is deferred.
- **No configuration knobs.** The compiler runs with each frontend's
  default elaboration settings (tolerance, balance mode, assertion scope).
  A `config` argument is reserved on the shim's `compile` API for a
  future release.
- **Three frontends only.** `ledger`, `hledger`, `beancount`. CSV / Qif /
  other dialects are out of scope here.

## Round-trip story

This demo compiles plain text → `.dop` (write side). The sibling
[`../dashboard/`](../dashboard/) decodes `.dop` → rendered views
(read side), using a **JS-native** protobuf decoder — no WASM dependency.
Together they prove the `.dop` format-as-API claim:

1. Compile your `.ledger` journal here. Download the `.dop`.
2. Drop the `.dop` into the dashboard.
3. The dashboard renders balance / register / charts without ever invoking
   doppio's compiler — only the `.dop` bytes flow between them.
