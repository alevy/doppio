<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import Decimal from "decimal.js";
import {
  readDop,
  localDateToString,
  type Journal,
  type LocalDate,
  DopError,
} from "@/lib/dop";

const journal = ref<Journal | null>(null);
const error = ref<string | null>(null);

interface BalanceLine {
  account: string;
  byCommodity: { commodity: string; total: Decimal }[];
}

const summary = computed(() => {
  if (!journal.value) return null;
  const j = journal.value;
  const dates = j.transactions.map((t) => t.date);
  return {
    transactions: j.transactions.length,
    accounts: Object.keys(j.accounts).length,
    commodities: Object.keys(j.commodities).length,
    prices: j.prices.length,
    firstDate: dates.length ? minDate(dates) : null,
    lastDate: dates.length ? maxDate(dates) : null,
  };
});

const balanceByAccount = computed<BalanceLine[]>(() => {
  if (!journal.value) return [];
  const totals: Record<string, Map<string, Decimal>> = {};
  for (const t of journal.value.transactions) {
    for (const p of t.postings) {
      if (p.kind === "virtualUnbalanced") continue;
      const row = (totals[p.account] ??= new Map());
      for (const [c, v] of Object.entries(p.amount.byCommodity)) {
        row.set(c, (row.get(c) ?? new Decimal(0)).plus(v));
      }
    }
  }
  return Object.keys(totals)
    .sort()
    .map((account) => ({
      account,
      byCommodity: [...totals[account]!.entries()]
        .filter(([_, v]) => !v.isZero())
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([commodity, total]) => ({ commodity, total })),
    }))
    .filter((line) => line.byCommodity.length > 0);
});

function minDate(ds: LocalDate[]): LocalDate {
  return ds.reduce((a, b) =>
    a.year < b.year ||
    (a.year === b.year && a.month < b.month) ||
    (a.year === b.year && a.month === b.month && a.day < b.day)
      ? a
      : b,
  );
}

function maxDate(ds: LocalDate[]): LocalDate {
  return ds.reduce((a, b) =>
    a.year > b.year ||
    (a.year === b.year && a.month > b.month) ||
    (a.year === b.year && a.month === b.month && a.day > b.day)
      ? a
      : b,
  );
}

function formatAmount(commodity: string, value: Decimal): string {
  const sign = value.isNegative() ? "-" : "";
  const abs = value.abs().toFixed(2);
  if (commodity === "$") return `${sign}$${abs}`;
  return `${sign}${abs} ${commodity}`;
}

onMounted(async () => {
  try {
    const url = `${import.meta.env.BASE_URL}sample.dop`;
    const res = await fetch(url);
    if (!res.ok) {
      throw new Error(`fetch ${url} → HTTP ${res.status}`);
    }
    const buf = new Uint8Array(await res.arrayBuffer());
    journal.value = readDop(buf);
  } catch (e) {
    if (e instanceof DopError) {
      error.value = `[${e.kind}] ${e.message}`;
    } else if (e instanceof Error) {
      error.value = e.message;
    } else {
      error.value = String(e);
    }
  }
});
</script>

<template>
  <header class="hero">
    <h1>doppio</h1>
    <p class="tagline">
      A typed compiler pipeline for
      <a href="https://ledger-cli.org/" target="_blank" rel="noopener">Ledger</a>
      plain-text accounting. This page reads a compiled
      <code>.dop</code> file using a JS-native protobuf decoder — no Rust or
      WASM at runtime.
    </p>
  </header>

  <main class="content">
    <section v-if="error" class="card error">
      <h2>Failed to load <code>sample.dop</code></h2>
      <pre>{{ error }}</pre>
    </section>

    <section v-else-if="!journal" class="card">
      <h2>Loading…</h2>
    </section>

    <template v-else>
      <section class="card">
        <h2>Journal summary</h2>
        <dl class="stats">
          <div><dt>Transactions</dt><dd>{{ summary!.transactions }}</dd></div>
          <div><dt>Accounts</dt><dd>{{ summary!.accounts }}</dd></div>
          <div><dt>Commodities</dt><dd>{{ summary!.commodities }}</dd></div>
          <div><dt>Price quotes</dt><dd>{{ summary!.prices }}</dd></div>
          <div>
            <dt>Date range</dt>
            <dd>
              {{ localDateToString(summary!.firstDate!) }} →
              {{ localDateToString(summary!.lastDate!) }}
            </dd>
          </div>
        </dl>
      </section>

      <section class="card">
        <h2>Balance</h2>
        <table class="balance">
          <tbody>
            <tr v-for="line in balanceByAccount" :key="line.account">
              <th scope="row">{{ line.account }}</th>
              <td>
                <span
                  v-for="({ commodity, total }, idx) in line.byCommodity"
                  :key="commodity"
                  class="amount"
                  :class="{ negative: total.isNegative() }"
                >
                  <template v-if="idx > 0">, </template>
                  {{ formatAmount(commodity, total) }}
                </span>
              </td>
            </tr>
          </tbody>
        </table>
      </section>
    </template>
  </main>

  <footer class="footer">
    <p>
      Source:
      <a href="https://github.com/alevy/doppio" target="_blank" rel="noopener">
        github.com/alevy/doppio
      </a>
      ·
      Schema:
      <a
        href="https://github.com/alevy/doppio/blob/main/proto/doppio.proto"
        target="_blank"
        rel="noopener"
      >
        proto/doppio.proto
      </a>
    </p>
  </footer>
</template>

<style scoped>
.hero {
  padding: 2.5rem 1.5rem 1.5rem;
  text-align: center;
  border-bottom: 1px solid #e5e5e5;
  background: #fff;
}

.hero h1 {
  margin: 0 0 0.5rem;
  font-size: 2.4rem;
  letter-spacing: -0.02em;
}

.tagline {
  margin: 0 auto;
  max-width: 40rem;
  color: #555;
}

.content {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 1.25rem;
  padding: 2rem 1.5rem;
}

.card {
  width: 100%;
  max-width: 44rem;
  background: #fff;
  border: 1px solid #e5e5e5;
  border-radius: 0.5rem;
  padding: 1.25rem 1.5rem;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
}

.card h2 {
  margin: 0 0 0.75rem;
  font-size: 1.05rem;
  color: #333;
}

.error {
  border-color: #c44;
  color: #722;
}

.stats {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
  gap: 0.75rem 1.5rem;
  margin: 0;
}

.stats div { display: flex; flex-direction: column; }
.stats dt { font-size: 0.78rem; color: #777; text-transform: uppercase; letter-spacing: 0.04em; }
.stats dd { margin: 0.15rem 0 0; font-size: 1.05rem; }

table.balance {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.95rem;
}

table.balance th {
  text-align: left;
  font-weight: 500;
  color: #444;
  padding: 0.3rem 0.75rem 0.3rem 0;
  border-bottom: 1px solid #f0f0f0;
}

table.balance td {
  text-align: right;
  font-variant-numeric: tabular-nums;
  padding: 0.3rem 0;
  border-bottom: 1px solid #f0f0f0;
}

.amount.negative { color: #b62325; }
</style>
