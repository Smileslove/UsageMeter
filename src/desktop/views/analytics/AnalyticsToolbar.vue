<script setup lang="ts">
/**
 * 分析页 - 工具栏（设计 7.3）：时间范围 / 粒度 / 主指标 / 比较 / 导出占位。
 * 与页面现有模式一致：直接读写 desktopAnalytics store（analyticsWindow /
 * analyticsGranularity / analyticsMetric / analyticsCompare），无 props / emits。
 */
import { computed } from 'vue'
import { FileDown } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import {
  useDesktopAnalyticsStore,
  type AnalyticsCompare,
  type AnalyticsGranularity
} from '../../stores/desktopAnalytics'
import { metricColor } from '../../composables/useTrendChart'
import { t, windowNameLabel } from '../../../i18n'
import { METRICS } from '../../../components/statistics/activityUtils'
import { WINDOW_ORDER } from '../../../types'

const store = useMonitorStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

const granularityOptions: Array<{ value: AnalyticsGranularity; key: string }> = [
  { value: 'auto', key: 'desktop.analytics.granularityAuto' },
  { value: 'hour', key: 'desktop.analytics.granularityHour' },
  { value: 'day', key: 'desktop.analytics.granularityDay' }
]

const compareOptions: Array<{ value: AnalyticsCompare; key: string }> = [
  { value: 'none', key: 'desktop.analytics.compareNone' },
  { value: 'previous', key: 'desktop.analytics.comparePrevious' }
]

/** 不合法粒度组合：30d/本月 不提供小时粒度，5h/24h/当天 不提供天粒度（设计 7.3）。 */
function granularityDisabled(g: AnalyticsGranularity): boolean {
  if (g === 'hour') {
    return analytics.analyticsWindow === '30d' || analytics.analyticsWindow === 'current_month'
  }
  if (g === 'day') {
    return (
      analytics.analyticsWindow === '5h' ||
      analytics.analyticsWindow === '24h' ||
      analytics.analyticsWindow === 'today'
    )
  }
  return false
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-2">
    <!-- 时间范围：页面内部工具栏读写 analyticsWindow -->
    <div
      class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="group"
      :aria-label="t(locale, 'desktop.analytics.toolbarRange')"
    >
      <button
        v-for="w in WINDOW_ORDER"
        :key="w"
        type="button"
        class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
        :class="
          analytics.analyticsWindow === w
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        :aria-pressed="analytics.analyticsWindow === w"
        @click="analytics.analyticsWindow = w"
      >
        {{ windowNameLabel(locale, w) }}
      </button>
    </div>

    <!-- 粒度：自动 / 小时 / 天；不合法组合禁用并说明 -->
    <div
      class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="group"
      :aria-label="t(locale, 'desktop.analytics.toolbarGranularity')"
    >
      <button
        v-for="g in granularityOptions"
        :key="g.value"
        type="button"
        class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-40"
        :class="
          analytics.analyticsGranularity === g.value
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        :disabled="granularityDisabled(g.value)"
        :title="granularityDisabled(g.value) ? t(locale, 'desktop.analytics.granularityInvalid') : undefined"
        :aria-pressed="analytics.analyticsGranularity === g.value"
        @click="analytics.analyticsGranularity = g.value"
      >
        {{ t(locale, g.key) }}
      </button>
    </div>

    <!-- 主指标：费用 / 请求 / Token -->
    <div
      class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="group"
      :aria-label="t(locale, 'desktop.analytics.toolbarMetric')"
    >
      <button
        v-for="m in METRICS"
        :key="m.value"
        type="button"
        class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
        :class="
          analytics.analyticsMetric === m.value
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        :aria-pressed="analytics.analyticsMetric === m.value"
        @click="analytics.analyticsMetric = m.value"
      >
        <span
          class="h-1.5 w-1.5 rounded-full"
          :style="{ backgroundColor: metricColor(m.value) }"
        ></span>
        {{ t(locale, m.key) }}
      </button>
    </div>

    <!-- 比较：无 / 上一等长周期 -->
    <div
      class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="group"
      :aria-label="t(locale, 'desktop.analytics.toolbarCompare')"
    >
      <button
        v-for="c in compareOptions"
        :key="c.value"
        type="button"
        class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
        :class="
          analytics.analyticsCompare === c.value
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        :aria-pressed="analytics.analyticsCompare === c.value"
        @click="analytics.analyticsCompare = c.value"
      >
        {{ t(locale, c.key) }}
      </button>
    </div>

    <!-- 导出（占位：CSV/PNG 将在后续版本提供） -->
    <button
      type="button"
      class="flex h-7 w-7 items-center justify-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
      :title="t(locale, 'desktop.analytics.exportPlaceholder')"
      :aria-label="t(locale, 'desktop.analytics.exportPlaceholder')"
      aria-disabled="true"
    >
      <FileDown :size="15" aria-hidden="true" />
    </button>
  </div>
</template>
