<script setup lang="ts">
/**
 * 分析页 - 性能视图（设计 7.7）：代理覆盖统计卡 + 请求级性能分析
 * （TTFT/耗时 P50/P95 分位、慢请求计数、TTFT 与生成速率双时序图、
 *   状态码堆叠条、最慢模型/来源排行、最慢请求明细表）。
 *
 * 数据来源：
 * - 统计卡：共享 store 的 statisticsSummary（父级 DesktopAnalytics onMounted 拉取），零 props。
 * - 请求级分析：本组件独立分页拉取（后端单次 limit 硬上限 30，循环取至多 300 条），
 *   结果存本地 ref 不写回共享 store——避免污染「请求」页的 30 条分页缓存语义；
 *   样本范围（最近 N 条）在标题注明。
 *
 * 抽屉说明：请求明细抽屉（selectedRequest/requestDrawerOpen）是 DesktopRequests.vue
 * 的组件内局部状态，未暴露为共享 store 或全局事件，本视图无法直接打开；
 * 因此行点击仅做选中态高亮，并提供跳转「请求」页按钮查看完整详情。
 */
import { computed, onMounted, ref } from 'vue'
import { Activity, ArrowRight, BarChart3, Gauge, Globe, Info, ShieldCheck, Table2, Timer } from 'lucide-vue-next'
import VChart from 'vue-echarts'
import { useMonitorStore } from '../../../stores/monitor'
import { useDesktopNavigationStore } from '../../stores/desktopNavigation'
import { useSessionDisplay } from '../../../composables/useSessionDisplay'
import { registerChartComponents, trendLineSeries, useChartTheme } from '../../composables/useTrendChart'
import { usePerformanceAvailability } from '../../composables/usePerformanceAvailability'
import { queryRecentRequestRecords } from '../../../stores/sessionQueries'
import { t } from '../../../i18n'
import { formatDurationMs, formatRate, formatRequestCount } from '../../../utils/format'
import type { RequestRecord } from '../../../types'

registerChartComponents()

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)
const display = useSessionDisplay(store)

const { performance, hasPerformance } = usePerformanceAvailability(store)
const statusBreakdown = computed(() => store.statisticsSummary?.status ?? null)
/** 模板安全取值（模板表达式不支持非空断言；渲染前提是 hasPerformance）。 */
const perf = computed(() => ({
  avgTtftMs: performance.value?.avgTtftMs ?? 0,
  avgTokensPerSecond: performance.value?.avgTokensPerSecond ?? 0,
  slowestModel: performance.value?.slowestModel ?? null,
  fastestModel: performance.value?.fastestModel ?? null
}))

// ---- 请求级数据：本地分页拉取（后端单次 limit 上限 30，见 requests.rs RECENT_REQUESTS_MAX_LIMIT） ----

/** 性能视图样本上限（10 页 × 30 条）。 */
const RECORD_MAX = 300
const RECORD_PAGE = 30
const records = ref<RequestRecord[]>([])
const recordsLoading = ref(false)

async function loadRecords() {
  if (recordsLoading.value) return
  recordsLoading.value = true
  try {
    const fetched: RequestRecord[] = []
    for (let offset = 0; offset < RECORD_MAX; offset += RECORD_PAGE) {
      const batch = await queryRecentRequestRecords(store.settings, null, RECORD_PAGE, offset)
      fetched.push(...batch)
      if (batch.length < RECORD_PAGE) break
    }
    records.value = fetched
  } catch (error) {
    console.error('Failed to load performance request records:', error)
  } finally {
    recordsLoading.value = false
  }
}

onMounted(loadRecords)

// ---- 分位统计（A）：线性插值分位；慢请求 = 耗时 > P95 的条数 ----

function percentile(sorted: number[], p: number): number | null {
  if (sorted.length === 0) return null
  if (sorted.length === 1) return sorted[0]
  const idx = (sorted.length - 1) * p
  const lo = Math.floor(idx)
  const hi = Math.ceil(idx)
  return lo === hi ? sorted[lo] : sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo)
}

