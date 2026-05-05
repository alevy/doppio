import { defineStore } from "pinia";
import type { LocalDate } from "@/lib/dop";

interface State {
  // Substring filter applied to account names. Case-insensitive.
  // Treated as plain substring (not regex) for now — simple and predictable.
  pattern: string;
  // If true, only cleared transactions/postings are included.
  clearedOnly: boolean;
  // Inclusive lower bound on transaction date.
  begin: LocalDate | null;
  // Inclusive upper bound on transaction date.
  end: LocalDate | null;
  // Maximum tree depth in the balance view (1 = top-level only).
  // null means no limit.
  depth: number | null;
}

export const useFiltersStore = defineStore("filters", {
  state: (): State => ({
    pattern: "",
    clearedOnly: false,
    begin: null,
    end: null,
    depth: null,
  }),
});
