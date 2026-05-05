<script setup lang="ts">
import { computed } from "vue";
import { storeToRefs } from "pinia";
import Decimal from "decimal.js";
import { useJournalStore } from "@/store/journal";
import { useFiltersStore } from "@/store/filters";
import {
  avgMonthlyExpense,
  cashOnHand,
  netWorthAsOfLatest,
  periodNet,
} from "@/lib/views/period";
import { formatAmount } from "@/lib/format";

const journals = useJournalStore();
const filters = useFiltersStore();
const { begin, end, clearedOnly } = storeToRefs(filters);

interface Kpi {
  label: string;
  value: Decimal;
  hint: string;
}

const cards = computed<Kpi[]>(() => {
  const j = journals.journal;
  if (!j) return [];
  const nw = netWorthAsOfLatest(j, clearedOnly.value);
  return [
    {
      label: "Net Worth",
      value: nw.netWorth,
      hint: `assets ${formatAmount("$", nw.assets)} − liabilities ${formatAmount("$", nw.liabilities)}`,
    },
    {
      label: "Cash on Hand",
      value: cashOnHand(j, clearedOnly.value),
      hint: "Bank + Cash accounts",
    },
    {
      label: "Period Net",
      value: periodNet(j, begin.value, end.value, clearedOnly.value),
      hint: "income − expenses over the active range",
    },
    {
      label: "Avg Monthly Expense",
      value: avgMonthlyExpense(j, begin.value, end.value, clearedOnly.value),
      hint: "expenses ÷ months in range",
    },
  ];
});
</script>

<template>
  <section class="kpi-strip">
    <div v-for="card in cards" :key="card.label" class="card kpi">
      <div class="label">{{ card.label }}</div>
      <div class="value" :class="{ negative: card.value.isNegative() }">
        {{ formatAmount("$", card.value) }}
      </div>
      <div class="hint">{{ card.hint }}</div>
    </div>
  </section>
</template>

<style scoped>
.kpi-strip {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 0.75rem;
}

.card.kpi {
  background: #fff;
  border: 1px solid #e5e5e5;
  border-radius: 0.5rem;
  padding: 0.85rem 1rem;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}

.label {
  font-size: 0.72rem;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: #777;
}

.value {
  font-size: 1.6rem;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  margin-top: 0.15rem;
  color: #1a1a1a;
}

.value.negative {
  color: #b62325;
}

.hint {
  font-size: 0.78rem;
  color: #888;
  margin-top: 0.2rem;
}

@media (max-width: 800px) {
  .kpi-strip {
    grid-template-columns: repeat(2, 1fr);
  }
}
</style>
