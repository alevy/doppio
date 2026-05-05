import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// gh-pages serves this app from https://alevy.github.io/doppio/.
// Local dev (npm run dev) uses base "/" automatically.
export default defineConfig(({ command }) => ({
  plugins: [vue()],
  base: command === "build" ? "/doppio/" : "/",
}));
