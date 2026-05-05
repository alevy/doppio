<script setup lang="ts">
import { computed } from "vue";
import { storeToRefs } from "pinia";
import { Doughnut } from "vue-chartjs";
import {
  Chart as ChartJS,
  ArcElement,
  Tooltip,
} from "chart.js";
import Decimal from "decimal.js";
import { useJournalStore } from "@/store/journal";
import { useFiltersStore } from "@/store/filters";
import { expensesByCategory, latestMonth } from "@/lib/views/period";
import { formatAmount } from "@/lib/format";

ChartJS.register(ArcElement, Tooltip);

const journals = useJournalStore();
const filters = useFiltersStore();
const { begin, end, clearedOnly } = storeToRefs(filters);

const month = computed(() => {
  const j = journals.journal;
  return j ? latestMonth(j, begin.value, end.value) : null;
});

const categories = computed(() => {
  const j = journals.journal;
  const m = month.value;
  if (!j || !m) return [];
  return expensesByCategory(j, m, clearedOnly.value);
});

const total = computed(() =>
  categories.value.reduce((acc, c) => acc.plus(c.total), new Decimal(0)),
);

// A small palette of muted, distinguishable colours. Chart.js cycles
// through these in order — we only need as many as the largest
// category list seen in practice.
const PALETTE = [
  "#7e8ed1",
  "#d68a72",
  "#88a98a",
  "#c4a55b",
  "#9a73a8",
  "#5fa8a8",
  "#bf6f8a",
  "#8a8a8a",
];

const chartData = computed(() => ({
  labels: categories.value.map((c) => c.label),
  datasets: [
    {
      data: categories.value.map((c) => c.total.toNumber()),
      backgroundColor: categories.value.map((_, i) => PALETTE[i % PALETTE.length]),
      borderColor: "#fff",
      borderWidth: 2,
    },
  ],
}));

const chartOptions = computed(() => ({
  responsive: true,
  maintainAspectRatio: false,
  cutout: "60%",
  plugins: {
    legend: { display: false },
    tooltip: {
      callbacks: {
        label: (ctx: { label?: string; parsed: number }) =>
          `${ctx.label ?? ""}: ${formatAmount("$", new Decimal(ctx.parsed))}`,
      },
    },
  },
}));

const monthLabel = computed(() => {
  if (!month.value) return "";
  return new Date(Date.UTC(month.value.year, month.value.month - 1, 1))
    .toLocaleDateString("en", { month: "long", year: "numeric" });
});
</script>

<template>
  <section class="card">
    <header>
      <h2>Where it Went</h2>
      <span class="hint">{{ monthLabel || "—" }}</span>
    </header>
    <div v-if="categories.length === 0" class="empty">No expenses this month.</div>
    <div v-else class="content">
      <div class="canvas">
        <Doughnut :data="chartData" :options="chartOptions" />
        <div class="centre-label">
          <div class="centre-amount">{{ formatAmount("$", total) }}</div>
          <div class="centre-caption">spent</div>
        </div>
      </div>
      <ol class="legend">
        <li v-for="(c, i) in categories" :key="c.label">
          <span
            class="swatch"
            :style="{ background: PALETTE[i % PALETTE.length] }"
          />
          <span class="cat-label">{{ c.label }}</span>
          <span class="cat-amount">{{ formatAmount("$", c.total) }}</span>
        </li>
      </ol>
    </div>
  </section>
</template>

<style scoped>
.card {
  background: #fff;
  border: 1px solid #e5e5e5;
  border-radius: 0.5rem;
  padding: 1rem 1.25rem;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  display: flex;
  flex-direction: column;
}

header {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  margin-bottom: 0.5rem;
}

h2 {
  margin: 0;
  font-size: 1rem;
  font-weight: 600;
  color: #333;
}

.hint {
  font-size: 0.78rem;
  color: #888;
}

.content {
  display: grid;
  grid-template-columns: 180px 1fr;
  gap: 1rem;
  align-items: center;
}

.canvas {
  position: relative;
  width: 180px;
  height: 180px;
}

.centre-label {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  pointer-events: none;
}

.centre-amount {
  font-size: 1.05rem;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

.centre-caption {
  font-size: 0.72rem;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: #888;
}

.legend {
  list-style: none;
  margin: 0;
  padding: 0;
  font-size: 0.85rem;
}

.legend li {
  display: grid;
  grid-template-columns: 14px 1fr auto;
  align-items: center;
  gap: 0.5rem;
  padding: 0.2rem 0;
  border-bottom: 1px dashed #f0f0f0;
}

.swatch {
  width: 10px;
  height: 10px;
  border-radius: 2px;
  border: 1px solid rgba(0, 0, 0, 0.1);
}

.cat-label {
  color: #444;
}

.cat-amount {
  font-variant-numeric: tabular-nums;
  color: #555;
}

.empty {
  padding: 2rem 1rem;
  text-align: center;
  color: #888;
}
</style>
