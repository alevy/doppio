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
  // When non-null, all commodity-aggregated views convert amounts to
  // this commodity using the journal's P price directives. null means
  // "as recorded" (no conversion).
  displayCommodity: string | null;
}

export const useFiltersStore = defineStore("filters", {
  state: (): State => ({
    begin: null,
    end: null,
    clearedOnly: false,
    displayCommodity: null,
  }),
});
