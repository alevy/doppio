<script setup lang="ts">
import { computed } from "vue";
import { storeToRefs } from "pinia";
import { useFiltersStore } from "@/store/filters";
import { useJournalStore } from "@/store/journal";
import {
  epochDaysFromLocalDate,
  localDateFromEpochDays,
  type LocalDate,
} from "doppio-dop";
import { allCommodities } from "@/lib/views/exchange";

const filters = useFiltersStore();
const journals = useJournalStore();
const { clearedOnly, begin, end, displayCommodity } = storeToRefs(filters);

const beginISO = computed({
  get: () => (begin.value ? toISO(begin.value) : ""),
  set: (v) => {
    begin.value = v ? fromISO(v) : null;
  },
});
const endISO = computed({
  get: () => (end.value ? toISO(end.value) : ""),
  set: (v) => {
    end.value = v ? fromISO(v) : null;
  },
});

function toISO(d: LocalDate): string {
  return new Date(epochDaysFromLocalDate(d) * 86_400_000).toISOString().slice(0, 10);
}

function fromISO(s: string): LocalDate {
  const [y, m, d] = s.split("-").map(Number);
  return localDateFromEpochDays(
    Math.round(Date.UTC(y!, (m ?? 1) - 1, d ?? 1) / 86_400_000),
  );
}

/** Sorted list of all commodity symbols in the loaded journal. */
const commodities = computed<string[]>(() => {
  const j = journals.journal;
  if (!j) return [];
  return allCommodities(j.prices, j.transactions);
});
</script>

<template>
  <div class="filter-bar">
    <label>
      <span class="label">Begin</span>
      <input v-model="beginISO" type="date" />
    </label>
    <label>
      <span class="label">End</span>
      <input v-model="endISO" type="date" />
    </label>
    <label>
      <span class="label">Display as</span>
      <select v-model="displayCommodity" class="commodity-select">
        <option :value="null">As recorded</option>
        <option v-for="c in commodities" :key="c" :value="c">{{ c }}</option>
      </select>
    </label>
    <label class="toggle">
      <input v-model="clearedOnly" type="checkbox" />
      <span>Cleared only</span>
    </label>
  </div>
</template>

<style scoped>
.filter-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem 1rem;
  align-items: flex-end;
  padding: 0.5rem 0.75rem;
  background: #fff;
  border: 1px solid #e5e5e5;
  border-radius: 0.5rem;
}

label {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  font-size: 0.85rem;
  color: #555;
}

label.toggle {
  flex-direction: row;
  align-items: center;
  gap: 0.4rem;
  padding-bottom: 0.3rem;
}

.label {
  font-size: 0.72rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: #777;
}

input[type="date"],
.commodity-select {
  padding: 0.3rem 0.5rem;
  border: 1px solid #ccc;
  border-radius: 0.25rem;
  font: inherit;
  background: #fafafa;
}

input[type="date"] {
  min-width: 9rem;
}

.commodity-select {
  min-width: 8rem;
  cursor: pointer;
}
</style>
