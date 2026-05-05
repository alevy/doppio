<script setup lang="ts">
import { computed } from "vue";
import { storeToRefs } from "pinia";
import { Bar } from "vue-chartjs";
import { Chart as ChartJS, BarElement, CategoryScale, LinearScale, Tooltip } from "chart.js";
import Decimal from "decimal.js";
import { useJournalStore } from "@/store/journal";
import { useFiltersStore } from "@/store/filters";
import { incomeExpenseByMonth } from "@/lib/views/period";
import { formatAmount, monthLabelShort } from "@/lib/format";

ChartJS.register(BarElement, CategoryScale, LinearScale, Tooltip);

const journals = useJournalStore();
const filters = useFiltersStore();
const { begin, end, clearedOnly } = storeToRefs(filters);

const buckets = computed(() => {
  const j = journals.journal;
  if (!j) return [];
  return incomeExpenseByMonth(j, begin.value, end.value, clearedOnly.value);
});

const chartData = computed(() => ({
  labels: buckets.value.map((b) => monthLabelShort(b.month)),
  datasets: [
    {
      label: "Income",
      data: buckets.value.map((b) => b.income.toNumber()),
      backgroundColor: "rgba(45, 130, 95, 0.65)",
      borderColor: "#2d825f",
      borderWidth: 1,
    },
    {
      label: "Expense",
      data: buckets.value.map((b) => b.expense.toNumber()),
      backgroundColor: "rgba(196, 81, 76, 0.65)",
      borderColor: "#c4514c",
      borderWidth: 1,
    },
  ],
}));

const chartOptions = computed(() => ({
  responsive: true,
  maintainAspectRatio: false,
  scales: {
    x: { grid: { display: false } },
    y: {
      grid: { color: "#f0f0f0" },
      ticks: {
        callback: (v: string | number) => formatAmount("$", new Decimal(Number(v))),
      },
    },
  },
  plugins: {
    legend: { position: "top" as const, labels: { boxWidth: 12, boxHeight: 12 } },
    tooltip: {
      callbacks: {
        label: (ctx: { dataset: { label?: string }; parsed: { y: number | null } }) => {
          const y = ctx.parsed.y;
          const lbl = ctx.dataset.label ?? "";
          if (y == null) return lbl;
          return `${lbl}: ${formatAmount("$", new Decimal(y))}`;
        },
      },
    },
  },
}));
</script>

<template>
  <section class="card">
    <header>
      <h2>Income vs Expense</h2>
      <span class="hint">monthly · USD</span>
    </header>
    <div v-if="buckets.length === 0" class="empty">
      No activity in the selected range.
    </div>
    <div v-else class="canvas">
      <Bar :data="chartData" :options="chartOptions" />
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

.canvas {
  height: 280px;
}

.empty {
  padding: 2.5rem 1rem;
  text-align: center;
  color: #888;
}
</style>
