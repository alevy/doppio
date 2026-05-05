<script setup lang="ts">
import { computed } from "vue";
import { storeToRefs } from "pinia";
import { useJournalStore } from "@/store/journal";
import { useFiltersStore } from "@/store/filters";
import { buildBalanceTree, type BalanceNode } from "@/lib/views/balance";
import { formatAmount } from "@/lib/format";

const journals = useJournalStore();
const filters = useFiltersStore();
const { pattern, clearedOnly, begin, end, depth, naturalSigns } = storeToRefs(filters);

const tree = computed<BalanceNode[]>(() => {
  const j = journals.journal;
  if (!j) return [];
  return buildBalanceTree(
    j,
    {
      pattern: pattern.value,
      clearedOnly: clearedOnly.value,
      begin: begin.value,
      end: end.value,
    },
    depth.value,
    naturalSigns.value,
  );
});

function flatten(nodes: BalanceNode[]): BalanceNode[] {
  const out: BalanceNode[] = [];
  function walk(ns: BalanceNode[]) {
    for (const n of ns) {
      out.push(n);
      walk(n.children);
    }
  }
  walk(nodes);
  return out;
}

const rows = computed(() => flatten(tree.value));
</script>

<template>
  <div v-if="rows.length === 0" class="empty">
    No matching accounts.
  </div>
  <table v-else class="balance">
    <tbody>
      <tr v-for="node in rows" :key="node.fullName">
        <th
          scope="row"
          :style="{ paddingLeft: `${(node.depth - 1) * 1.25}rem` }"
          :class="{ 'has-children': node.children.length > 0 }"
        >
          {{ node.segment }}
        </th>
        <td>
          <span
            v-for="([commodity, total], idx) in Object.entries(node.rollupTotals.byCommodity).sort(([a], [b]) => a.localeCompare(b))"
            :key="commodity"
            class="amount"
            :class="{ negative: total.isNegative() }"
          >
            <template v-if="idx > 0">, </template>{{ formatAmount(commodity, total) }}
          </span>
        </td>
      </tr>
    </tbody>
  </table>
</template>

<style scoped>
table.balance {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.95rem;
}

th {
  text-align: left;
  font-weight: 500;
  color: #444;
  padding: 0.3rem 0.75rem 0.3rem 0;
  border-bottom: 1px solid #f0f0f0;
}

th.has-children {
  font-weight: 600;
  color: #222;
}

td {
  text-align: right;
  font-variant-numeric: tabular-nums;
  padding: 0.3rem 0;
  border-bottom: 1px solid #f0f0f0;
}

.amount.negative {
  color: #b62325;
}

.empty {
  padding: 2rem 1rem;
  text-align: center;
  color: #888;
}
</style>
