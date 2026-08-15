<script setup lang="ts">
/**
 * 分析页 - 趋势视图（设计 7.4）：总量摘要条 + 主趋势图（当前周期 + 对比周期）+ 峰值表。
 * 数据拉取编排（trendQuery / fetchTrend / prevSummary）由父级 DesktopAnalytics 负责，
 * 本组件 props 接收趋势点与上一周期点，内部只做渲染派生（图表 option / 摘要 / 峰值表）。
 */
import { computed, ref } from 'vue'
import VChart from 'vue-echarts'
import { BarChart3, RefreshCw, Table2, Zap } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { useDesktopNavigationStore } from '../../stores/desktopNavigation'
import { useDesktopAnalyticsStore, type AnalyticsCompare } from '../../stores/desktopAnalytics'
import {
  buildTrendChartOption,
  formatMetric,
  metricColor,
  metricLabel,
  metricValue,
  registerChartComponents,
  trendLineSeries,
  useChartTheme
} from '../../composables/useTrendChart'
import { t } from '../../../i18n'
import {
  formatCost,
  formatRate,
  formatRequestCount,
  formatTokenPair,
  formatTokenValue
} from '../../../utils/format'
import type { StatisticsTrendPoint } from '../../../types'

registerChartComponents()

const props = defineProps<{
  /** 当前周期趋势点（父级从 store.statisticsSummary 派生）。 */
  points: StatisticsTrendPoint[]
  /** 上一等长周期趋势点（比较模式开启时由父级 fetchTrend 填充，否则空数组）。 */
  prevPoints: StatisticsTrendPoint[]
  /** 比较模式（决定是否渲染对比序列）。 */
  compare: AnalyticsCompare
  /** 趋势摘要加载中（来自 store.statisticsLoading，父级透传）。 */
  statisticsLoading: boolean
}>()

const emit = defineEmits<{
  /** 趋势无数据时点击重试，由父级重新执行 fetchTrend。 */
  (e: 'retry'): void
}>()

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

/** 主题颜色：touch theme 设置触发重算（同 StatisticsTrendChart 的做法，实现在 useTrendChart）。 */
const chartTheme = useChartTheme()

function changePercent(cur: number, prev: number): number | null {
  if (!prev) return null
  return ((cur - prev) / prev) * 100
}

const trendChartOptions = computed(() => {
  const colors = chartTheme.value
  const points = props.points
  const prev = props.prevPoints
  const hasPrev = prev.length > 0 && props.compare === 'previous'
  const primaryColor = metricColor(analytics.analyticsMetric)
  // 同时显示当前周期与对比周期的精确值和变化率（设计 7.4，页面特有 tooltip）
  const tooltipFormatter = (params: any) => {
    const cur = points[params[0].dataIndex]
    if (!cur) return ''
    const prevPoint = hasPrev ? prev[params[0].dataIndex] : undefined
    const curValue = metricValue(cur, analytics.analyticsMetric)
    const prevValue = prevPoint ? metricValue(prevPoint, analytics.analyticsMetric) : null
    const change = prevValue != null ? changePercent(curValue, prevValue) : null
    const changeRow =
      change != null
        ? `<div style="margin-top:3px;"><span>${t(locale.value, 'desktop.analytics.trendChange')}: <b>${changeText(change)}</b></span></div>`
        : ''
    const prevRow =
      prevValue != null
        ? `<div style="margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${colors.series3};margin-right:6px;"></span><span>${t(locale.value, 'desktop.analytics.trendPrevious')}: <b>${formatMetric(analytics.analyticsMetric, prevValue)}</b></span></div>`
        : ''
    return `<div style="font-weight:600;margin-bottom:4px;">${cur.label}</div><div style="display:flex;align-items:center;gap:6px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${primaryColor};"></span><span>${t(locale.value, 'desktop.analytics.trendCurrent')}: <b>${formatMetric(analytics.analyticsMetric, curValue)}</b></span></div>${prevRow}${changeRow}`
  }
  // 当前周期主序列（2.5 宽渐变面积）+ 上一周期对比序列（1.8 宽虚线，设计 7.4）
  const series = [
    trendLineSeries({
      name: t(locale.value, 'desktop.analytics.trendCurrent'),
      data: points.map(p => metricValue(p, analytics.analyticsMetric)),
      color: primaryColor,
      primary: true
    }),
    ...(hasPrev
      ? [
          trendLineSeries({
            name: t(locale.value, 'desktop.analytics.trendPrevious'),
            data: prev.map(p => metricValue(p, analytics.analyticsMetric)),
            color: colors.series3,
            width: 1.8,
            dashed: true
          })
        ]
      : [])
  ]
  return buildTrendChartOption({
    points,
    metric: analytics.analyticsMetric,
    colors,
    tooltipFormatter,
    series
  })
})

