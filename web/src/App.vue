<script setup lang="ts">
import { computed, onMounted } from "vue";
import { storeToRefs } from "pinia";
import { useJournalStore } from "@/store/journal";
import FilterBar from "@/components/FilterBar.vue";
import DashboardView from "@/components/DashboardView.vue";
import { compareLocalDate, localDateToString } from "@/lib/dop";

const journals = useJournalStore();
const { journal, error, loading } = storeToRefs(journals);

const summary = computed(() => {
  const j = journal.value;
  if (!j || j.transactions.length === 0) return null;
  const dates = j.transactions.map((t) => t.date);
  const sorted = [...dates].sort(compareLocalDate);
  return {
    transactions: j.transactions.length,
    accounts: Object.keys(j.accounts).length,
    last: localDateToString(sorted[sorted.length - 1]!),
    first: localDateToString(sorted[0]!),
  };
});

onMounted(() => {
  const url = `${import.meta.env.BASE_URL}sample.dop`;
  journals.loadFromUrl(url);
});
</script>

<template>
  <header class="hero">
    <div class="hero-inner">
      <div class="hero-titles">
        <h1>doppio</h1>
        <p class="tagline">
          A typed compiler pipeline for
          <a href="https://ledger-cli.org/" target="_blank" rel="noopener">Ledger</a>
          plain-text accounting. This page reads
          <a
            href="https://github.com/alevy/doppio/blob/main/web/fixtures/sample.ledger"
            target="_blank"
            rel="noopener"
          >a fictional sample journal</a>
          via a JS-native protobuf decoder -- no Rust or WASM at runtime.
        </p>
      </div>
      <div v-if="summary" class="hero-meta">
        <span><strong>{{ summary.transactions }}</strong> transactions</span>
        <span><strong>{{ summary.accounts }}</strong> accounts</span>
        <span class="range">{{ summary.first }} → {{ summary.last }}</span>
      </div>
    </div>
  </header>

  <main class="content">
    <section v-if="error" class="card error">
      <h2>Failed to load <code>sample.dop</code></h2>
      <pre>{{ error }}</pre>
    </section>

    <section v-else-if="loading || !journal" class="card placeholder">
      <h2>Loading…</h2>
    </section>

    <template v-else>
      <FilterBar />
      <DashboardView />
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
  padding: 2rem 1.5rem 1.25rem;
  border-bottom: 1px solid #e5e5e5;
  background: #fff;
}

.hero-inner {
  max-width: 64rem;
  margin: 0 auto;
  display: flex;
  flex-wrap: wrap;
  align-items: flex-end;
  gap: 1rem;
}

.hero-titles {
  flex: 1;
  min-width: 18rem;
}

.hero h1 {
  margin: 0 0 0.4rem;
  font-size: 2.2rem;
  letter-spacing: -0.02em;
}

.tagline {
  margin: 0;
  max-width: 38rem;
  color: #555;
}

.hero-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4rem 1rem;
  font-size: 0.85rem;
  color: #666;
  font-variant-numeric: tabular-nums;
}

.hero-meta .range {
  color: #888;
}

.content {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 1rem;
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
}

.card.error {
  border-color: #c44;
  color: #722;
}

.card.placeholder {
  text-align: center;
  color: #888;
  padding: 2rem 1rem;
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
