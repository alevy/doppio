<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { storeToRefs } from "pinia";
import { useJournalStore } from "@/store/journal";
import FilterBar from "@/components/FilterBar.vue";
import BalanceView from "@/components/BalanceView.vue";
import RegisterView from "@/components/RegisterView.vue";
import ChartView from "@/components/ChartView.vue";
import { localDateToString } from "@/lib/dop";

const journals = useJournalStore();
const { journal, error, loading } = storeToRefs(journals);

type Tab = "balance" | "register" | "chart";
const activeTab = ref<Tab>("balance");

const summary = computed(() => {
  const j = journal.value;
  if (!j) return null;
  const dates = j.transactions.map((t) => t.date);
  const first = dates[0];
  const last = dates[dates.length - 1];
  return {
    transactions: j.transactions.length,
    accounts: Object.keys(j.accounts).length,
    commodities: Object.keys(j.commodities).length,
    prices: j.prices.length,
    range: first && last ? `${localDateToString(first)} → ${localDateToString(last)}` : null,
  };
});

onMounted(() => {
  const url = `${import.meta.env.BASE_URL}sample.dop`;
  journals.loadFromUrl(url);
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

    <section v-else-if="loading || !journal" class="card">
      <h2>Loading…</h2>
    </section>

    <template v-else>
      <section class="card summary-bar">
        <span><strong>{{ summary!.transactions }}</strong> transactions</span>
        <span><strong>{{ summary!.accounts }}</strong> accounts</span>
        <span><strong>{{ summary!.commodities }}</strong> commodities</span>
        <span><strong>{{ summary!.prices }}</strong> prices</span>
        <span class="range" v-if="summary!.range">{{ summary!.range }}</span>
      </section>

      <FilterBar :show-depth="activeTab === 'balance'" />

      <nav class="tabs" role="tablist">
        <button
          type="button"
          role="tab"
          :aria-selected="activeTab === 'balance'"
          :class="{ active: activeTab === 'balance' }"
          @click="activeTab = 'balance'"
        >
          Balance
        </button>
        <button
          type="button"
          role="tab"
          :aria-selected="activeTab === 'register'"
          :class="{ active: activeTab === 'register' }"
          @click="activeTab = 'register'"
        >
          Register
        </button>
        <button
          type="button"
          role="tab"
          :aria-selected="activeTab === 'chart'"
          :class="{ active: activeTab === 'chart' }"
          @click="activeTab = 'chart'"
        >
          Chart
        </button>
      </nav>

      <section class="card pane">
        <BalanceView v-if="activeTab === 'balance'" />
        <RegisterView v-else-if="activeTab === 'register'" />
        <ChartView v-else-if="activeTab === 'chart'" />
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
  padding: 2rem 1.5rem 1rem;
  text-align: center;
  border-bottom: 1px solid #e5e5e5;
  background: #fff;
}

.hero h1 {
  margin: 0 0 0.4rem;
  font-size: 2.2rem;
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
  align-items: stretch;
  max-width: 64rem;
  width: 100%;
  margin: 0 auto;
  padding: 1.25rem 1.5rem;
}

.card {
  background: #fff;
  border: 1px solid #e5e5e5;
  border-radius: 0.5rem;
  padding: 1rem 1.25rem;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.04);
  margin-bottom: 0;
}

.card.error {
  border-color: #c44;
  color: #722;
  margin-bottom: 1rem;
}

.summary-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem 1.5rem;
  align-items: baseline;
  font-size: 0.9rem;
  color: #555;
  margin-bottom: 1rem;
}

.summary-bar .range {
  color: #888;
  margin-left: auto;
  font-variant-numeric: tabular-nums;
}

.tabs {
  display: flex;
  gap: 0.25rem;
  margin: 0 0 -1px;
}

.tabs button {
  padding: 0.5rem 1rem;
  border: 1px solid #e5e5e5;
  border-bottom: 1px solid transparent;
  border-top-left-radius: 0.4rem;
  border-top-right-radius: 0.4rem;
  background: #f4f4f4;
  cursor: pointer;
  font: inherit;
  color: #555;
}

.tabs button.active {
  background: #fff;
  color: #1769aa;
  font-weight: 500;
  border-bottom-color: #fff;
}

.tabs button:hover:not(.active) {
  background: #ececec;
}

.pane {
  border-top-left-radius: 0;
  padding: 1.25rem;
  min-height: 16rem;
}

.footer {
  border-top: 1px solid #e5e5e5;
  padding: 1rem 1.5rem;
  text-align: center;
  color: #666;
  font-size: 0.9rem;
  background: #fff;
}
</style>