/** 有效样本（> 0 视为有性能数据），升序排列。 */
const ttftSamples = computed(() =>
  records.value.map(r => r.ttftMs).filter((v): v is number => !!v && v > 0).sort((a, b) => a - b)
)
const durationSamples = computed(() =>
  records.value.map(r => r.durationMs).filter((v): v is number => !!v && v > 0).sort((a, b) => a - b)
)

const ttftP50 = computed(() => percentile(ttftSamples.value, 0.5))
const ttftP95 = computed(() => percentile(ttftSamples.value, 0.95))
const durationP50 = computed(() => percentile(durationSamples.value, 0.5))
const durationP95 = computed(() => percentile(durationSamples.value, 0.95))
/** 慢请求数：耗时严格大于 P95 的条数；无样本时为 null（模板显示 —）。 */
const slowRequestsCount = computed(() => {
  const p95 = durationP95.value
  if (p95 == null) return null
  return durationSamples.value.filter(v => v > p95).length
})

// ---- 状态码分组（C）：2xx / 3xx / 4xx / 5xx ----

interface StatusGroup {
  key: string
  color: string
  count: number
}

const statusGroups = computed(() => {
  const groups: StatusGroup[] = [
    { key: '2xx', color: 'var(--theme-chart-requests)', count: 0 },
    { key: '3xx', color: 'var(--theme-chart-tokens)', count: 0 },
    { key: '4xx', color: 'var(--theme-chart-cost)', count: 0 },
    { key: '5xx', color: 'var(--theme-chart-series-3)', count: 0 }
  ]
  let total = 0
  for (const r of records.value) {
    const code = r.statusCode
    if (!code || code < 100 || code >= 600) continue
    total++
    const idx = code < 300 ? 0 : code < 400 ? 1 : code < 500 ? 2 : 3
    groups[idx].count++
  }
  return { groups, total }
})

// ---- 最慢模型/来源排行（D）：按平均耗时降序 top 5，带请求数 ----

interface RankRow {
  label: string
  avgMs: number
  count: number
}

function rankByDuration(pick: (r: RequestRecord) => { key: string; label: string } | null): RankRow[] {
  const acc = new Map<string, { label: string; total: number; count: number }>()
  for (const r of records.value) {
    const picked = pick(r)
    if (!picked || !r.durationMs || r.durationMs <= 0) continue
    const cur = acc.get(picked.key) ?? { label: picked.label, total: 0, count: 0 }
    cur.total += r.durationMs
    cur.count += 1
    acc.set(picked.key, cur)
  }
  return [...acc.entries()]
    .map(([, v]) => ({ label: v.label, avgMs: v.total / v.count, count: v.count }))
    .sort((a, b) => b.avgMs - a.avgMs)
    .slice(0, 5)
}

const slowestModels = computed(() =>
  rankByDuration(r => {
    const model = r.model?.trim()
    return model ? { key: model, label: display.requestModelLabel(r) } : null
  })
)

const slowestSources = computed(() =>
  rankByDuration(r => {
    const source = r.sourceLabel?.trim() || r.requestBaseUrl?.trim() || ''
    return source ? { key: source, label: source } : null
  })
)

// ---- TTFT 与生成速率双时序图（B）：上下两个独立坐标区，按时间正序 ----

const chartTheme = useChartTheme()

const sortedByTime = computed(() => [...records.value].sort((a, b) => a.timestampMs - b.timestampMs))

function chartTimeLabel(tsMs: number): string {
  const localeTag = locale.value.replace('_', '-')
  return new Date(tsMs).toLocaleTimeString(localeTag, { hour: '2-digit', minute: '2-digit' })
}

