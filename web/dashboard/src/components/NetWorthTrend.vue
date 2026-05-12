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
} from "chart.js";
import Decimal from "decimal.js";
import { storeToRefs } from "pinia";
import { useJournalStore } from "@/store/journal";
import { useFiltersStore } from "@/store/filters";
import { netWorthByMonth, type ConversionContext } from "@/lib/views/period";
import { formatAmount, monthLabelLong, monthLabelShort } from "@/lib/format";

ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Tooltip,
  Filler,
);

const journals = useJournalStore();
const filters = useFiltersStore();
const { clearedOnly, end, displayCommodity } = storeToRefs(filters);

// Net-worth trend uses the as-of cutoff for all bars (true per-date
// rate lookup across a time series is a follow-up).
const convCtx = computed<ConversionContext | null>(() => {
  const j = journals.journal;
  const dc = displayCommodity.value;
  if (!j || dc === null) return null;
  return { toCommodity: dc, prices: j.prices, asOf: end.value };
});

const displayCommodityLabel = computed(() => displayCommodity.value ?? "$");

const series = computed(() => {
  const j = journals.journal;
  return j ? netWorthByMonth(j, clearedOnly.value, convCtx.value) : [];
});

const commodity = computed(() => displayCommodityLabel.value);

const chartData = computed(() => ({
  labels: series.value.map((p) => monthLabelShort(p.month)),
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
    x: { grid: { color: "#f0f0f0" } },
    y: {
      grid: { color: "#f0f0f0" },
      ticks: {
        callback: (v: string | number) => formatAmount(commodity.value, new Decimal(Number(v))),
      },
    },
  },
  plugins: {
    legend: { display: false },
    tooltip: {
      callbacks: {
        title: (items: { dataIndex: number }[]) => {
          const idx = items[0]?.dataIndex;
          if (idx == null) return "";
          const p = series.value[idx];
          return p ? monthLabelLong(p.month) : "";
        },
        label: (ctx: { parsed: { y: number | null } }) =>
          ctx.parsed.y == null ? "" : formatAmount(commodity.value, new Decimal(ctx.parsed.y)),
      },
    },
  },
}));

const hintLabel = computed(() =>
  displayCommodity.value
    ? `monthly · assets − liabilities · ${displayCommodity.value}`
    : "monthly · assets − liabilities",
);
</script>

<template>
  <section class="card">
    <header>
      <h2>Net Worth Trend</h2>
      <span class="hint">{{ hintLabel }}</span>
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
