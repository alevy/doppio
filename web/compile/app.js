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

// Extensions that identify a file as a possible journal entry point.
const JOURNAL_EXTENSIONS = new Set(["ledger", "hledger", "journal", "beancount"]);

function frontendForFilename(name) {
  if (!name) return null;
  const dot = name.lastIndexOf(".");
  if (dot < 0) return null;
  const ext = name.slice(dot + 1).toLowerCase();
  return EXTENSION_TO_FRONTEND[ext] ?? null;
}

function isJournalFile(name) {
  if (!name) return false;
  const dot = name.lastIndexOf(".");
  if (dot < 0) return false;
  return JOURNAL_EXTENSIONS.has(name.slice(dot + 1).toLowerCase());
}

function basenameStem(name) {
  if (!name) return "journal";
  const slash = Math.max(name.lastIndexOf("/"), name.lastIndexOf("\\"));
  const stripped = slash >= 0 ? name.slice(slash + 1) : name;
  const dot = stripped.lastIndexOf(".");
  return dot > 0 ? stripped.slice(0, dot) : stripped;
}

function directoryOf(path) {
  const i = path.lastIndexOf("/");
  return i >= 0 ? path.slice(0, i) : "";
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

// ── Common-prefix stripping ─────────────────────────────────────────────────
//
// Given a list of paths (e.g. ["dir/a.ledger", "dir/sub/b.ledger"]), return
// the longest common directory prefix that can be stripped. This lets the user
// drag-drop a whole directory and have include paths work as written in the
// source (relative to the top of the uploaded set).
//
// Example: ["myjournal/main.ledger", "myjournal/sub/exp.ledger"]
//   → prefix "myjournal/"
//   → keys become ["main.ledger", "sub/exp.ledger"]
function commonDirectoryPrefix(paths) {
  if (paths.length === 0) return "";
  // Split each path into directory components (everything up to and including
  // the last "/"). We only strip full directory components, not partial names.
  const parts = paths.map((p) => {
    const lastSlash = p.lastIndexOf("/");
    return lastSlash >= 0 ? p.slice(0, lastSlash + 1) : "";
  });
  const first = parts[0];
  let prefix = first;
  for (const p of parts.slice(1)) {
    // Shorten prefix to the longest shared directory prefix.
    while (!p.startsWith(prefix)) {
      const prev = prefix.lastIndexOf("/", prefix.length - 2);
      prefix = prev >= 0 ? prefix.slice(0, prev + 1) : "";
    }
  }
  return prefix;
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
    // ── Input mode ─────────────────────────────────────────────────────────
    //
    // "paste"  — user typed/pasted source into the textarea; calls wasmCompile.
    // "upload" — user uploaded one or more files; calls wasmCompileMulti.
    const inputMode = ref("paste"); // "paste" | "upload"

    // ── Paste-mode state ───────────────────────────────────────────────────
    const source = ref("");
    const filename = ref(null);
    const frontend = ref("ledger");
    const userOverrodeFrontend = ref(false);

    // ── Upload-mode state ──────────────────────────────────────────────────
    //
    // fileMap: Record<relativePath, contents> — the full set of uploaded files
    //   with their common ancestor prefix stripped.
    // entryPath: the relative path selected as the root file to compile.
    // uploadedPaths: ordered list of all keys in fileMap (for the entry picker).
    const fileMap = ref({}); // { relPath: contents }
    const entryPath = ref(null); // selected root path
    const uploadedPaths = ref([]); // sorted list of keys in fileMap

    const output = ref(null); // Uint8Array | null
    const outputFilename = ref(null);
    const error = ref(null); // { line, col, detail } | null
    const compiling = ref(false);

    const isDragOver = ref(false);
    const fileInput = ref(null);
    const multiFileInput = ref(null);

    // Derived: does the paste-mode source contain an `include` directive?
    // In paste mode the no-op opener still applies, so warn the user.
    const includeWarning = computed(
      () => inputMode.value === "paste" && /^\s*include\s/m.test(source.value),
    );

    // Derived: frontend inferred from the selected entry path (upload mode).
    const uploadFrontend = computed(() => {
      if (entryPath.value) return frontendForFilename(entryPath.value) ?? "ledger";
      return "ledger";
    });

    // Derived: whether the upload set has more than one candidate entry file
    // (top-level journal files). When there is exactly one, we auto-pick it.
    const uploadEntryOptions = computed(() =>
      uploadedPaths.value.filter(isJournalFile),
    );

    // ── File selection (single — paste mode) ───────────────────────────────

    function openFilePicker() {
      fileInput.value?.click();
    }

    function onFileInputChange(event) {
      const file = event.target.files?.[0];
      if (file) loadFile(file);
      if (fileInput.value) fileInput.value.value = "";
    }

    function loadFile(file) {
      const reader = new FileReader();
      reader.onload = () => {
        source.value = String(reader.result ?? "");
        filename.value = file.name;
        inputMode.value = "paste";
        if (!userOverrodeFrontend.value) {
          const inferred = frontendForFilename(file.name);
          if (inferred) frontend.value = inferred;
        }
        output.value = null;
        outputFilename.value = null;
        error.value = null;
      };
      reader.onerror = () => {
        error.value = {
          detail: `Failed to read ${file.name}: ${reader.error?.message ?? "unknown error"}`,
          line: null,
          col: null,
        };
      };
      reader.readAsText(file);
    }

    // ── Multi-file selection (upload mode) ─────────────────────────────────

    function openMultiFilePicker() {
      multiFileInput.value?.click();
    }

    // Read all files from a FileList/array into fileMap, stripping the common
    // ancestor prefix from their webkitRelativePath (or falling back to name).
    async function loadFileSet(files) {
      if (!files || files.length === 0) return;

      // Collect raw paths. Prefer webkitRelativePath (set when a directory was
      // selected via the directory picker), falling back to the plain name.
      const rawPaths = Array.from(files).map(
        (f) => f.webkitRelativePath || f.name,
      );

      const prefix = commonDirectoryPrefix(rawPaths);

      // Read each file as text in parallel.
      const entries = await Promise.all(
        Array.from(files).map(
          (f, i) =>
            new Promise((resolve, reject) => {
              const reader = new FileReader();
              reader.onload = () =>
                resolve([rawPaths[i].slice(prefix.length), String(reader.result ?? "")]);
              reader.onerror = () =>
                reject(new Error(`Failed to read ${f.name}: ${reader.error?.message}`));
              reader.readAsText(f);
            }),
        ),
      );

      const map = Object.fromEntries(entries);
      const paths = entries.map(([p]) => p).sort();

      fileMap.value = map;
      uploadedPaths.value = paths;
      inputMode.value = "upload";
      output.value = null;
      outputFilename.value = null;
      error.value = null;

      // Auto-select entry: if exactly one top-level journal file exists, pick it.
      const candidates = paths.filter(isJournalFile);
      entryPath.value = candidates.length === 1 ? candidates[0] : (candidates[0] ?? paths[0] ?? null);
    }

    async function onMultiFileInputChange(event) {
      await loadFileSet(event.target.files);
      if (multiFileInput.value) multiFileInput.value.value = "";
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
      if (!event.currentTarget.contains(event.relatedTarget)) {
        isDragOver.value = false;
      }
    }
    async function onDrop(event) {
      event.preventDefault();
      isDragOver.value = false;

      const items = event.dataTransfer?.items;
      const files = event.dataTransfer?.files;

      // Try to walk directory entries via webkitGetAsEntry() for folder drops.
      if (items && items.length > 0 && typeof items[0].webkitGetAsEntry === "function") {
        const collectedFiles = [];
        await Promise.all(
          Array.from(items).map((item) => {
            const entry = item.webkitGetAsEntry();
            if (!entry) return Promise.resolve();
            return collectEntryFiles(entry, collectedFiles);
          }),
        );
        if (collectedFiles.length > 0) {
          await loadFileSet(collectedFiles);
          return;
        }
      }

      // Fallback: load a single file in paste mode.
      if (files && files.length > 0) {
        if (files.length === 1) {
          loadFile(files[0]);
        } else {
          await loadFileSet(files);
        }
      }
    }

    // Recursively collect File objects from a FileSystemEntry tree.
    function collectEntryFiles(entry, out) {
      if (entry.isFile) {
        return new Promise((resolve) =>
          entry.file(
            (f) => {
              // Attach a synthetic webkitRelativePath using the entry's full path
              // (which already includes the directory name).
              Object.defineProperty(f, "webkitRelativePath", {
                value: entry.fullPath.replace(/^\//, ""),
                writable: false,
              });
              out.push(f);
              resolve();
            },
            () => resolve(),
          ),
        );
      }
      if (entry.isDirectory) {
        return new Promise((resolve) => {
          const reader = entry.createReader();
          function readAll(accum) {
            reader.readEntries(async (batch) => {
              if (batch.length === 0) {
                await Promise.all(accum.map((e) => collectEntryFiles(e, out)));
                resolve();
              } else {
                readAll(accum.concat(batch));
              }
            });
          }
          readAll([]);
        });
      }
      return Promise.resolve();
    }

    // ── Frontend dropdown (paste mode) ─────────────────────────────────────

    function onFrontendChange(event) {
      frontend.value = event.target.value;
      userOverrodeFrontend.value = true;
    }

    // ── Entry-file picker (upload mode) ────────────────────────────────────

    function onEntryPathChange(event) {
      entryPath.value = event.target.value;
      output.value = null;
      outputFilename.value = null;
      error.value = null;
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
          let bytes;
          if (inputMode.value === "upload") {
            const entry = entryPath.value;
            const entrySource = fileMap.value[entry];
            if (entrySource === undefined) {
              throw new Error(`entry file "${entry}" not found in the uploaded file set`);
            }
            bytes = wasmCompile(entrySource, uploadFrontend.value, {
              basePath: directoryOf(entry),
              opener: (path) => {
                const content = fileMap.value[path];
                if (content === undefined) {
                  throw new Error(`include file "${path}" not found in the uploaded file set`);
                }
                return content;
              },
            });
            outputFilename.value = `${basenameStem(entry)}.dop`;
          } else {
            bytes = wasmCompile(source.value, frontend.value);
            outputFilename.value = `${basenameStem(filename.value)}.dop`;
          }
          output.value = bytes;
        } catch (e) {
          error.value = parseErrorMessage(e.message ?? String(e));
        } finally {
          compiling.value = false;
        }
      }, 0);
    }

    // Derived: whether compile button should be enabled.
    const canCompile = computed(() => {
      if (compiling.value) return false;
      if (inputMode.value === "upload") return !!entryPath.value;
      return source.value.trim().length > 0;
    });

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
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }

    // ── Keyboard shortcut: Cmd/Ctrl+Enter to compile ───────────────────────

    function onKeydown(event) {
      if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
        event.preventDefault();
        if (canCompile.value) compileNow();
      }
    }
    onMounted(() => window.addEventListener("keydown", onKeydown));
    onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));

    // Expose to template.
    return {
      inputMode,
      source,
      filename,
      frontend,
      fileMap,
      entryPath,
      uploadedPaths,
      uploadFrontend,
      uploadEntryOptions,
      output,
      outputFilename,
      error,
      compiling,
      isDragOver,
      fileInput,
      multiFileInput,
      includeWarning,
      canCompile,
      supportedFrontends: SUPPORTED_FRONTENDS,
      openFilePicker,
      onFileInputChange,
      openMultiFilePicker,
      onMultiFileInputChange,
      onDragEnter,
      onDragOver,
      onDragLeave,
      onDrop,
      onFrontendChange,
      onEntryPathChange,
      compileNow,
      downloadOutput,
      formatBytes,
      isJournalFile,
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
      <!-- Hidden file inputs -->
      <input
        ref="fileInput"
        type="file"
        accept=".ledger,.hledger,.journal,.beancount,text/*"
        class="visually-hidden"
        @change="onFileInputChange"
      />
      <input
        ref="multiFileInput"
        type="file"
        multiple
        accept=".ledger,.hledger,.journal,.beancount,text/*"
        class="visually-hidden"
        @change="onMultiFileInputChange"
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
              <button class="btn btn-secondary" type="button" @click="openMultiFilePicker">
                Upload files…
              </button>
              <span v-if="inputMode === 'paste' && filename" class="source-label" :title="filename">
                {{ filename }}
              </span>
            </div>
          </div>
        </div>
      </header>

      <div v-if="isDragOver" class="drop-overlay" aria-hidden="true">
        <div class="drop-overlay-inner">Drop file(s) or a folder to load</div>
      </div>

      <main class="content">

        <!-- ── Upload mode ─────────────────────────────────────────────── -->
        <section v-if="inputMode === 'upload'" class="card">
          <div class="editor-toolbar">
            <span class="upload-summary">
              <strong>{{ uploadedPaths.length }}</strong>
              file{{ uploadedPaths.length !== 1 ? 's' : '' }} uploaded
            </span>
            <label v-if="uploadEntryOptions.length > 1">
              Entry file:
              <select :value="entryPath" @change="onEntryPathChange">
                <option v-for="p in uploadEntryOptions" :key="p" :value="p">{{ p }}</option>
              </select>
            </label>
            <span v-else-if="entryPath" class="source-label entry-label">
              Entry: <code>{{ entryPath }}</code>
            </span>
            <span class="toolbar-spacer"></span>
            <span class="frontend-badge">{{ uploadFrontend }}</span>
            <button
              class="btn btn-primary"
              type="button"
              :disabled="!canCompile"
              @click="compileNow"
            >
              {{ compiling ? "Compiling…" : "Compile → .dop" }}
            </button>
          </div>
          <ul class="file-list">
            <li
              v-for="p in uploadedPaths"
              :key="p"
              :class="{ 'file-list-entry': true, 'file-list-entry--active': p === entryPath }"
            >
              <span class="file-list-icon">{{ p === entryPath ? '▶' : ' ' }}</span>
              <code>{{ p }}</code>
            </li>
          </ul>
          <p class="upload-hint">
            Drag-and-drop a folder or click <em>Upload files…</em> to change the file set.
            <code>include</code> paths are resolved against the uploaded set.
          </p>
        </section>

        <!-- ── Paste mode ──────────────────────────────────────────────── -->
        <section v-else class="card">
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
              :disabled="!canCompile"
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
          In single-file / paste mode, included files are silently treated as empty.
          Use <em>Upload files…</em> (or drag-and-drop a folder) to compile journals
          that span multiple files.
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
          · <a href="https://github.com/alevy/doppio/issues/311" target="_blank" rel="noopener">
            issue #311
          </a>
          · sibling: <a href="../dashboard/">dashboard demo</a>
        </p>
      </footer>
    </div>
  `,
};
