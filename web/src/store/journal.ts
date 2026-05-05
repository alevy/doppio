import { defineStore } from "pinia";
import { DopError, readDop, type Journal } from "@/lib/dop";

interface State {
  journal: Journal | null;
  error: string | null;
  loading: boolean;
}

export const useJournalStore = defineStore("journal", {
  state: (): State => ({
    journal: null,
    error: null,
    loading: false,
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
      } catch (e) {
        this.journal = null;
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
