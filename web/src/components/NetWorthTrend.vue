<script setup lang="ts">
import { computed } from "vue";
import { Line } from "vue-chartjs";
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Tooltip,
  Filler,
  TimeScale,
} from "chart.js";
import "chartjs-adapter-date-fns";
import Decimal from "decimal.js";
import { storeToRefs } from "pinia";
import { useJournalStore } from "@/store/journal";
import { useFiltersStore } from "@/store/filters";
import { netWorthByMonth } from "@/lib/views/period";
import { formatAmount } from "@/lib/format";

ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Tooltip,
  Filler,
  TimeScale,
);

const journals = useJournalStore();
const filters = useFiltersStore();
const { clearedOnly } = storeToRefs(filters);

const series = computed(() => {
  const j = journals.journal;
  return j ? netWorthByMonth(j, clearedOnly.value) : [];
});

const chartData = computed(() => ({
  labels: series.value.map((p) => new Date(Date.UTC(p.month.year, p.month.month - 1, 1))),
  datasets: [
    {
      label: "Net worth",
      data: series.value.map((p) => p.netWorth.toNumber()),
      fill: true,
      borderColor: "#1769aa",
      backgroundColor: "rgba(23, 105, 170, 0.12)",
      tension: 0.2,
      pointRadius: 3,
      pointHoverRadius: 6,
    },
  ],
}));

const chartOptions = computed(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: { mode: "index" as const, intersect: false },
  scales: {
    x: {
      type: "time" as const,
      time: { unit: "month" as const, tooltipFormat: "MMM yyyy" },
      grid: { color: "#f0f0f0" },
    },
    y: {
      grid: { color: "#f0f0f0" },
      ticks: {
        callback: (v: string | number) => formatAmount("$", new Decimal(Number(v))),
      },
    },
  },
  plugins: {
    legend: { display: false },
    tooltip: {
      callbacks: {
        title: (items: { parsed: { x: number | null } }[]) => {
          const x = items[0]?.parsed.x;
          if (x == null) return "";
          return new Date(x).toLocaleDateString("en", { month: "long", year: "numeric" });
        },
        label: (ctx: { parsed: { y: number | null } }) =>
          ctx.parsed.y == null ? "" : formatAmount("$", new Decimal(ctx.parsed.y)),
      },
    },
  },
}));
</script>

<template>
  <section class="card">
    <header>
      <h2>Net Worth Trend</h2>
      <span class="hint">monthly · assets − liabilities</span>
    </header>
    <div v-if="series.length === 0" class="empty">No data.</div>
    <div v-else class="canvas">
      <Line :data="chartData" :options="chartOptions" />
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
  padding: 2rem 1rem;
  text-align: center;
  color: #888;
}
</style>
