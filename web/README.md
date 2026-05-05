# doppio-web

Browser demo for the [doppio](../README.md) compiled-ledger format.

## What this is

A small Vue 3 + Vite + TypeScript app that loads `.dop` files (and, post-1.0, `.ledger` source files via a WASM shim) and renders balance, register, and chart views over them. The runtime path for `.dop` consumption is **JS-native** — the app uses `@bufbuild/protobuf` to decode the wire format directly from the published [`proto/doppio.proto`](../proto/doppio.proto) schema, with no Rust or WASM dependency for the read-only path.

This is the central architectural validator of doppio's format-as-API claim: any non-Rust language consumer can read `.dop` files via the published schema, no special tooling required.

## Stack

- **UI framework:** Vue 3 (Composition API) + Pinia
- **Build tool:** Vite
- **Language:** TypeScript (strict)
- **Protobuf codegen:** [`@bufbuild/protoc-gen-es`](https://github.com/bufbuild/protobuf-es) — generates idiomatic TS interfaces from `../proto/doppio.proto`
- **Decimal:** [`decimal.js`](https://mikemcl.github.io/decimal.js/) (eager conversion at the loader boundary)
- **Charts:** [Chart.js](https://www.chartjs.org/) via `vue-chartjs`
- **Decompression:** [`pako`](https://github.com/nodeca/pako) (deflate, matching the `.dop` body codec)

## Running locally

```sh
npm install
npm run dev
```

The dev server hot-reloads on changes to `src/` and the schema at `../proto/doppio.proto` (regenerated at every `dev`/`build` start).

## Building for production

```sh
npm run build
npm run preview   # serves the dist/ output for local verification
```

The Vite output assumes a base path of `/doppio/` to match the GitHub Pages URL. Dev mode uses `/`.

## Regenerating the sample `.dop`

The committed [`public/sample.dop`](./public/sample.dop) is the compiled output of [`fixtures/sample.ledger`](./fixtures/sample.ledger), a small fictional journal designed to exercise every view in the demo (multi-commodity, FX, lots, balance assertions, ~6 months of activity). The journal is **entirely synthetic** — names, payees, and amounts are not real.

To regenerate after editing the source:

```sh
npm run fixtures
```

This shells out to the workspace's `dop` CLI (`cargo run -p doppio-cli`), so you need a Rust toolchain available. The committed `.dop` exists so JS-only contributors don't need cargo.

## Layout

```
web/
├── fixtures/sample.ledger        # hand-written demo journal (synthetic)
├── public/sample.dop             # compiled artifact, served at /sample.dop
├── src/
│   ├── App.vue                   # skeleton page
│   ├── main.ts                   # Vue + Pinia bootstrap
│   ├── lib/
│   │   ├── dop/                  # .dop reader (#151)
│   │   └── proto/generated/      # buf-generated TS stubs (gitignored)
│   ├── store/                    # Pinia stores
│   └── views/                    # balance / register / chart (#150)
├── buf.gen.yaml                  # protoc-gen-es codegen config
├── buf.yaml                      # buf lint/breaking config
└── vite.config.ts                # Vite + Vue config; base path = /doppio/
```

## Status

This is the **bootstrap** (issue [#148](https://github.com/alevy/doppio/issues/148)). The interactive `.dop` reader and views land in subsequent issues — see the parent [Web GUI milestone](https://github.com/alevy/doppio/milestones).

## License

Same as the parent repository.
