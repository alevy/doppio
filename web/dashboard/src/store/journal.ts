import { defineStore } from "pinia";
import { DopError, readDop, type Journal } from "doppio-dop";

interface State {
  journal: Journal | null;
  error: string | null;
  loading: boolean;
  /** Human-readable label for the currently-loaded source (URL basename or local filename). */
  sourceLabel: string | null;
}

export const useJournalStore = defineStore("journal", {
  state: (): State => ({
    journal: null,
    error: null,
    loading: false,
    sourceLabel: null,
  }),
  actions: {
    async loadFromUrl(url: string) {
      this.loading = true;
      this.error = null;
      try {
        const res = await fetch(url);
        if (!res.ok) {
          throw new Error(`fetch ${url} → HTTP ${res.status}`);
        }
        const buf = new Uint8Array(await res.arrayBuffer());
        this.journal = readDop(buf);
        // Display only the final path segment of the URL as the source label.
        this.sourceLabel = url.split("/").pop() ?? url;
      } catch (e) {
        this.journal = null;
        this.sourceLabel = null;
        if (e instanceof DopError) {
          this.error = `[${e.kind}] ${e.message}`;
        } else if (e instanceof Error) {
          this.error = e.message;
        } else {
          this.error = String(e);
        }
      } finally {
        this.loading = false;
      }
    },

    loadFromBuffer(buf: ArrayBuffer, label: string) {
      this.loading = true;
      this.error = null;
      try {
        this.journal = readDop(new Uint8Array(buf));
        this.sourceLabel = label;
      } catch (e) {
        this.journal = null;
        this.sourceLabel = null;
        if (e instanceof DopError) {
          this.error = `[${e.kind}] ${e.message}`;
        } else if (e instanceof Error) {
          this.error = e.message;
        } else {
          this.error = String(e);
        }
      } finally {
        this.loading = false;
      }
    },
  },
});
