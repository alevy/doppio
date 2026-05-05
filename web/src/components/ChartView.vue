<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { Line } from "vue-chartjs";
import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend,
  Filler,
  TimeScale,
} from "chart.js";
import "chartjs-adapter-date-fns";
import Decimal from "decimal.js";
import { useJournalStore } from "@/store/journal";
import { useFiltersStore } from "@/store/filters";
import { accountCommodityPairs, buildAccountSeries } from "@/lib/views/chart";
import { localDateToJSDate, localDateToString } from "@/lib/dop";
import { formatAmount } from "@/lib/format";

ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Legend,
  Filler,
  TimeScale,
);

const journals = useJournalStore();
const filters = useFiltersStore();
const { begin, end, naturalSigns } = storeToRefs(filters);

const pairs = computed(() => {
  const j = journals.journal;
  return j ? accountCommodityPairs(j) : [];
});

// Index into pairs.value. Avoids encoding (account, commodity) into a
// single string key — accounts contain `:` and commodities are arbitrary.
const selectedIdx = ref<number>(0);

watch(
  pairs,
  (ps) => {
    const preferredAt = ps.findIndex(
      (p) => p.account === "Assets:Bank:Checking" && p.commodity === "$",
    );
    selectedIdx.value = preferredAt >= 0 ? preferredAt : 0;
  },
  { immediate: true },
);

const selected = computed(() => pairs.value[selectedIdx.value] ?? null);

const series = computed(() => {
  if (!journals.journal || !selected.value) return [];
  return buildAccountSeries(
    journals.journal,
    selected.value.account,
    selected.value.commodity,
    begin.value,
    end.value,
    naturalSigns.value,
  );
});

const chartData = computed(() => ({
  labels: series.value.map((p) => localDateToJSDate(p.date)),
  datasets: [
    {
      label: selected.value
        ? `${selected.value.account} (${selected.value.commodity})`
        : "",
      data: series.value.map((p) => p.value.toNumber()),
      fill: true,
      borderColor: "#1769aa",
      backgroundColor: "rgba(23, 105, 170, 0.12)",
      tension: 0.15,
      pointRadius: 2,
      pointHoverRadius: 5,
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
      time: { unit: "month" as const },
      grid: { color: "#f0f0f0" },
    },
    y: {
      grid: { color: "#f0f0f0" },
      ticks: {
        callback: (v: string | number) =>
          selected.value
            ? formatAmount(selected.value.commodity, new Decimal(Number(v)))
            : v,
      },
    },
  },
  plugins: {
    legend: { display: false },
    tooltip: {
      callbacks: {
        title: (items: { parsed: { x: number | null } }[]): string => {
          const x = items[0]?.parsed.x;
          return x == null ? "" : new Date(x).toISOString().slice(0, 10);
        },
        label: (ctx: { parsed: { y: number | null } }): string => {
          const y = ctx.parsed.y;
          if (y == null) return "";
          return selected.value
            ? formatAmount(selected.value.commodity, new Decimal(y))
            : String(y);
        },
      },
    },
  },
}));

const summary = computed(() => {
  if (series.value.length === 0) return null;
  const first = series.value[0]!;
  const last = series.value[series.value.length - 1]!;
  return {
    first: { date: localDateToString(first.date), value: first.value },
    last: { date: localDateToString(last.date), value: last.value },
  };
});
</script>

<template>
  <div class="chart-pane">
    <div class="controls">
      <label>
        <span class="label">Account / commodity</span>
        <select v-model.number="selectedIdx">
          <option v-for="(p, i) in pairs" :key="`${p.account}|${p.commodity}`" :value="i">
            {{ p.account }} ({{ p.commodity }})
          </option>
        </select>
      </label>
      <div v-if="summary && selected" class="summary">
        <span>
          <strong>{{ summary.first.date }}</strong>
          {{ formatAmount(selected.commodity, summary.first.value) }}
          →
          <strong>{{ summary.last.date }}</strong>
          {{ formatAmount(selected.commodity, summary.last.value) }}
        </span>
      </div>
    </div>
    <div v-if="series.length === 0" class="empty">
      No activity for this account in the selected date range.
    </div>
    <div v-else class="canvas">
      <Line :data="chartData" :options="chartOptions" />
    </div>
  </div>
</template>

<style scoped>
.chart-pane {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.controls {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-end;
  gap: 1rem;
}

.controls label {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  font-size: 0.85rem;
  color: #555;
}

.label {
  font-size: 0.72rem;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: #777;
}

select {
  padding: 0.3rem 0.5rem;
  border: 1px solid #ccc;
  border-radius: 0.25rem;
  font: inherit;
  background: #fafafa;
  min-width: 18rem;
}

.summary {
  font-size: 0.85rem;
  color: #555;
}

.canvas {
  height: 320px;
}

.empty {
  padding: 3rem 1rem;
  text-align: center;
  color: #888;
}
</style>
