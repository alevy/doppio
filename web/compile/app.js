// doppio · compile — single-page Vue 3 + wasm-bindgen demo.
//
// This module is intentionally framework-light: it imports Vue 3 from a CDN
// (via the importmap in index.html) and the wasm-bindgen output from
// ./pkg/doppio_wasm.js (regenerated locally via crates/doppio-wasm/build-wasm.sh).
//
// See ../README.md for the build / deploy / round-trip story.

import { createApp, ref, computed, onMounted, onBeforeUnmount, h } from "vue";
import initWasm, { compile as wasmCompile } from "./pkg/doppio_wasm.js";

// ── Extension → frontend table ──────────────────────────────────────────────
//
// Mirrors `frontend_for_extension` in crates/doppio/src/lib.rs (the canonical
// table). Keep this in sync if a new extension is added upstream.
const EXTENSION_TO_FRONTEND = {
  ledger: "ledger",
  hledger: "hledger",
  journal: "hledger",
  beancount: "beancount",
};

const SUPPORTED_FRONTENDS = ["ledger", "hledger", "beancount"];

function frontendForFilename(name) {
  if (!name) return null;
  const dot = name.lastIndexOf(".");
  if (dot < 0) return null;
  const ext = name.slice(dot + 1).toLowerCase();
  return EXTENSION_TO_FRONTEND[ext] ?? null;
}

