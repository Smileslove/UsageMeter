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
import type { StatisticsMetric, StatisticsTotals, StatisticsTrendPoint } from '../../../types'

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

// ---- 主趋势子序列切换（设计 7.4：主指标之外可下钻到 Token 细分口径） ----

type TrendSubSeries = 'total' | 'input' | 'output' | 'cacheCreate' | 'cacheRead'
const subSeries = ref<TrendSubSeries>('total')
const subSeriesOptions: { value: TrendSubSeries; labelKey: string }[] = [
  { value: 'total', labelKey: 'desktop.analytics.trendSubSeries.subTotal' },
  { value: 'input', labelKey: 'desktop.analytics.trendSubSeries.subInput' },
  { value: 'output', labelKey: 'desktop.analytics.trendSubSeries.subOutput' },
  { value: 'cacheCreate', labelKey: 'desktop.analytics.trendSubSeries.subCacheCreate' },
  { value: 'cacheRead', labelKey: 'desktop.analytics.trendSubSeries.subCacheRead' }
]

/** 子序列取值：total 取 totalTokens，其余取对应 Token 细分字段（数据字段在 trendPoints 中已存在）。 */
function subSeriesValue(point: StatisticsTrendPoint, sub: TrendSubSeries): number {
  switch (sub) {
    case 'input':
      return point.inputTokens
    case 'output':
      return point.outputTokens
    case 'cacheCreate':
      return point.cacheCreateTokens
    case 'cacheRead':
      return point.cacheReadTokens
    default:
      return point.totalTokens
  }
}