// ---- 总量摘要条 ----

interface TotalItem {
  key: string
  labelKey: string
  value: string
}

const totalItems = computed<TotalItem[]>(() => {
  const totals = store.statisticsSummary?.totals
  if (!totals) return []
  const pair = formatTokenPair(totals.inputTokens, totals.outputTokens)
  return [
    { key: 'requests', labelKey: 'desktop.analytics.totalsRequests', value: formatRequestCount(totals.requestCount) },
    { key: 'tokens', labelKey: 'desktop.analytics.totalsTokens', value: formatTokenValue(totals.totalTokens) },
    { key: 'cost', labelKey: 'desktop.analytics.totalsCost', value: formatCost(totals.cost, store.settings.currency) },
    { key: 'input', labelKey: 'desktop.analytics.totalsInput', value: pair.input },
    { key: 'output', labelKey: 'desktop.analytics.totalsOutput', value: pair.output },
    { key: 'models', labelKey: 'desktop.analytics.totalsModels', value: String(totals.modelCount) }
  ]
})

// ---- 峰值表（设计 7.4） ----

interface PeakRow {
  point: StatisticsTrendPoint
  changePct: number | null
}

const peakRows = computed<PeakRow[]>(() => {
  const metric = analytics.analyticsMetric
  const prev = props.prevPoints
  return props.points
    .map((point, index) => ({
      point,
      changePct: prev[index] ? changePercent(metricValue(point, metric), metricValue(prev[index], metric)) : null
    }))
    .sort((a, b) => metricValue(b.point, metric) - metricValue(a.point, metric))
})

function changeText(pct: number | null): string {
  if (pct == null) return '—'
  const sign = pct > 0 ? '+' : ''
  return `${sign}${formatRate(pct)}%`
}

const showAllTable = ref(false)
const visiblePeakRows = computed(() => (showAllTable.value ? peakRows.value : peakRows.value.slice(0, 10)))
</script>

