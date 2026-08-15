<script setup lang="ts">
/**
 * 分析页 - 性能视图（设计 7.7）：代理覆盖数据卡片 + 未启用覆盖提示。
 * 数据全部来自共享 store 的 statisticsSummary（父级 onMounted 拉取），零 props。
 */
import { computed } from 'vue'
import { Gauge, ShieldCheck, Timer } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { useDesktopNavigationStore } from '../../stores/desktopNavigation'
import { t } from '../../../i18n'
import { formatDurationMs, formatRate, formatRequestCount } from '../../../utils/format'

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

const performance = computed(() => store.statisticsSummary?.performance ?? null)
const statusBreakdown = computed(() => store.statisticsSummary?.status ?? null)
/** 仅当 store 有代理覆盖数据时渲染（StatisticsSummary.performance 为可选字段）。 */
const hasPerformance = computed(
  () => !!store.statisticsSummary?.capability.hasPerformance && !!performance.value && performance.value.requestCount > 0
)
/** 模板安全取值（模板表达式不支持非空断言；渲染前提是 hasPerformance）。 */
const perf = computed(() => ({
  avgTtftMs: performance.value?.avgTtftMs ?? 0,
  avgTokensPerSecond: performance.value?.avgTokensPerSecond ?? 0,
  slowestModel: performance.value?.slowestModel ?? null,
  fastestModel: performance.value?.fastestModel ?? null
}))
</script>

<template>
  <div class="flex flex-col gap-5">
    <!-- 仅当 store 有代理覆盖数据时渲染；否则整体覆盖提示（设计 7.7） -->
    <div v-if="hasPerformance" class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
      <div class="mb-3 flex items-center justify-between gap-2">
        <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
          <Gauge :size="14" class="shrink-0" aria-hidden="true" />
          {{ t(locale, 'desktop.analytics.performanceTitle') }}
        </h3>
        <span class="rounded-full border border-[var(--theme-border-default)] px-1.5 py-0.5 text-[10px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.analytics.performanceCoverage') }}
        </span>
      </div>
      <div class="grid grid-cols-2 gap-3 lg:grid-cols-3">
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-[10px] font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfSuccessRate') }}</p>
          <p class="mt-1 font-mono text-base font-bold text-[var(--theme-text-primary)]">
            {{ statusBreakdown?.successRate != null ? `${formatRate(statusBreakdown.successRate)}%` : '—' }}
          </p>
          <p class="mt-0.5 text-[10px] text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.analytics.perfCovered', { count: formatRequestCount(statusBreakdown ? statusBreakdown.successRequests + statusBreakdown.clientErrorRequests + statusBreakdown.serverErrorRequests : 0) }) }}
          </p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-[10px] font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfAvgTtft') }}</p>
          <p class="mt-1 font-mono text-base font-bold text-[var(--theme-text-primary)]">{{ formatDurationMs(perf.avgTtftMs) }}</p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-[10px] font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfAvgRate') }}</p>
          <p class="mt-1 font-mono text-base font-bold text-[var(--theme-text-primary)]">{{ formatRate(perf.avgTokensPerSecond) }} t/s</p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-[10px] font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfSlowestModel') }}</p>
          <p class="mt-1 truncate text-sm font-semibold text-[var(--theme-text-primary)]">{{ perf.slowestModel || '—' }}</p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-[10px] font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfFastestModel') }}</p>
          <p class="mt-1 truncate text-sm font-semibold text-[var(--theme-text-primary)]">{{ perf.fastestModel || '—' }}</p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-[10px] font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfSlowRequests') }}</p>
          <p class="mt-1 font-mono text-base font-bold text-[var(--theme-text-primary)]">—</p>
          <p class="mt-0.5 text-[10px] text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.analytics.perfSlowHint') }}</p>
        </div>
      </div>
    </div>

    <div v-else class="flex flex-col items-center gap-2 rounded-lg border border-dashed border-[var(--theme-border-strong)] px-6 py-12 text-center">
      <ShieldCheck :size="20" class="text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <p class="text-sm font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.analytics.performanceNotAvailable') }}</p>
      <p class="max-w-md text-xs leading-5 text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.performanceNotAvailableDesc') }}</p>
      <button
        type="button"
        class="mt-1 flex items-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3 py-1.5 text-xs font-semibold text-[var(--theme-accent-contrast)] transition-opacity duration-150 hover:opacity-90"
        @click="nav.navigate('settings')"
      >
        {{ t(locale, 'desktop.manageDataSources') }}
      </button>
    </div>

    <!-- 性能视图顶部附加说明：即使有数据也始终显示覆盖提示（设计 7.7） -->
    <p v-if="hasPerformance" class="flex items-center gap-1.5 text-[11px] text-[var(--theme-text-tertiary)]">
      <Timer :size="12" class="shrink-0" aria-hidden="true" />
      {{ t(locale, 'desktop.analytics.performanceCoverageNote') }}
    </p>
  </div>
</template>