const trendChartOptions = computed(() => {
  const colors = chartTheme.value
  const points = props.points
  const prev = props.prevPoints
  const hasPrev = prev.length > 0 && props.compare === 'previous'
  const primaryColor = metricColor(analytics.analyticsMetric)
  // 非 total 子序列时图表按 Token 口径展示：buildTrendChartOption 的 metric 仅用于 yAxis 标签格式化
  const chartMetric: StatisticsMetric = subSeries.value === 'total' ? analytics.analyticsMetric : 'tokens'
  const fmtValue = (value: number): string =>
    subSeries.value === 'total' ? formatMetric(analytics.analyticsMetric, value) : formatTokenValue(value)
  // 同时显示当前周期与对比周期的精确值和变化率（设计 7.4，页面特有 tooltip）
  const tooltipFormatter = (params: any) => {
    const cur = points[params[0].dataIndex]
    if (!cur) return ''
    const prevPoint = hasPrev ? prev[params[0].dataIndex] : undefined
    const curValue = subSeriesValue(cur, subSeries.value)
    const prevValue = prevPoint ? subSeriesValue(prevPoint, subSeries.value) : null
    const change = prevValue != null ? changePercent(curValue, prevValue) : null
    const changeRow =
      change != null
        ? `<div style="margin-top:3px;"><span>${t(locale.value, 'desktop.analytics.trendChange')}: <b>${changeText(change)}</b></span></div>`
        : ''
    const prevRow =
      prevValue != null
        ? `<div style="margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${colors.series3};margin-right:6px;"></span><span>${t(locale.value, 'desktop.analytics.trendPrevious')}: <b>${fmtValue(prevValue)}</b></span></div>`
        : ''
    return `<div style="font-weight:600;margin-bottom:4px;">${cur.label}</div><div style="display:flex;align-items:center;gap:6px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${primaryColor};"></span><span>${t(locale.value, 'desktop.analytics.trendCurrent')}: <b>${fmtValue(curValue)}</b></span></div>${prevRow}${changeRow}`
  }
  // 当前周期主序列（2.5 宽渐变面积）+ 上一周期对比序列（1.8 宽虚线，设计 7.4）
  const series = [
    trendLineSeries({
      name: t(locale.value, 'desktop.analytics.trendCurrent'),
      data: points.map(p => subSeriesValue(p, subSeries.value)),
      color: primaryColor,
      primary: true
    }),
    ...(hasPrev
      ? [
          trendLineSeries({
            name: t(locale.value, 'desktop.analytics.trendPrevious'),
            data: prev.map(p => subSeriesValue(p, subSeries.value)),
            color: colors.series3,
            width: 1.8,
            dashed: true
          })
        ]
      : [])
  ]
  return buildTrendChartOption({
    points,
    metric: chartMetric,
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

/**
 * 总量摘要按模型筛选（设计 7.3）：模型过滤生效时对选中模型的明细求和；
 * 后端未返回模型明细时退回全量（口径受限，见筛选摘要说明）。
 * 注意：StatisticsModelBreakdown 无代理/本地请求拆分，proxyRequestCount 保持 0。
 */
const filteredTotals = computed<StatisticsTotals | null>(() => {
  const summary = store.statisticsSummary
  if (!summary) return null
  const models = analytics.analyticsModelFilter
  if (models.length === 0 || !summary.models || summary.models.length === 0) return summary.totals
  const selected = new Set(models)
  const acc: StatisticsTotals = {
    requestCount: 0,
    totalTokens: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheCreateTokens: 0,
    cacheReadTokens: 0,
    cost: 0,
    modelCount: 0,
    localRequestCount: 0,
    proxyRequestCount: 0,
    successRequests: 0,
    errorRequests: 0
  }
  for (const m of summary.models) {
    if (!selected.has(m.modelName)) continue
    acc.requestCount += m.requestCount
    acc.totalTokens += m.totalTokens
    acc.inputTokens += m.inputTokens
    acc.outputTokens += m.outputTokens
    acc.cacheCreateTokens += m.cacheCreateTokens
    acc.cacheReadTokens += m.cacheReadTokens
    acc.cost += m.cost
    acc.modelCount += 1
    acc.localRequestCount += m.localRequestCount
    acc.successRequests = (acc.successRequests ?? 0) + (m.successRequests ?? 0)
    acc.errorRequests = (acc.errorRequests ?? 0) + (m.errorRequests ?? 0)
  }
  return acc
})

const totalItems = computed<TotalItem[]>(() => {
  const totals = filteredTotals.value
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

/** 峰值表下钻（验收 21.1-7）：携带该峰值时间桶（秒级半开区间，与统计窗口/会话时间口径一致）跳转会话页。
 *  桶宽优先用相邻趋势点 startEpoch 差值推导（DST 时区日桶为 82800/90000 秒，固定 86400 会错位 1 小时）。 */
function drillToPeak(row: PeakRow): void {
  const bucket = store.statisticsSummary?.range.bucket ?? 'hour'
  let span = bucket === 'day' ? 86400 : 3600
  const point = row.point
  const idx = props.points.findIndex(p => p.startEpoch === point.startEpoch)
  const next = idx >= 0 ? props.points[idx + 1] : undefined
  if (next && next.startEpoch > point.startEpoch) {
    span = next.startEpoch - point.startEpoch
  }
  nav.applyNavigationTarget({
    page: 'sessions',
    timeRange: { startEpoch: point.startEpoch, endEpoch: point.startEpoch + span }
  })
}
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
        <div class="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-2">
          <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
            <BarChart3 :size="14" class="shrink-0" aria-hidden="true" />
            {{ metricLabel(analytics.analyticsMetric) }}
            <span v-if="compare === 'previous'" class="text-[10px] font-normal text-[var(--theme-text-quaternary)]">
              {{ t(locale, 'desktop.analytics.trendPrevHint') }}
            </span>
          </h3>
          <!-- 子序列切换（设计 7.4）：主指标之外可查看 Token 细分口径；total 跟随主指标 -->
          <div v-if="points.length" class="trend-seg" role="tablist" :aria-label="t(locale, 'desktop.analytics.tabsMetric')">
            <button
              v-for="option in subSeriesOptions"
              :key="option.value"
              type="button"
              class="trend-seg__item"
              :class="{ 'trend-seg__item--on': subSeries === option.value }"
              :aria-pressed="subSeries === option.value"
              @click="subSeries = option.value"
            >
              <span class="trend-seg__label">{{ t(locale, option.labelKey) }}</span>
            </button>
          </div>
        </div>
        <button
          v-if="peakRows.length"
          type="button"
          class="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-xs font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:bg-[var(--theme-accent-soft)]"
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
                  @click="drillToPeak(row)"
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
