<script setup lang="ts">
import { computed } from "vue";
import { storeToRefs } from "pinia";
import { useFiltersStore } from "@/store/filters";
import {
  epochDaysFromLocalDate,
  localDateFromEpochDays,
  type LocalDate,
} from "@/lib/dop";

defineProps<{ showDepth?: boolean }>();

const filters = useFiltersStore();
const { pattern, clearedOnly, begin, end, depth, naturalSigns } = storeToRefs(filters);

// Bind LocalDate state to <input type="date"> via ISO YYYY-MM-DD strings.
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
const depthString = computed({
  get: () => (depth.value === null ? "" : String(depth.value)),
  set: (v) => {
    const n = parseInt(v, 10);
    depth.value = Number.isFinite(n) && n > 0 ? n : null;
  },
});

function toISO(d: LocalDate): string {
  return new Date(epochDaysFromLocalDate(d) * 86_400_000).toISOString().slice(0, 10);
}

function fromISO(s: string): LocalDate {
  // <input type="date"> emits YYYY-MM-DD in the user's locale. Treat it
  // as a calendar date — no timezone conversion.
  const [y, m, d] = s.split("-").map(Number);
  return localDateFromEpochDays(
    Math.round(Date.UTC(y!, (m ?? 1) - 1, d ?? 1) / 86_400_000),
  );
}
</script>

<template>
  <div class="filter-bar">
    <label>
      <span class="label">Account</span>
      <input
        v-model="pattern"
        type="text"
        placeholder="filter (substring)"
        spellcheck="false"
      />
    </label>
    <label>
      <span class="label">Begin</span>
      <input v-model="beginISO" type="date" />
    </label>
    <label>
      <span class="label">End</span>
      <input v-model="endISO" type="date" />
    </label>
    <label v-if="showDepth">
      <span class="label">Depth</span>
      <input
        v-model="depthString"
        type="number"
        min="1"
        placeholder="∞"
        class="narrow"
      />
    </label>
    <label class="toggle">
      <input v-model="clearedOnly" type="checkbox" />
      <span>Cleared only</span>
    </label>
    <label class="toggle" title="Display Income / Liabilities / Equity with their natural sign instead of the raw double-entry sign.">
      <input v-model="naturalSigns" type="checkbox" />
      <span>Natural signs</span>
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
  margin-bottom: 1rem;
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

input[type="text"],
input[type="number"],
input[type="date"] {
  padding: 0.3rem 0.5rem;
  border: 1px solid #ccc;
  border-radius: 0.25rem;
  font: inherit;
  min-width: 9rem;
  background: #fafafa;
}

input.narrow {
  min-width: 4rem;
  width: 5rem;
}
</style>
