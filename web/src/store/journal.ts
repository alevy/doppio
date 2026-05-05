import { defineStore } from "pinia";

// Pinia store stub. Populated by the loader (#151) and consumed by the views (#150).
export const useJournalStore = defineStore("journal", {
  state: () => ({
    loaded: false as boolean,
    sourcePath: null as string | null,
  }),
});
