import { defineStore } from "pinia";
import type { LocalDate } from "@/lib/dop";

interface State {
  // Inclusive lower bound on transaction date.
  begin: LocalDate | null;
  // Inclusive upper bound on transaction date.
  end: LocalDate | null;
  // If true, pending and uncleared transactions are excluded from
  // every dashboard computation.
  clearedOnly: boolean;
}

export const useFiltersStore = defineStore("filters", {
  state: (): State => ({
    begin: null,
    end: null,
    clearedOnly: false,
  }),
});
