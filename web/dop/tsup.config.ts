import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts"],
  format: ["esm"],
  dts: true,
  clean: true,
  sourcemap: false,
  // Exclude generated proto files from the bundle — they are compiled in.
  // All runtime deps (decimal.js, pako, @bufbuild/protobuf) are kept external
  // so bundler-based consumers can tree-shake them and CDN consumers can
  // import them separately if desired.
  external: ["decimal.js", "pako", "@bufbuild/protobuf"],
});
