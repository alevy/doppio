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
  type ConversionContext,
} from "@/lib/views/period";
import { convertByCommodity } from "@/lib/views/exchange";
import { formatAmount } from "@/lib/format";

const journals = useJournalStore();
const filters = useFiltersStore();
const { begin, end, clearedOnly, displayCommodity } = storeToRefs(filters);

/** Build a ConversionContext from the current filter state, or null when
 * "as recorded" is selected. */
const convCtx = computed<ConversionContext | null>(() => {
  const j = journals.journal;
  const dc = displayCommodity.value;
  if (!j || dc === null) return null;
  return { toCommodity: dc, prices: j.prices, asOf: end.value };
});

interface Kpi {
  label: string;
  value: Decimal;
  hint: string;
  commodity: string;
}

/** Commodities that could not be converted for the net-worth KPI.
 * Computed separately so we can surface a badge in the UI. */
const netWorthUnconvertible = computed<string[]>(() => {
  const j = journals.journal;
  const ctx = convCtx.value;
  if (!j || ctx === null) return [];

  // Walk all asset and liability postings, collecting unconvertible ones.
  const seen = new Set<string>();
  for (const t of j.transactions) {
    if (clearedOnly.value && t.state !== "cleared") continue;
    for (const p of t.postings) {
      if (p.kind === "virtualUnbalanced") continue;
      const { unconvertible } = convertByCommodity(
        p.amount.byCommodity,
        ctx.toCommodity,
        ctx.prices,
        ctx.asOf,
      );
      for (const c of unconvertible) seen.add(c);
    }
  }
  // Remove the target commodity itself — it's never unconvertible.
  seen.delete(ctx.toCommodity);
  return [...seen].sort();
});

const displayCommodityLabel = computed(() => displayCommodity.value ?? "$");

const cards = computed<Kpi[]>(() => {
  const j = journals.journal;
  if (!j) return [];
  const ctx = convCtx.value;
  const commodity = displayCommodityLabel.value;
  const nw = netWorthAsOfLatest(j, clearedOnly.value, ctx);
  return [
    {
      label: "Net Worth",
      value: nw.netWorth,
      hint: `assets ${formatAmount(commodity, nw.assets)} − liabilities ${formatAmount(commodity, nw.liabilities)}`,
      commodity,
    },
    {
      label: "Cash on Hand",
      value: cashOnHand(j, clearedOnly.value, ctx),
      hint: "Bank + Cash accounts",
      commodity,
    },
    {
      label: "Period Net",
      value: periodNet(j, begin.value, end.value, clearedOnly.value, ctx),
      hint: "income − expenses over the active range",
      commodity,
    },
    {
      label: "Avg Monthly Expense",
      value: avgMonthlyExpense(j, begin.value, end.value, clearedOnly.value, ctx),
      hint: "expenses ÷ months in range",
      commodity,
    },
  ];
});
</script>

<template>
  <section class="kpi-strip">
    <div v-for="card in cards" :key="card.label" class="card kpi">
      <div class="label">{{ card.label }}</div>
      <div class="value" :class="{ negative: card.value.isNegative() }">
        {{ formatAmount(card.commodity, card.value) }}
      </div>
      <div class="hint">{{ card.hint }}</div>
    </div>
    <div
      v-if="netWorthUnconvertible.length > 0"
      class="unconvertible-note"
    >
      No rate for: {{ netWorthUnconvertible.join(", ") }} — excluded from totals
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

.unconvertible-note {
  grid-column: 1 / -1;
  font-size: 0.78rem;
  color: #b06b00;
  background: #fff8ec;
  border: 1px solid #eed9a0;
  border-radius: 0.35rem;
  padding: 0.35rem 0.75rem;
}

@media (max-width: 800px) {
  .kpi-strip {
    grid-template-columns: repeat(2, 1fr);
  }
}
</style>
