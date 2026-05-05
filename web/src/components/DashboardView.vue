<script setup lang="ts">
import KpiStrip from "./KpiStrip.vue";
import IncomeExpenseChart from "./IncomeExpenseChart.vue";
import CategoryDonut from "./CategoryDonut.vue";
import NetWorthTrend from "./NetWorthTrend.vue";
</script>

<template>
  <div class="dashboard">
    <KpiStrip />
    <div class="row">
      <IncomeExpenseChart class="wide" />
      <CategoryDonut class="narrow" />
    </div>
    <NetWorthTrend />
  </div>
</template>

<style scoped>
.dashboard {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.row {
  display: grid;
  grid-template-columns: 1.4fr 1fr;
  gap: 1rem;
}

@media (max-width: 880px) {
  .row {
    grid-template-columns: 1fr;
  }
}
</style>