function basenameStem(name) {
  if (!name) return "journal";
  const slash = Math.max(name.lastIndexOf("/"), name.lastIndexOf("\\"));
  const stripped = slash >= 0 ? name.slice(slash + 1) : name;
  const dot = stripped.lastIndexOf(".");
  return dot > 0 ? stripped.slice(0, dot) : stripped;
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

// ── Error shape parsing ─────────────────────────────────────────────────────
//
// The wasm shim formats parse errors as:
//   "parse error (line N, col M): <pest-style multi-line message>"
// We split off the leading prefix so we can render the line/col prominently
// and the body in a <pre>.
function parseErrorMessage(rawMessage) {
  const m = /^parse error \(line (\d+), col (\d+)\): ([\s\S]*)$/.exec(
    rawMessage,
  );
  if (m) {
    return {
      line: Number(m[1]),
      col: Number(m[2]),
      detail: m[3],
    };
  }
  return { line: null, col: null, detail: rawMessage };
}

// ── Bootstrap: await WASM init, then mount Vue ──────────────────────────────

const mountTarget = document.querySelector("#app");

(async () => {
  try {
    await initWasm();
  } catch (e) {
    mountTarget.innerHTML = "";
    const err = document.createElement("div");
    err.style.padding = "2rem";
    err.style.color = "#991b1b";
    err.textContent = `Failed to load the doppio WASM module: ${e.message ?? e}`;
    mountTarget.appendChild(err);
    return;
  }

  createApp(App).mount(mountTarget);
})();

// ── Vue root component ──────────────────────────────────────────────────────

const App = {
  setup() {
    // Reactive state.
    const source = ref("");
    const filename = ref(null);
    const frontend = ref("ledger");
    const userOverrodeFrontend = ref(false);

    const output = ref(null); // Uint8Array | null
    const outputFilename = ref(null);
    const error = ref(null); // { line, col, detail } | { raw } | null
    const compiling = ref(false);

    const isDragOver = ref(false);
    const fileInput = ref(null);

    // Derived: does the pasted source contain an `include` directive?
    // The wasm shim's no-op opener silently returns empty content, so includes
    // won't error, they just won't pull in anything — warn the user.
    const includeWarning = computed(() => {
      return /^\s*include\s/m.test(source.value);
    });

    // ── File selection ──────────────────────────────────────────────────────

    function openFilePicker() {
      fileInput.value?.click();
    }

    function onFileInputChange(event) {
      const file = event.target.files?.[0];
      if (file) loadFile(file);
      // Reset so picking the same file again still triggers change.
      if (fileInput.value) fileInput.value.value = "";
    }

    function loadFile(file) {
      const reader = new FileReader();
      reader.onload = () => {
        source.value = String(reader.result ?? "");
        filename.value = file.name;
        // Auto-infer frontend from extension only if user hasn't overridden.
        if (!userOverrodeFrontend.value) {
          const inferred = frontendForFilename(file.name);
          if (inferred) frontend.value = inferred;
        }
        // Clear any previous compile output so it's clear we have new input.
        output.value = null;
        outputFilename.value = null;
        error.value = null;
      };
      reader.onerror = () => {
        error.value = { detail: `Failed to read ${file.name}: ${reader.error?.message ?? "unknown error"}`, line: null, col: null };
      };
      reader.readAsText(file);
    }

    // ── Drag-and-drop (full-page overlay) ──────────────────────────────────

    function onDragEnter(event) {
      event.preventDefault();
      isDragOver.value = true;
    }
    function onDragOver(event) {
      event.preventDefault();
    }
    function onDragLeave(event) {
      // Only clear when leaving the outermost element.
      if (
        !event.currentTarget.contains(event.relatedTarget)
      ) {
        isDragOver.value = false;
      }
    }
    function onDrop(event) {
      event.preventDefault();
      isDragOver.value = false;
      const file = event.dataTransfer?.files?.[0];
      if (file) loadFile(file);
    }

    // ── Frontend dropdown ──────────────────────────────────────────────────

    function onFrontendChange(event) {
      frontend.value = event.target.value;
      userOverrodeFrontend.value = true;
    }

    // ── Compile ────────────────────────────────────────────────────────────

    function compileNow() {
      if (compiling.value) return;
      compiling.value = true;
      error.value = null;
      output.value = null;
      outputFilename.value = null;

      // Yield to the event loop so the "compiling" state can render before
      // the (synchronous) WASM call blocks the main thread.
      setTimeout(() => {
        try {
          const bytes = wasmCompile(source.value, frontend.value);
          output.value = bytes;
          outputFilename.value = `${basenameStem(filename.value)}.dop`;
        } catch (e) {
          error.value = parseErrorMessage(e.message ?? String(e));
        } finally {
          compiling.value = false;
        }
      }, 0);
    }

    // ── Download ───────────────────────────────────────────────────────────

    function downloadOutput() {
      if (!output.value) return;
      const blob = new Blob([output.value], {
        type: "application/octet-stream",
      });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = outputFilename.value ?? "journal.dop";
      document.body.appendChild(a);
      a.click();
      a.remove();
      // Give the browser a tick to actually start the download before revoking.
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }

    // ── Keyboard shortcut: Cmd/Ctrl+Enter to compile ───────────────────────

    function onKeydown(event) {
      if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
        event.preventDefault();
        if (source.value.trim().length > 0) compileNow();
      }
    }
    onMounted(() => window.addEventListener("keydown", onKeydown));
    onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));

    // Expose to template.
    return {
      source,
      filename,
      frontend,
      output,
      outputFilename,
      error,
      compiling,
      isDragOver,
      fileInput,
      includeWarning,
      supportedFrontends: SUPPORTED_FRONTENDS,
      openFilePicker,
      onFileInputChange,
      onDragEnter,
      onDragOver,
      onDragLeave,
      onDrop,
      onFrontendChange,
      compileNow,
      downloadOutput,
      formatBytes,
    };
  },

  // Template string — Vue's runtime compiler (present in vue.esm-browser.prod.js)
  // compiles this at first render. Keeps the source readable without a build step.
  template: /* html */ `
    <div
      class="app-root"
      @dragenter="onDragEnter"
      @dragover="onDragOver"
      @dragleave="onDragLeave"
      @drop="onDrop"
    >
      <input
        ref="fileInput"
        type="file"
        accept=".ledger,.hledger,.journal,.beancount,text/*"
        class="visually-hidden"
        @change="onFileInputChange"
      />

      <header class="hero">
        <div class="hero-inner">
          <div class="hero-titles">
            <h1>doppio <span class="subtitle">· compile</span></h1>
            <p class="tagline">
              Compile a
              <a href="https://ledger-cli.org/" target="_blank" rel="noopener">ledger</a>,
              <a href="https://hledger.org/" target="_blank" rel="noopener">hledger</a>,
              or
              <a href="https://beancount.github.io/" target="_blank" rel="noopener">beancount</a>
              source journal into a portable
              <code>.dop</code> binary in your browser — doppio runs as WebAssembly,
              entirely client-side.
            </p>
          </div>
          <div class="hero-controls">
            <div class="file-controls">
              <button class="btn btn-secondary" type="button" @click="openFilePicker">
                Open file…
              </button>
              <span v-if="filename" class="source-label" :title="filename">
                {{ filename }}
              </span>
            </div>
          </div>
        </div>
      </header>

      <div v-if="isDragOver" class="drop-overlay" aria-hidden="true">
        <div class="drop-overlay-inner">Drop source file to load</div>
      </div>

      <main class="content">
        <section class="card">
          <div class="editor-toolbar">
            <label>
              Frontend:
              <select :value="frontend" @change="onFrontendChange">
                <option v-for="fe in supportedFrontends" :key="fe" :value="fe">
                  {{ fe }}
                </option>
              </select>
            </label>
            <span class="toolbar-spacer"></span>
            <button
              class="btn btn-primary"
              type="button"
              :disabled="compiling || source.trim().length === 0"
              @click="compileNow"
            >
              {{ compiling ? "Compiling…" : "Compile → .dop" }}
            </button>
          </div>

          <textarea
            class="source"
            v-model="source"
            spellcheck="false"
            placeholder="Paste a ledger / hledger / beancount journal here, drop a file anywhere on the page, or click 'Open file…'.

Example:
2024-01-15 * Groceries
    Expenses:Food      $50.00
    Assets:Checking"
          ></textarea>
        </section>

        <section v-if="includeWarning && !output && !error" class="card panel-notice">
          <strong>Heads up:</strong> this source uses an <code>include</code> directive.
          In v1, doppio-wasm's file opener is a no-op stub — included files are
          silently treated as empty. Inline the included content into the textarea,
          or expect the compile to succeed against only the visible source.
        </section>

        <section v-if="error" class="card panel-error" role="alert">
          <h2>
            Compile failed<span v-if="error.line">
              · line {{ error.line }}, col {{ error.col }}</span>
          </h2>
          <pre>{{ error.detail }}</pre>
        </section>

        <section v-if="output" class="card panel-success">
          <h2>Compiled successfully</h2>
          <div class="output-row">
            <button class="btn btn-primary" type="button" @click="downloadOutput">
              Download {{ outputFilename }}
            </button>
            <span class="output-stats">
              <span class="mono">{{ formatBytes(output.length) }}</span>
              ({{ output.length.toLocaleString() }} bytes)
            </span>
          </div>
          <p class="roundtrip-note">
            Verify the round-trip by opening the downloaded
            <code>.dop</code> in the <a href="../dashboard/">dashboard demo</a>
            (drag-and-drop or use its file picker).
          </p>
        </section>
      </main>

      <footer class="footer">
        <p>
          <a href="https://github.com/alevy/doppio" target="_blank" rel="noopener">
            github.com/alevy/doppio
          </a>
          · <a href="https://github.com/alevy/doppio/issues/299" target="_blank" rel="noopener">
            issue #299
          </a>
          · sibling: <a href="../dashboard/">dashboard demo</a>
        </p>
      </footer>
    </div>
  `,
};
