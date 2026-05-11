<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import { useJournalStore } from "@/store/journal";
import FilterBar from "@/components/FilterBar.vue";
import DashboardView from "@/components/DashboardView.vue";
import { compareLocalDate, localDateToString } from "@/lib/dop";

const journals = useJournalStore();
const { journal, error, loading, sourceLabel } = storeToRefs(journals);

const summary = computed(() => {
  const j = journal.value;
  if (!j || j.transactions.length === 0) return null;
  const dates = j.transactions.map((t) => t.date);
  const sorted = [...dates].sort(compareLocalDate);
  return {
    transactions: j.transactions.length,
    accounts: Object.keys(j.accounts).length,
    last: localDateToString(sorted[sorted.length - 1]!),
    first: localDateToString(sorted[0]!),
  };
});

onMounted(() => {
  const url = `${import.meta.env.BASE_URL}sample.dop`;
  journals.loadFromUrl(url);
});

// ── File picker ─────────────────────────────────────────────────────────────

const fileInput = ref<HTMLInputElement | null>(null);

function openFilePicker() {
  fileInput.value?.click();
}

function onFileInputChange(event: Event) {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (file) loadFile(file);
  // Reset so re-selecting the same file still triggers change.
  if (fileInput.value) fileInput.value.value = "";
}

// ── Drag-and-drop ────────────────────────────────────────────────────────────

const isDragOver = ref(false);

function onDragEnter(event: DragEvent) {
  event.preventDefault();
  isDragOver.value = true;
}

function onDragOver(event: DragEvent) {
  event.preventDefault();
}

function onDragLeave(event: DragEvent) {
  // Only clear when leaving the outermost element (relatedTarget is outside).
  if (!(event.currentTarget as HTMLElement).contains(event.relatedTarget as Node | null)) {
    isDragOver.value = false;
  }
}

function onDrop(event: DragEvent) {
  event.preventDefault();
  isDragOver.value = false;
  const file = event.dataTransfer?.files[0];
  if (file) loadFile(file);
}

// ── Shared file loader ───────────────────────────────────────────────────────

function loadFile(file: File) {
  const reader = new FileReader();
  reader.onload = () => {
    if (reader.result instanceof ArrayBuffer) {
      journals.loadFromBuffer(reader.result, file.name);
    }
  };
  reader.readAsArrayBuffer(file);
}
</script>

<template>
  <!-- Full-page drag-and-drop wrapper -->
  <div
    class="app-root"
    :class="{ 'drag-active': isDragOver }"
    @dragenter="onDragEnter"
    @dragover="onDragOver"
    @dragleave="onDragLeave"
    @drop="onDrop"
  >
    <!-- Hidden native file input -->
    <input
      ref="fileInput"
      type="file"
      accept=".dop"
      class="visually-hidden"
      @change="onFileInputChange"
    />

    <header class="hero">
      <div class="hero-inner">
        <div class="hero-titles">
          <h1>doppio</h1>
          <p class="tagline">
            A typed compiler pipeline for
            <a href="https://ledger-cli.org/" target="_blank" rel="noopener">Ledger</a>
            plain-text accounting. This page reads
            <a
              href="https://github.com/alevy/doppio/blob/main/web/dashboard/fixtures/sample.ledger"
              target="_blank"
              rel="noopener"
            >a fictional sample journal</a>
            via a JS-native protobuf decoder -- no Rust or WASM at runtime.
          </p>
        </div>
        <div class="hero-actions">
          <div v-if="summary" class="hero-meta">
            <span><strong>{{ summary.transactions }}</strong> transactions</span>
            <span><strong>{{ summary.accounts }}</strong> accounts</span>
            <span class="range">{{ summary.first }} → {{ summary.last }}</span>
          </div>
          <div class="file-controls">
            <button class="btn-open" @click="openFilePicker">Open .dop file</button>
            <span v-if="sourceLabel" class="source-label" :title="sourceLabel">
              {{ sourceLabel }}
            </span>
          </div>
        </div>
      </div>
    </header>

    <!-- Drag-over overlay -->
    <div v-if="isDragOver" class="drop-overlay" aria-hidden="true">
      <div class="drop-overlay-inner">Drop .dop file to load</div>
    </div>

    <main class="content">
      <section v-if="error" class="card error">
        <h2>Failed to load <code>{{ sourceLabel ?? "file" }}</code></h2>
        <pre>{{ error }}</pre>
      </section>

      <section v-else-if="loading || !journal" class="card placeholder">
        <h2>Loading…</h2>
      </section>

      <template v-else>
        <FilterBar />
        <DashboardView />
      </template>
    </main>

    <footer class="footer">
      <p>
        Source:
        <a href="https://github.com/alevy/doppio" target="_blank" rel="noopener">
          github.com/alevy/doppio
        </a>
        ·
        Schema:
        <a
          href="https://github.com/alevy/doppio/blob/main/proto/doppio.proto"
          target="_blank"
          rel="noopener"
        >
          proto/doppio.proto
        </a>
      </p>
    </footer>
  </div>
