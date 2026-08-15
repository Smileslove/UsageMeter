<script setup lang="ts">
/**
 * 概览页 KPI 条带（设计 6.4）：一条连续表面 6 个指标，固定高度骨架保证布局不跳动。
 * 数据由父页面经 useOverviewKpis 组装后传入；点击卡片切换趋势主指标。
 */
import type { PropType } from 'vue'
import { t } from '../../../i18n'
import type { KpiItem } from '../../composables/useOverviewKpis'
import type { StatisticsMetric } from '../../../types'

defineProps({
  kpis: { type: Array as PropType<KpiItem[]>, required: true },
  activeMetric: { type: String as PropType<StatisticsMetric>, required: true },
  locale: { type: String, required: true }
})

const emit = defineEmits<{
  select: [metric: StatisticsMetric]
}>()
</script>

<template>
  <div
    class="overflow-hidden rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-border-default)]"
    role="group"
    :aria-label="t(locale, 'desktop.overview.kpiBand')"
  >
    <div v-if="kpis.length" class="grid grid-cols-2 gap-px md:grid-cols-3 2xl:grid-cols-6">
      <button
        v-for="kpi in kpis"
        :key="kpi.key"
        type="button"
        class="flex h-[112px] flex-col items-start justify-between bg-[var(--theme-bg-elevated)] p-4 text-left transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-ring-focus)]"
        :aria-pressed="activeMetric === kpi.metric"
        :title="t(locale, 'desktop.overview.kpiSwitchHint')"
        @click="emit('select', kpi.metric)"
      >
        <span class="flex w-full items-center justify-between gap-1.5">
          <span class="flex min-w-0 items-center gap-1.5 text-xs font-medium text-[var(--theme-text-tertiary)]">
            <component :is="kpi.icon" :size="14" class="shrink-0" aria-hidden="true" />
            <span class="truncate">{{ t(locale, kpi.labelKey) }}</span>
          </span>
          <span
            v-if="kpi.coverageTag"
            class="shrink-0 rounded-full border border-[var(--theme-status-warning-border)] bg-[var(--theme-status-warning-bg)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--theme-status-warning-fg)]"
          >
            {{ t(locale, 'desktop.overview.kpiCoverageOnly') }}
          </span>
        </span>
        <span class="w-full truncate font-mono text-[20px] font-bold leading-7 text-[var(--theme-text-primary)]">
          {{ kpi.primary }}
        </span>
        <span
          class="w-full truncate text-xs text-[var(--theme-text-tertiary)]"
          :title="kpi.secondaryTitle"
        >
          {{ kpi.secondary }}
        </span>
      </button>
    </div>
    <div v-else class="grid grid-cols-2 gap-px md:grid-cols-3 2xl:grid-cols-6">
      <div v-for="i in 6" :key="i" class="h-[112px] animate-pulse bg-[var(--theme-bg-elevated)] p-4">
        <div class="h-3 w-16 rounded bg-[var(--theme-text-primary)]/10"></div>
        <div class="mt-4 h-5 w-20 rounded bg-[var(--theme-text-primary)]/10"></div>
        <div class="mt-2 h-3 w-24 rounded bg-[var(--theme-text-primary)]/10"></div>
      </div>
    </div>
  </div>
</template>