function chartFullLabel(tsMs: number): string {
  const localeTag = locale.value.replace('_', '-')
  return new Date(tsMs).toLocaleString(localeTag, {
    month: 'numeric',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

interface TimelinePoint {
  time: string
  full: string
  value: number
}

const ttftSeries = computed<TimelinePoint[]>(() => {
  const data: TimelinePoint[] = []
  for (const r of sortedByTime.value) {
    if (r.ttftMs && r.ttftMs > 0) data.push({ time: chartTimeLabel(r.timestampMs), full: chartFullLabel(r.timestampMs), value: r.ttftMs })
  }
  return data
})

const rateSeries = computed<TimelinePoint[]>(() => {
  const data: TimelinePoint[] = []
  for (const r of sortedByTime.value) {
    if (r.outputTokensPerSecond && r.outputTokensPerSecond > 0) {
      data.push({ time: chartTimeLabel(r.timestampMs), full: chartFullLabel(r.timestampMs), value: r.outputTokensPerSecond })
    }
  }
  return data
})

function timelineOption(
  data: TimelinePoint[],
  color: string,
  valueFormatter: (v: number) => string,
  seriesName: string
) {
  const colors = chartTheme.value
  return {
    grid: { left: 48, right: 12, top: 10, bottom: 22 },
    tooltip: {
      trigger: 'axis' as const,
      backgroundColor: colors.tooltipBg,
      borderColor: colors.tooltipBorder,
      borderRadius: 8,
      padding: [7, 9],
      textStyle: { color: colors.tooltipText, fontSize: 11 },
      formatter: (params: any) => {
        const point = params?.[0]
        if (!point || !data[point.dataIndex]) return ''
        const item = data[point.dataIndex]
        return `<div style="font-weight:600;margin-bottom:2px;">${item.full}</div><div style="display:flex;align-items:center;gap:6px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${color};"></span><span>${seriesName}: <b>${valueFormatter(item.value)}</b></span></div>`
      }
    },
    xAxis: {
      type: 'category' as const,
      data: data.map(d => d.time),
      boundaryGap: false,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: colors.axis, fontSize: 10, hideOverlap: true, margin: 8 }
    },
    yAxis: {
      type: 'value' as const,
      min: 0,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: colors.axis, fontSize: 10, formatter: (v: number) => valueFormatter(v) },
      splitLine: { lineStyle: { type: 'dashed' as const, color: colors.grid } }
    },
    series: [trendLineSeries({ name: seriesName, data: data.map(d => d.value), color, primary: true })]
  }
}

const ttftAxisFormatter = (v: number) => (v >= 1000 ? `${(v / 1000).toFixed(1)}s` : `${Math.round(v)}ms`)
const rateAxisFormatter = (v: number) => `${formatRate(v)}t/s`

const ttftChartOption = computed(() =>
  timelineOption(ttftSeries.value, chartTheme.value.requests, ttftAxisFormatter, t(locale.value, 'desktop.analytics.perfTtftChart'))
)
const rateChartOption = computed(() =>
  timelineOption(rateSeries.value, chartTheme.value.tokens, rateAxisFormatter, t(locale.value, 'desktop.analytics.perfRateChart'))
)

// ---- 最慢 20 条请求子表（E）：耗时降序，行点击选中态（抽屉为请求页局部状态，本视图不可达） ----

const slowestRequests = computed(() =>
  records.value
    .filter(r => !!r.durationMs && r.durationMs > 0)
    .sort((a, b) => (b.durationMs ?? 0) - (a.durationMs ?? 0))
    .slice(0, 20)
)

const selectedRequestKey = ref<string | null>(null)

function toggleRow(request: RequestRecord) {
  selectedRequestKey.value = selectedRequestKey.value === request.requestKey ? null : request.requestKey
}
</script>

<template>
  <div class="flex flex-col gap-5">
    <!-- 统计卡区（设计 7.7）：仅当 store 有代理覆盖数据时渲染 -->
    <div v-if="hasPerformance" class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
      <div class="mb-3 flex items-center justify-between gap-2">
        <h3 class="flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
          <Gauge :size="14" class="shrink-0" aria-hidden="true" />
          {{ t(locale, 'desktop.analytics.performanceTitle') }}
        </h3>
        <div class="flex items-center gap-2">
          <span v-if="records.length > 0" class="rounded-full border border-[var(--theme-border-default)] px-1.5 py-0.5 text-xs text-[var(--theme-text-tertiary)]">
            {{ t(locale, 'desktop.analytics.perfSampleScope', { count: formatRequestCount(records.length) }) }}
          </span>
          <span class="rounded-full border border-[var(--theme-border-default)] px-1.5 py-0.5 text-xs text-[var(--theme-text-tertiary)]">
            {{ t(locale, 'desktop.analytics.performanceCoverage') }}
          </span>
        </div>
      </div>
      <div class="grid grid-cols-2 gap-3 lg:grid-cols-3">
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfSuccessRate') }}</p>
          <p class="mt-1 font-mono text-base font-bold text-[var(--theme-text-primary)]">
            {{ statusBreakdown?.successRate != null ? `${formatRate(statusBreakdown.successRate)}%` : '—' }}
          </p>
          <p class="mt-0.5 text-xs text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.analytics.perfCovered', { count: formatRequestCount(statusBreakdown ? statusBreakdown.successRequests + statusBreakdown.clientErrorRequests + statusBreakdown.serverErrorRequests : 0) }) }}
          </p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfAvgTtft') }}</p>
          <p class="mt-1 font-mono text-base font-bold text-[var(--theme-text-primary)]">{{ formatDurationMs(perf.avgTtftMs) }}</p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfAvgRate') }}</p>
          <p class="mt-1 font-mono text-base font-bold text-[var(--theme-text-primary)]">{{ formatRate(perf.avgTokensPerSecond) }} t/s</p>
        </div>
        <!-- 分位卡（A）：TTFT P50 / P95，无样本显示 — -->
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfTtftPercentile') }}</p>
          <p class="mt-1 flex items-baseline justify-between font-mono text-[13px] font-bold text-[var(--theme-text-primary)]">
            <span class="text-xs font-medium text-[var(--theme-text-quaternary)]">P50</span>
            <span>{{ ttftP50 != null ? formatDurationMs(ttftP50) : '—' }}</span>
          </p>
          <p class="mt-0.5 flex items-baseline justify-between font-mono text-[13px] font-bold text-[var(--theme-text-primary)]">
            <span class="text-xs font-medium text-[var(--theme-text-quaternary)]">P95</span>
            <span>{{ ttftP95 != null ? formatDurationMs(ttftP95) : '—' }}</span>
          </p>
        </div>
        <!-- 分位卡（A）：耗时 P50 / P95 -->
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfDurationPercentile') }}</p>
          <p class="mt-1 flex items-baseline justify-between font-mono text-[13px] font-bold text-[var(--theme-text-primary)]">
            <span class="text-xs font-medium text-[var(--theme-text-quaternary)]">P50</span>
            <span>{{ durationP50 != null ? formatDurationMs(durationP50) : '—' }}</span>
          </p>
          <p class="mt-0.5 flex items-baseline justify-between font-mono text-[13px] font-bold text-[var(--theme-text-primary)]">
            <span class="text-xs font-medium text-[var(--theme-text-quaternary)]">P95</span>
            <span>{{ durationP95 != null ? formatDurationMs(durationP95) : '—' }}</span>
          </p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfSlowestModel') }}</p>
          <p class="mt-1 truncate text-sm font-semibold text-[var(--theme-text-primary)]">{{ perf.slowestModel || '—' }}</p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfFastestModel') }}</p>
          <p class="mt-1 truncate text-sm font-semibold text-[var(--theme-text-primary)]">{{ perf.fastestModel || '—' }}</p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-subtle)] px-4 py-3">
          <p class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.perfSlowRequests') }}</p>
          <p class="mt-1 font-mono text-base font-bold text-[var(--theme-text-primary)]">
            {{ slowRequestsCount != null ? slowRequestsCount : '—' }}
          </p>
          <p class="mt-0.5 text-xs text-[var(--theme-text-quaternary)]">
            {{ slowRequestsCount != null ? t(locale, 'desktop.analytics.perfSlowCountHint', { threshold: formatDurationMs(durationP95 ?? 0) }) : t(locale, 'desktop.analytics.perfSlowHint') }}
          </p>
        </div>
      </div>
    </div>

    <!-- 请求记录加载中（统计卡区不可见时） -->
    <div
      v-else-if="recordsLoading && records.length === 0"
      class="grid h-24 place-items-center rounded-lg border border-[var(--theme-border-subtle)] text-xs text-[var(--theme-text-tertiary)]"
    >
      <span class="animate-pulse">{{ t(locale, 'desktop.analytics.perfRecordsLoading') }}</span>
    </div>

    <!-- 兜底空态（复用现有未启用提示逻辑，设计 7.7） -->
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

    <!-- 请求级性能分析区块（有请求记录时渲染） -->
    <template v-if="records.length > 0">
      <!-- 双时序图（B）：TTFT 与生成速率，上下两个独立坐标区 -->
      <section class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
        <h3 class="mb-3 flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
          <Activity :size="14" class="shrink-0" aria-hidden="true" />
          {{ t(locale, 'desktop.analytics.perfTimelineTitle') }}
        </h3>
        <div class="flex flex-col gap-4">
          <div>
            <p class="mb-1.5 flex items-center gap-1.5 text-xs font-medium text-[var(--theme-text-tertiary)]">
              <span class="inline-block h-2 w-2 rounded-full" style="background: var(--theme-chart-requests)"></span>
              {{ t(locale, 'desktop.analytics.perfTtftChart') }}
            </p>
            <div v-if="ttftSeries.length > 1" class="h-[130px]">
              <v-chart class="h-full w-full" :option="ttftChartOption" autoresize />
            </div>
            <div
              v-else
              class="grid h-[130px] place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] text-xs text-[var(--theme-text-tertiary)]"
            >
              {{ t(locale, 'desktop.analytics.perfChartEmpty') }}
            </div>
          </div>
          <div>
            <p class="mb-1.5 flex items-center gap-1.5 text-xs font-medium text-[var(--theme-text-tertiary)]">
              <span class="inline-block h-2 w-2 rounded-full" style="background: var(--theme-chart-tokens)"></span>
              {{ t(locale, 'desktop.analytics.perfRateChart') }}
            </p>
            <div v-if="rateSeries.length > 1" class="h-[130px]">
              <v-chart class="h-full w-full" :option="rateChartOption" autoresize />
            </div>
            <div
              v-else
              class="grid h-[130px] place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] text-xs text-[var(--theme-text-tertiary)]"
            >
              {{ t(locale, 'desktop.analytics.perfChartEmpty') }}
            </div>
          </div>
        </div>
      </section>

      <!-- 状态码堆叠条（C）：2xx/3xx/4xx/5xx 纯 div 实现 -->
      <section class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
        <h3 class="mb-3 flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
          <BarChart3 :size="14" class="shrink-0" aria-hidden="true" />
          {{ t(locale, 'desktop.analytics.perfStatusBreakdown') }}
        </h3>
        <div v-if="statusGroups.total > 0">
          <div class="flex h-2.5 w-full overflow-hidden rounded-full bg-[var(--theme-border-subtle)]">
            <div
              v-for="group in statusGroups.groups"
              :key="group.key"
              v-show="group.count > 0"
              class="h-full transition-all duration-300"
              :style="{ width: `${(group.count / statusGroups.total) * 100}%`, background: group.color }"
              :title="`${group.key}: ${group.count}`"
            ></div>
          </div>
          <div class="mt-3 flex flex-wrap gap-x-4 gap-y-1.5">
            <span v-for="group in statusGroups.groups" :key="group.key" class="flex items-center gap-1.5 text-xs text-[var(--theme-text-secondary)]">
              <span class="inline-block h-2 w-2 rounded-full" :style="{ background: group.color }"></span>
              <b class="font-mono text-xs text-[var(--theme-text-primary)]">{{ group.key }}</b>
              <span class="font-mono text-xs text-[var(--theme-text-tertiary)]">
                {{ group.count }} · {{ Math.round((group.count / statusGroups.total) * 100) }}%
              </span>
            </span>
          </div>
          <p class="mt-2 text-xs text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.analytics.perfStatusTotal', { count: formatRequestCount(statusGroups.total) }) }}
          </p>
        </div>
        <div
          v-else
          class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-6 text-center text-xs text-[var(--theme-text-tertiary)]"
        >
          {{ t(locale, 'desktop.analytics.perfRecordsEmpty') }}
        </div>
      </section>

      <!-- 最慢模型 / 来源排行（D）：平均耗时 top 5 -->
      <section class="grid gap-3 lg:grid-cols-2">
        <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
          <h3 class="mb-2 flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
            <Gauge :size="14" class="shrink-0" aria-hidden="true" />
            {{ t(locale, 'desktop.analytics.perfSlowestModels') }}
          </h3>
          <ul v-if="slowestModels.length" class="flex flex-col gap-1.5">
            <li v-for="(row, index) in slowestModels" :key="row.label" class="flex items-center gap-2 text-xs">
              <span class="w-4 shrink-0 text-right font-mono text-xs text-[var(--theme-text-quaternary)]">{{ index + 1 }}</span>
              <span class="min-w-0 flex-1 truncate text-[var(--theme-text-secondary)]" :title="row.label">{{ row.label }}</span>
              <span class="shrink-0 font-mono font-semibold text-[var(--theme-text-primary)]">{{ formatDurationMs(row.avgMs) }}</span>
              <span class="shrink-0 rounded-full border border-[var(--theme-border-default)] px-1.5 py-px font-mono text-xs text-[var(--theme-text-tertiary)]">
                {{ t(locale, 'desktop.analytics.perfRankRequests', { count: row.count }) }}
              </span>
            </li>
          </ul>
          <p
            v-else
            class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-5 text-center text-xs text-[var(--theme-text-tertiary)]"
          >
            {{ t(locale, 'desktop.analytics.perfRecordsEmpty') }}
          </p>
        </div>
        <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
          <h3 class="mb-2 flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
            <Globe :size="14" class="shrink-0" aria-hidden="true" />
            {{ t(locale, 'desktop.analytics.perfSlowestSources') }}
          </h3>
          <ul v-if="slowestSources.length" class="flex flex-col gap-1.5">
            <li v-for="(row, index) in slowestSources" :key="row.label" class="flex items-center gap-2 text-xs">
              <span class="w-4 shrink-0 text-right font-mono text-xs text-[var(--theme-text-quaternary)]">{{ index + 1 }}</span>
              <span class="min-w-0 flex-1 truncate text-[var(--theme-text-secondary)]" :title="row.label">{{ row.label }}</span>
              <span class="shrink-0 font-mono font-semibold text-[var(--theme-text-primary)]">{{ formatDurationMs(row.avgMs) }}</span>
              <span class="shrink-0 rounded-full border border-[var(--theme-border-default)] px-1.5 py-px font-mono text-xs text-[var(--theme-text-tertiary)]">
                {{ t(locale, 'desktop.analytics.perfRankRequests', { count: row.count }) }}
              </span>
            </li>
          </ul>
          <p
            v-else
            class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-5 text-center text-xs text-[var(--theme-text-tertiary)]"
          >
            {{ t(locale, 'desktop.analytics.perfRecordsEmpty') }}
          </p>
        </div>
      </section>

      <!-- 最慢 20 条请求子表（E）：行点击仅选中态；完整详情跳转请求页 -->
      <section class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
        <div class="mb-2 flex items-center justify-between gap-2">
          <h3 class="flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
            <Table2 :size="14" class="shrink-0" aria-hidden="true" />
            {{ t(locale, 'desktop.analytics.perfRequestTable') }}
          </h3>
          <button
            type="button"
            class="flex items-center gap-1 rounded-md px-2 py-1 text-xs font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:bg-[var(--theme-accent-soft)]"
            @click="nav.navigate('requests')"
          >
            {{ t(locale, 'desktop.analytics.perfRequestTableOpen') }}
            <ArrowRight :size="12" aria-hidden="true" />
          </button>
        </div>
        <div v-if="slowestRequests.length" class="overflow-x-auto">
          <table class="w-full min-w-[720px] border-collapse text-xs">
            <thead>
              <tr class="border-b border-[var(--theme-border-default)] text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnTime') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnTool') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnModel') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnStatus') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnTtft') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnDuration') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnRate') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="request in slowestRequests"
                :key="request.requestKey"
                class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors duration-150 last:border-0 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
                :class="selectedRequestKey === request.requestKey ? 'bg-[var(--theme-accent-soft)]' : ''"
                tabindex="0"
                @click="toggleRow(request)"
                @keydown.enter.prevent="toggleRow(request)"
                @keydown.space.prevent="toggleRow(request)"
              >
                <td class="whitespace-nowrap px-2.5 py-1.5 text-[var(--theme-text-secondary)]">{{ display.formatTime(request.timestampSec) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-[var(--theme-text-secondary)]">{{ display.requestToolLabel(request.tool) }}</td>
                <td class="max-w-40 truncate px-2.5 py-1.5 text-[var(--theme-text-secondary)]" :title="request.model">{{ display.requestModelLabel(request) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5">
                  <span v-if="request.coverageOrigin === 'local_only'" class="text-xs text-[var(--theme-text-quaternary)]">—</span>
                  <span
                    v-else
                    class="inline-flex items-center rounded-full border px-1.5 py-px text-xs font-bold leading-none"
                    :class="display.requestStatusClasses(request)"
                  >
                    {{ display.requestStatusLabel(request) }}
                  </span>
                </td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">
                  {{ display.requestHasProxyPerformance(request) ? display.formatDuration(request.ttftMs) : '—' }}
                </td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono font-semibold text-[var(--theme-text-primary)]">
                  {{ display.formatDuration(request.durationMs) }}
                </td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">
                  {{ display.requestHasProxyPerformance(request) && request.outputTokensPerSecond ? `${formatRate(request.outputTokensPerSecond)}t/s` : '—' }}
                </td>
              </tr>
            </tbody>
          </table>
          <p class="mt-2 flex items-center gap-1.5 text-xs text-[var(--theme-text-tertiary)]">
            <Info :size="12" class="shrink-0" aria-hidden="true" />
            {{ t(locale, 'desktop.analytics.perfRequestTableHint') }}
          </p>
        </div>
        <div
          v-else
          class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center text-xs text-[var(--theme-text-tertiary)]"
        >
          {{ t(locale, 'desktop.analytics.perfRecordsEmpty') }}
        </div>
      </section>
    </template>

    <!-- 性能视图顶部附加说明：即使有数据也始终显示覆盖提示（设计 7.7） -->
    <p v-if="hasPerformance" class="flex items-center gap-1.5 text-xs text-[var(--theme-text-tertiary)]">
      <Timer :size="12" class="shrink-0" aria-hidden="true" />
      {{ t(locale, 'desktop.analytics.performanceCoverageNote') }}
    </p>
  </div>
</template>