</template>

<style scoped>
/* ── Page wrapper ─────────────────────────────────────────────────────────── */

.app-root {
  display: flex;
  flex-direction: column;
  min-height: 100vh;
  position: relative;
}

/* ── Drag-and-drop overlay ───────────────────────────────────────────────── */

.drop-overlay {
  position: fixed;
  inset: 0;
  z-index: 100;
  background: rgba(59, 130, 246, 0.12);
  border: 3px dashed #3b82f6;
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: none;
}

.drop-overlay-inner {
  background: #fff;
  border: 2px solid #3b82f6;
  border-radius: 0.75rem;
  padding: 1.25rem 2.5rem;
  font-size: 1.25rem;
  font-weight: 600;
  color: #1d4ed8;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.12);
}

/* ── Hero ─────────────────────────────────────────────────────────────────── */

.hero {
  padding: 2rem 1.5rem 1.25rem;
  border-bottom: 1px solid #e5e5e5;
  background: #fff;
}

.hero-inner {
  max-width: 64rem;
  margin: 0 auto;
  display: flex;
  flex-wrap: wrap;
  align-items: flex-end;
  gap: 1rem;
}

.hero-titles {
  flex: 1;
  min-width: 18rem;
}

.hero h1 {
  margin: 0 0 0.4rem;
  font-size: 2.2rem;
  letter-spacing: -0.02em;
}

.tagline {
  margin: 0;
  max-width: 38rem;
  color: #555;
}

/* ── Right-side hero actions (meta + file controls) ──────────────────────── */

.hero-actions {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 0.5rem;
}

.hero-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4rem 1rem;
  font-size: 0.85rem;
  color: #666;
  font-variant-numeric: tabular-nums;
}

.hero-meta .range {
  color: #888;
}

/* ── File controls ────────────────────────────────────────────────────────── */

.file-controls {
  display: flex;
  align-items: center;
  gap: 0.6rem;
}

.btn-open {
  padding: 0.35rem 0.8rem;
  font-size: 0.82rem;
  font-weight: 500;
  color: #1d4ed8;
  background: #eff6ff;
  border: 1px solid #bfdbfe;
  border-radius: 0.375rem;
  cursor: pointer;
  white-space: nowrap;
  transition: background 0.15s, border-color 0.15s;
}

.btn-open:hover {
  background: #dbeafe;
  border-color: #93c5fd;
}

.btn-open:active {
  background: #bfdbfe;
}

.source-label {
  font-size: 0.78rem;
  color: #666;
  max-width: 14rem;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ── Visually hidden (accessible hide for file input) ────────────────────── */

.visually-hidden {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

/* ── Content ──────────────────────────────────────────────────────────────── */

.content {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 1rem;
  max-width: 64rem;
  width: 100%;
  margin: 0 auto;
  padding: 1.25rem 1.5rem;
}

.card {
  background: #fff;
  border: 1px solid #e5e5e5;
  border-radius: 0.5rem;
  padding: 1rem 1.25rem;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}

.card.error {
  border-color: #c44;
  color: #722;
}

.card.placeholder {
  text-align: center;
  color: #888;
  padding: 2rem 1rem;
}

/* ── Footer ───────────────────────────────────────────────────────────────── */

.footer {
  border-top: 1px solid #e5e5e5;
  padding: 1rem 1.5rem;
  text-align: center;
  color: #666;
  font-size: 0.9rem;
  background: #fff;
}
</style>