<template>
  <div class="flex flex-col gap-5">
    <!-- 总量摘要条 -->
    <div
      v-if="totalItems.length"
      class="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-border-default)] sm:grid-cols-3 lg:grid-cols-6"
    >
      <div v-for="item in totalItems" :key="item.key" class="bg-[var(--theme-bg-elevated)] px-4 py-3">
        <p class="text-[10px] font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, item.labelKey) }}</p>
        <p class="mt-0.5 truncate font-mono text-sm font-bold text-[var(--theme-text-primary)]">{{ item.value }}</p>
      </div>
    </div>

    <!-- 主趋势（当前实线+面积，对比虚线）+ 数据表开关 -->
    <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
      <div class="mb-3 flex items-center justify-between gap-2">
        <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
          <BarChart3 :size="14" class="shrink-0" aria-hidden="true" />
          {{ metricLabel(analytics.analyticsMetric) }}
          <span v-if="compare === 'previous'" class="text-[10px] font-normal text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.analytics.trendPrevHint') }}
          </span>
        </h3>
        <button
          v-if="peakRows.length"
          type="button"
          class="flex items-center gap-1 rounded-md px-2 py-1 text-xs font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:bg-[var(--theme-accent-soft)]"
          :aria-pressed="showAllTable"
          @click="showAllTable = !showAllTable"
        >
          <Table2 :size="13" aria-hidden="true" />
          {{ t(locale, showAllTable ? 'desktop.analytics.hideTable' : 'desktop.analytics.showTable') }}
        </button>
      </div>

      <div v-if="points.length" class="h-[260px]">
        <v-chart class="h-full w-full" :option="trendChartOptions" autoresize />
      </div>
      <div
        v-else-if="statisticsLoading"
        class="h-[260px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"
        :aria-label="t(locale, 'common.syncing')"
      ></div>
      <div
        v-else
        class="grid h-[260px] place-items-center gap-2 rounded-lg border border-dashed border-[var(--theme-border-strong)] text-xs text-[var(--theme-text-tertiary)]"
      >
        <span>{{ t(locale, 'desktop.analytics.trendNoData') }}</span>
        <button
          type="button"
          class="flex items-center gap-1 rounded-lg bg-[var(--theme-accent-primary)] px-2.5 py-1 text-xs font-semibold text-[var(--theme-accent-contrast)] transition-opacity duration-150 hover:opacity-90"
          @click="emit('retry')"
        >
          <RefreshCw :size="12" aria-hidden="true" />
          {{ t(locale, 'desktop.overview.errorRetry') }}
        </button>
      </div>
    </div>

    <!-- 峰值表（默认按主指标降序；点击动作进入会话页） -->
    <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
      <h3 class="mb-2 flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
        <Zap :size="14" class="shrink-0" aria-hidden="true" />
        {{ t(locale, 'desktop.analytics.peakTable') }}
      </h3>
      <div v-if="visiblePeakRows.length" class="overflow-x-auto">
        <table class="w-full min-w-[600px] text-xs">
          <thead>
            <tr class="border-b border-[var(--theme-border-subtle)] text-left text-[10px] font-medium uppercase tracking-wide text-[var(--theme-text-quaternary)]">
              <th class="py-2 pr-3 font-medium">{{ t(locale, 'desktop.analytics.peakTime') }}</th>
              <th class="py-2 pr-3 font-medium">{{ metricLabel(analytics.analyticsMetric) }}</th>
              <th class="py-2 pr-3 text-right font-medium">{{ t(locale, 'desktop.analytics.peakRequests') }}</th>
              <th class="py-2 pr-3 text-right font-medium">{{ t(locale, 'desktop.analytics.peakTokens') }}</th>
              <th class="py-2 pr-3 text-right font-medium">{{ t(locale, 'desktop.analytics.peakCost') }}</th>
              <th class="py-2 pr-3 text-right font-medium">{{ t(locale, 'desktop.analytics.peakChange') }}</th>
              <th class="py-2 font-medium">{{ t(locale, 'desktop.analytics.peakAction') }}</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-[var(--theme-border-subtle)]">
            <tr v-for="row in visiblePeakRows" :key="row.point.startEpoch" class="transition-colors duration-150 hover:bg-[var(--theme-bg-hover)]">
              <td class="py-2 pr-3 font-mono text-[var(--theme-text-secondary)]">{{ row.point.label }}</td>
              <td class="py-2 pr-3 font-mono font-semibold text-[var(--theme-text-primary)]">
                {{ formatMetric(analytics.analyticsMetric, metricValue(row.point, analytics.analyticsMetric)) }}
              </td>
              <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatRequestCount(row.point.requestCount) }}</td>
              <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatTokenValue(row.point.totalTokens) }}</td>
              <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatCost(row.point.cost, store.settings.currency) }}</td>
              <td
                class="py-2 pr-3 text-right font-mono"
                :class="
                  row.changePct == null
                    ? 'text-[var(--theme-text-quaternary)]'
                    : row.changePct >= 0
                      ? 'text-emerald-600 dark:text-emerald-400'
                      : 'text-red-600 dark:text-red-400'
                "
              >
                {{ changeText(row.changePct) }}
              </td>
              <td class="py-2">
                <button
                  type="button"
                  class="rounded-md px-2 py-1 text-[11px] font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:bg-[var(--theme-accent-soft)]"
                  @click="nav.navigate('sessions')"
                >
                  {{ t(locale, 'desktop.analytics.viewSessions') }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-else class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center text-xs text-[var(--theme-text-tertiary)]">
        {{ t(locale, 'desktop.analytics.peakEmpty') }}
      </div>
    </div>
  </div>
</template>
