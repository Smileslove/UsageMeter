<script setup lang="ts">
import SummaryPanel from '../components/SummaryPanel.vue'
import OverviewBreakdown from '../components/overview/OverviewBreakdown.vue'
</script>

<template>
  <div class="space-y-2 animate-in fade-in zoom-in-95 duration-300 pb-1">
    <!-- 顶部汇总面板 -->
    <SummaryPanel />

    <!-- 当前窗口归因排行 -->
    <OverviewBreakdown />
  </div>
</template>
