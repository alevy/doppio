<script setup lang="ts">
import { computed } from "vue";
import { storeToRefs } from "pinia";
import { useJournalStore } from "@/store/journal";
import { useFiltersStore } from "@/store/filters";
import { buildRegister } from "@/lib/views/register";
import { formatAmount } from "@/lib/format";
import { localDateToString } from "@/lib/dop";

const journals = useJournalStore();
const filters = useFiltersStore();
const { pattern, clearedOnly, begin, end, naturalSigns } = storeToRefs(filters);

const rows = computed(() => {
  const j = journals.journal;
  if (!j) return [];
  return buildRegister(
    j,
    {
      pattern: pattern.value,
      clearedOnly: clearedOnly.value,
      begin: begin.value,
      end: end.value,
    },
    naturalSigns.value,
  );
});

function postingAmounts(commodities: Record<string, import("decimal.js").default>) {
  return Object.entries(commodities).sort(([a], [b]) => a.localeCompare(b));
}
</script>

<template>
  <div v-if="rows.length === 0" class="empty">No matching postings.</div>
  <table v-else class="register">
    <thead>
      <tr>
        <th>Date</th>
        <th>Description</th>
        <th>Account</th>
        <th class="num">Amount</th>
        <th class="num">Running</th>
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="(row, idx) in rows"
        :key="`${idx}-${row.account}`"
        :class="{ pending: row.state === 'pending', uncleared: row.state === 'uncleared' }"
      >
        <td class="date">{{ localDateToString(row.date) }}</td>
        <td class="desc">{{ row.description }}</td>
        <td class="account">{{ row.account }}</td>
        <td class="num">
          <span
            v-for="([commodity, value], i) in postingAmounts(row.amount)"
            :key="commodity"
            class="amount"
            :class="{ negative: value.isNegative() }"
          >
            <template v-if="i > 0">, </template>{{ formatAmount(commodity, value) }}
          </span>
        </td>
        <td class="num running">
          <span
            v-for="([commodity, value], i) in postingAmounts(row.running)"
            :key="commodity"
            class="amount"
            :class="{ negative: value.isNegative() }"
          >
            <template v-if="i > 0">, </template>{{ formatAmount(commodity, value) }}
          </span>
        </td>
      </tr>
    </tbody>
  </table>
</template>

<style scoped>
table.register {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.9rem;
}

th {
  text-align: left;
  font-weight: 500;
  color: #555;
  padding: 0.4rem 0.75rem 0.4rem 0;
  border-bottom: 2px solid #ddd;
  position: sticky;
  top: 0;
  background: #fff;
  font-size: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

th.num,
td.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
}

td {
  padding: 0.3rem 0.75rem 0.3rem 0;
  border-bottom: 1px solid #f3f3f3;
}

td.date {
  white-space: nowrap;
  color: #666;
  font-size: 0.85rem;
  font-variant-numeric: tabular-nums;
}

td.account {
  color: #444;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
  font-size: 0.85rem;
}

td.running {
  color: #666;
}

.amount.negative {
  color: #b62325;
}

tr.pending td {
  font-style: italic;
}

tr.uncleared td.desc::before {
  content: "·";
  color: #aaa;
  margin-right: 0.4rem;
}
</style>
