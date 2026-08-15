<script setup lang="ts">
/**
 * 桌面主窗口 - 分析页（设计文档第 7 章）。
 * 四个二级视图（趋势/构成/活跃度/性能）+ 工具栏（范围/粒度/主指标/比较/导出占位）。
 * 时间范围由页面内部工具栏读写 desktopAnalytics store 的 analyticsWindow 字段（设计 7.3）。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { LineChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import VChart from 'vue-echarts'
import {
  BarChart3,
  CalendarDays,
  ChevronLeft,
  ChevronRight,
  FileDown,
  Gauge,
  RefreshCw,
  ShieldCheck,
  Table2,
  Timer,
  Zap
} from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import {
  previousRangeQuery,
  rangeQueryForWindow,
  resolveChartTheme,
  useDesktopAnalyticsStore,
  withAlpha,
  type AnalyticsCompare,
  type AnalyticsGranularity,
  type AnalyticsView
} from '../stores/desktopAnalytics'
import { t, windowNameLabel } from '../../i18n'
import {
  formatCost,
  formatDurationMs,
  formatRate,
  formatRequestCount,
  formatTokenPair,
  formatTokenValue
} from '../../utils/format'
import { formatToolDisplayName } from '../../utils/toolDisplay'
import {
  getMonthDayCount,
  intensityClass,
  makeEmptyDay,
  METRICS,
  valueOf,
  formatLocalDate
} from '../../components/statistics/activityUtils'
import type {
  DayActivity,
  OverviewBreakdownItem,
  ProjectStats,
  StatisticsMetric,
  StatisticsSummary,
  StatisticsTrendPoint,
  WindowName
} from '../../types'
import { WINDOW_ORDER } from '../../types'

use([CanvasRenderer, LineChart, GridComponent, TooltipComponent])

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

const dayBoundaryHour = computed(() => (store.settings.dayBoundaryMode === 'night_owl' ? 4 : 0))

// ============ 顶部 tabs（设计 7.2：二级视图，不加入侧栏） ============

const viewTabs: Array<{ value: AnalyticsView; key: string }> = [
  { value: 'trend', key: 'desktop.analytics.tabsTrend' },
  { value: 'composition', key: 'desktop.analytics.tabsComposition' },
  { value: 'activity', key: 'desktop.analytics.tabsActivity' },
  { value: 'performance', key: 'desktop.analytics.tabsPerformance' }
]

// ============ 工具栏（设计 7.3） ============

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

// ============ 趋势视图（设计 7.4） ============

/** 分析范围查询：auto 粒度跟随窗口默认，hour/day 由工具栏覆盖。 */
const trendQuery = computed(() => {
  const q = rangeQueryForWindow(analytics.analyticsWindow, store.settings.timezone, dayBoundaryHour.value)
  if (analytics.analyticsGranularity === 'hour') return { ...q, bucket: 'hour' as const }
  if (analytics.analyticsGranularity === 'day') return { ...q, bucket: 'day' as const }
  return q
})

/** 上一等长周期摘要（比较模式开启时）。statisticsSummary 是共享状态，串行拉取避免互相覆盖。 */
const prevSummary = ref<StatisticsSummary | null>(null)

async function fetchTrend() {
  await store.fetchStatisticsSummary(trendQuery.value)
  if (analytics.analyticsCompare === 'previous') {
    await store.fetchStatisticsSummary(previousRangeQuery(trendQuery.value))
    prevSummary.value = store.statisticsSummary
    // 恢复主显示为当前周期（prev 数据只进 prevSummary）
    await store.fetchStatisticsSummary(trendQuery.value)
  } else {
    prevSummary.value = null
  }
}

const trendPoints = computed(() => store.statisticsSummary?.trend ?? [])
const prevPoints = computed(() => prevSummary.value?.trend ?? [])

function metricValue(point: StatisticsTrendPoint, metric: StatisticsMetric): number {
  if (metric === 'cost') return point.cost
  if (metric === 'tokens') return point.totalTokens
  return point.requestCount
}

function formatMetric(metric: StatisticsMetric, value: number): string {
  if (metric === 'cost') return formatCost(value, store.settings.currency)
  if (metric === 'tokens') return formatTokenValue(value)
  return formatRequestCount(value)
}

function metricLabel(metric: StatisticsMetric): string {
  if (metric === 'cost') return t(locale.value, 'statistics.metricCost')
  if (metric === 'tokens') return t(locale.value, 'statistics.metricTokens')
  return t(locale.value, 'statistics.metricRequests')
}

const chartTheme = computed(() => {
  void store.settings.theme.appearance
  void store.settings.theme.lightPalette
  void store.settings.theme.darkPalette
  return resolveChartTheme()
})

function metricColor(metric: StatisticsMetric): string {
  if (metric === 'cost') return chartTheme.value.cost
  if (metric === 'tokens') return chartTheme.value.tokens
  return chartTheme.value.requests
}

function changePercent(cur: number, prev: number): number | null {
  if (!prev) return null
  return ((cur - prev) / prev) * 100
}

const trendChartOptions = computed(() => {
  const colors = chartTheme.value
  const points = trendPoints.value
  const prev = prevPoints.value
  const hasPrev = prev.length > 0 && analytics.analyticsCompare === 'previous'
  const primaryColor = metricColor(analytics.analyticsMetric)
  return {
    grid: { left: 48, right: 12, top: 14, bottom: 22 },
    tooltip: {
      trigger: 'axis',
      backgroundColor: colors.tooltipBg,
      borderColor: colors.tooltipBorder,
      borderRadius: 8,
      padding: [7, 9],
      textStyle: { color: colors.tooltipText, fontSize: 11 },
      // 同时显示当前周期与对比周期的精确值和变化率（设计 7.4）
      formatter: (params: any) => {
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
    },
    xAxis: {
      type: 'category',
      data: points.map(p => p.label),
      boundaryGap: false,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: colors.axis, fontSize: 10, hideOverlap: true, margin: 8 }
    },
    yAxis: {
      type: 'value',
      min: 0,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: {
        color: colors.axis,
        fontSize: 10,
        formatter: (value: number) => formatMetric(analytics.analyticsMetric, value)
      },
      splitLine: { lineStyle: { type: 'dashed', color: colors.grid } }
    },
    series: [
      {
        name: t(locale.value, 'desktop.analytics.trendCurrent'),
        type: 'line',
        data: points.map(p => metricValue(p, analytics.analyticsMetric)),
        smooth: true,
        showSymbol: false,
        lineStyle: { width: 2.5, color: primaryColor },
        itemStyle: { color: primaryColor },
        emphasis: { focus: 'series' },
        areaStyle: {
          color: {
            type: 'linear',
            x: 0,
            y: 0,
            x2: 0,
            y2: 1,
            colorStops: [
              { offset: 0, color: withAlpha(primaryColor, '26') },
              { offset: 1, color: withAlpha(primaryColor, '04') }
            ]
          }
        }
      },
      ...(hasPrev
        ? [
            {
              name: t(locale.value, 'desktop.analytics.trendPrevious'),
              type: 'line',
              data: prev.map(p => metricValue(p, analytics.analyticsMetric)),
              smooth: true,
              showSymbol: false,
              lineStyle: { width: 1.8, type: 'dashed', color: colors.series3 },
              itemStyle: { color: colors.series3 },
              emphasis: { focus: 'series' }
            }
          ]
        : [])
    ]
  }
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
  const prev = prevPoints.value
  return trendPoints.value
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

// ============ 构成视图（设计 7.5） ============

type CompositionDimension = 'source' | 'tool' | 'model' | 'project'

const dimensionOptions: Array<{ value: CompositionDimension; key: string }> = [
  { value: 'source', key: 'desktop.analytics.dimensionSource' },
  { value: 'tool', key: 'desktop.analytics.dimensionTool' },
  { value: 'model', key: 'desktop.analytics.dimensionModel' },
  { value: 'project', key: 'desktop.analytics.dimensionProject' }
]

const dimension = ref<CompositionDimension>('source')

type CompositionSortKey = 'percent' | 'requests' | 'tokens' | 'cost' | 'speed' | 'errorRate' | 'lastSeen'

interface CompositionRow {
  id: string
  name: string
  percent: number
  requests: number
  tokens: number
  inputTokens: number
  outputTokens: number
  cost: number
  speed: number | null
  errorRate: number | null
  lastSeen: number | null
}

const sortKey = ref<CompositionSortKey>('percent')
const sortDir = ref<'asc' | 'desc'>('desc')

const sortableColumns: Array<{ key: CompositionSortKey; labelKey: string }> = [
  { key: 'percent', labelKey: 'desktop.analytics.compPercent' },
  { key: 'requests', labelKey: 'desktop.analytics.compRequests' },
  { key: 'tokens', labelKey: 'desktop.analytics.compTokens' },
  { key: 'cost', labelKey: 'desktop.analytics.compCost' },
  { key: 'speed', labelKey: 'desktop.analytics.compSpeed' },
  { key: 'errorRate', labelKey: 'desktop.analytics.compErrorRate' },
  { key: 'lastSeen', labelKey: 'desktop.analytics.compLastActive' }
]

function primaryValueOf(item: OverviewBreakdownItem): number {
  if (analytics.analyticsMetric === 'cost') return item.cost
  if (analytics.analyticsMetric === 'tokens') return item.totalTokens
  return item.requestCount
}

function projectPrimaryValue(p: ProjectStats): number {
  if (analytics.analyticsMetric === 'cost') return p.totalCost
  if (analytics.analyticsMetric === 'tokens') {
    return p.totalInputTokens + p.totalOutputTokens + p.totalCacheCreateTokens + p.totalCacheReadTokens
  }
  return p.requestCount
}

function displayLabel(item: OverviewBreakdownItem): string {
  if (item.kind === 'tool') {
    return formatToolDisplayName(item.id, locale.value, store.settings.clientTools.profiles)
  }
  if (item.label === '__unknown__') return t(locale.value, 'sources.unknown')
  if (item.label === '__official_api__') return t(locale.value, 'sources.officialAnthropic')
  return item.label
}

function rowsFromItems(items: OverviewBreakdownItem[]): CompositionRow[] {
  const total = Math.max(items.reduce((sum, i) => sum + primaryValueOf(i), 0), 1)
  return items.map(item => ({
    id: item.id,
    name: displayLabel(item),
    percent: (primaryValueOf(item) / total) * 100,
    requests: item.requestCount,
    tokens: item.totalTokens,
    inputTokens: item.inputTokens,
    outputTokens: item.outputTokens,
    cost: item.cost,
    speed: item.avgTokensPerSecond ?? null,
    errorRate: item.errorRequests != null && item.requestCount > 0 ? (item.errorRequests / item.requestCount) * 100 : null,
    lastSeen: item.lastSeenMs ?? null
  }))
}

function rowsFromProjects(projects: ProjectStats[]): CompositionRow[] {
  const total = Math.max(projects.reduce((sum, p) => sum + projectPrimaryValue(p), 0), 1)
  return projects.map(p => ({
    id: p.projectKey ?? p.name,
    name: p.name === '__unknown__' ? t(locale.value, 'common.unknownProject') : p.name,
    percent: (projectPrimaryValue(p) / total) * 100,
    requests: p.requestCount,
    tokens: p.totalInputTokens + p.totalOutputTokens + p.totalCacheCreateTokens + p.totalCacheReadTokens,
    inputTokens: p.totalInputTokens + p.totalCacheReadTokens,
    outputTokens: p.totalOutputTokens,
    cost: p.totalCost,
    speed: null,
    errorRate: null,
    lastSeen: p.lastActive
  }))
}

const compositionData = computed<CompositionRow[]>(() => {
  const b = store.overviewBreakdown
  if (dimension.value === 'source') return rowsFromItems(b?.sourceRanking ?? [])
  if (dimension.value === 'tool') return rowsFromItems(b?.toolRanking ?? [])
  if (dimension.value === 'model') return rowsFromItems(b?.modelRanking ?? [])
  return rowsFromProjects(store.projectStats)
})

function columnValue(row: CompositionRow, key: CompositionSortKey): number {
  if (key === 'percent') return row.percent
  if (key === 'requests') return row.requests
  if (key === 'tokens') return row.tokens
  if (key === 'cost') return row.cost
  if (key === 'speed') return row.speed ?? -1
  if (key === 'errorRate') return row.errorRate ?? -1
  return row.lastSeen ?? -1
}

/** 排序使用原始数值，不使用格式化字符串（设计 7.8）。 */
const sortedComposition = computed(() => {
  const rows = [...compositionData.value]
  const dir = sortDir.value === 'desc' ? -1 : 1
  rows.sort((a, b) => {
    const primary = (columnValue(b, sortKey.value) - columnValue(a, sortKey.value)) * dir
    if (primary !== 0) return primary
    return b.requests - a.requests || a.name.localeCompare(b.name)
  })
  return rows
})

function toggleSort(key: CompositionSortKey) {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 'desc' ? 'asc' : 'desc'
  } else {
    sortKey.value = key
    sortDir.value = 'desc'
  }
}

function switchDimension(d: CompositionDimension) {
  dimension.value = d
  sortKey.value = 'percent'
  sortDir.value = 'desc'
}

function formatRelativeTime(epoch: number | null): string {
  if (!epoch) return '—'
  const diffMs = Date.now() - epoch * 1000
  if (diffMs < 60_000) return t(locale.value, 'common.justNow')
  const minutes = Math.floor(diffMs / 60_000)
  if (minutes < 60) return `${minutes}${t(locale.value, 'common.minutesAgo')}`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}${t(locale.value, 'common.hoursAgo')}`
  return `${Math.floor(hours / 24)}${t(locale.value, 'common.daysAgo')}`
}

// ============ 活跃度视图（设计 7.6） ============

const currentMonth = ref(new Date())
const selectedDate = ref('')

const monthYear = computed(() => currentMonth.value.getFullYear())
const monthNumber = computed(() => currentMonth.value.getMonth() + 1)

function moveMonth(delta: number) {
  currentMonth.value = new Date(monthYear.value, currentMonth.value.getMonth() + delta, 1)
  selectedDate.value = ''
}

function selectDay(day: DayActivity) {
  selectedDate.value = day.date
}

/** 月历内最大值（用于单元格强度归一化）。 */
function monthMaxValue(): number {
  const days = calendarCells.value
    .map(c => c.day)
    .filter((d): d is DayActivity => !!d)
  return Math.max(...days.map(d => valueOf(d, analytics.analyticsMetric)), 0)
}

/** 月历单元格强度（0-1）。 */
function monthCellRatio(day: DayActivity | null): number {
  if (!day) return 0
  const max = monthMaxValue()
  if (max <= 0) return 0
  return valueOf(day, analytics.analyticsMetric) / max
}

/** 月历单元格（复用 activityUtils 语义）。 */
const calendarCells = computed(() => {
  const activity = store.monthActivity
  const activeDays = activity && activity.year === monthYear.value && activity.month === monthNumber.value ? activity.days : []
  const dayMap = new Map(activeDays.map(d => [d.date, d]))
  const dayCount = getMonthDayCount(monthYear.value, monthNumber.value)
  const firstDay = new Date(monthYear.value, monthNumber.value - 1, 1).getDay()
  const leadingCount = firstDay === 0 ? 6 : firstDay - 1
  const cells: Array<{ key: string; day: DayActivity | null; dayNumber: string }> = Array.from(
    { length: leadingCount },
    (_, index) => ({ key: `blank-leading-${index}`, day: null, dayNumber: '' })
  )
  for (let day = 1; day <= dayCount; day += 1) {
    const date = `${monthYear.value}-${String(monthNumber.value).padStart(2, '0')}-${String(day).padStart(2, '0')}`
    cells.push({ key: date, day: dayMap.get(date) ?? makeEmptyDay(date), dayNumber: String(day) })
  }
  const trailingCount = Math.max(0, Math.ceil(cells.length / 7) * 7 - cells.length)
  return [
    ...cells,
    ...Array.from({ length: trailingCount }, (_, index) => ({
      key: `blank-trailing-${index}`,
      day: null,
      dayNumber: ''
    }))
  ]
})

const weekDayLabels = computed(() => [
  t(locale.value, 'statistics.weekMon'),
  t(locale.value, 'statistics.weekTue'),
  t(locale.value, 'statistics.weekWed'),
  t(locale.value, 'statistics.weekThu'),
  t(locale.value, 'statistics.weekFri'),
  t(locale.value, 'statistics.weekSat'),
  t(locale.value, 'statistics.weekSun')
])

/** 年度贡献单元格：53 周 × 7 天（可水平压缩，不出现页面级横向滚动，设计 7.6）。 */
const annualCells = computed(() => {
  const activity = store.yearActivity
  const days = activity && activity.year === monthYear.value ? activity.days : []
  const dayMap = new Map(days.map(d => [d.date, d]))
  const start = new Date(monthYear.value, 0, 1)
  const end = new Date(monthYear.value + 1, 0, 1)
  const leadingCount = start.getDay() === 0 ? 6 : start.getDay() - 1
  const cells: Array<{ key: string; day: DayActivity | null }> = Array.from({ length: leadingCount }, (_, i) => ({
    key: `year-leading-${i}`,
    day: null
  }))
  for (let time = start.getTime(); time < end.getTime(); time += 86400000) {
    const date = new Date(time)
    const key = formatLocalDate(date)
    cells.push({ key, day: dayMap.get(key) ?? makeEmptyDay(key) })
  }
  const trailingCount = Math.max(0, Math.ceil(cells.length / 7) * 7 - cells.length)
  return [
    ...cells,
    ...Array.from({ length: trailingCount }, (_, i) => ({ key: `year-trailing-${i}`, day: null }))
  ]
})

const annualMaxValue = computed(() => {
  const days = annualCells.value.map(c => c.day).filter((d): d is DayActivity => !!d)
  return Math.max(...days.map(d => valueOf(d, analytics.analyticsMetric)), 0)
})

function cellRatio(day: DayActivity): number {
  if (annualMaxValue.value <= 0) return 0
  return valueOf(day, analytics.analyticsMetric) / annualMaxValue.value
}

function dayTooltip(day: DayActivity): string {
  const v = valueOf(day, analytics.analyticsMetric)
  return `${day.date} · ${formatMetric(analytics.analyticsMetric, v)} · ${t(locale.value, 'desktop.analytics.activityRequests', { count: formatRequestCount(day.requestCount) })}`
}

function monthCalendarTooltip(day: DayActivity): string {
  const v = valueOf(day, analytics.analyticsMetric)
  const rows = [
    `${t(locale.value, 'desktop.analytics.totalsRequests')} ${formatRequestCount(day.requestCount)}`,
    `${t(locale.value, 'desktop.analytics.totalsTokens')} ${formatTokenValue(day.totalTokens)}`,
    `${t(locale.value, 'desktop.analytics.totalsCost')} ${formatCost(day.cost, store.settings.currency)}`
  ].join(' · ')
  return `${day.date} · ${metricLabel(analytics.analyticsMetric)} ${formatMetric(analytics.analyticsMetric, v)} · ${rows}`
}

// ============ 性能视图（设计 7.7） ============

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

// ============ 数据加载 ============

function applyPendingFilters() {
  const filters = nav.consumePendingFilters()
  if (!filters) return
  if (filters.sourceId && filters.sourceId !== store.settings.sourceAware.activeSourceFilter) {
    void store.setActiveSourceFilter(filters.sourceId)
  }
  if (filters.tool && filters.tool !== store.settings.clientTools.activeToolFilter) {
    void store.setActiveToolFilter(filters.tool)
  }
  // window/metric/view 属于分析页局部状态（设计 3.5）：仅接受合法值，非法忽略。
  if (filters.window && (WINDOW_ORDER as readonly string[]).includes(filters.window)) {
    analytics.analyticsWindow = filters.window as WindowName
  }
  if (filters.metric && ['cost', 'requests', 'tokens'].includes(filters.metric)) {
    analytics.analyticsMetric = filters.metric as StatisticsMetric
  }
  if (filters.view && ['trend', 'composition', 'activity', 'performance'].includes(filters.view)) {
    analytics.analyticsView = filters.view as AnalyticsView
  }
}

watch(
  () => [analytics.analyticsWindow, analytics.analyticsGranularity, analytics.analyticsCompare] as const,
  () => void fetchTrend()
)

watch(
  () => [monthYear.value, monthNumber.value, analytics.analyticsMetric] as const,
  () => {
    void store.fetchMonthActivity(monthYear.value, monthNumber.value, analytics.analyticsMetric)
    void store.fetchYearActivity(monthYear.value, analytics.analyticsMetric)
  },
  { immediate: true }
)

// 同页深链：hash 相同页面不重挂载，onMounted 消费路径不执行；
// pendingConsumeTick 变化时若本页激活则补消费（跨页场景由 onMounted 覆盖，这里幂等）。
watch(
  () => nav.pendingConsumeTick,
  () => {
    if (nav.currentPage === 'analytics') applyPendingFilters()
  }
)

onMounted(() => {
  applyPendingFilters()
  void store.fetchOverviewBreakdown(analytics.analyticsWindow)
  void store.fetchProjectStats()
  void fetchTrend()
})
</script>

<template>
  <section class="flex flex-col gap-4">
    <!-- 顶部 tabs：趋势 / 构成 / 活跃度 / 性能（设计 7.2） -->
    <div
      class="flex w-fit items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="tablist"
      :aria-label="t(locale, 'desktop.analytics.tabsLabel')"
    >
      <button
        v-for="tab in viewTabs"
        :key="tab.value"
        type="button"
        role="tab"
        :aria-selected="analytics.analyticsView === tab.value"
        class="rounded-md px-3 py-1.5 text-xs font-medium transition-colors duration-150"
        :class="
          analytics.analyticsView === tab.value
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        @click="analytics.analyticsView = tab.value"
      >
        {{ t(locale, tab.key) }}
      </button>
    </div>

    <!-- 工具栏：时间范围 / 粒度 / 主指标 / 比较 / 导出占位（设计 7.3） -->
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

    <!-- ============ 趋势视图 ============ -->
    <div v-if="analytics.analyticsView === 'trend'" class="flex flex-col gap-5">
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
            <span v-if="analytics.analyticsCompare === 'previous'" class="text-[10px] font-normal text-[var(--theme-text-quaternary)]">
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

        <div v-if="trendPoints.length" class="h-[260px]">
          <v-chart class="h-full w-full" :option="trendChartOptions" autoresize />
        </div>
        <div
          v-else-if="store.statisticsLoading"
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
            @click="() => void fetchTrend()"
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

    <!-- ============ 构成视图 ============ -->
    <div v-else-if="analytics.analyticsView === 'composition'" class="flex flex-col gap-3">
      <div class="flex flex-wrap items-center gap-2">
        <div
          class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
          role="group"
          :aria-label="t(locale, 'desktop.analytics.dimensionLabel')"
        >
          <button
            v-for="d in dimensionOptions"
            :key="d.value"
            type="button"
            class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
            :class="
              dimension === d.value
                ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
                : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
            "
            :aria-pressed="dimension === d.value"
            @click="switchDimension(d.value)"
          >
            {{ t(locale, d.key) }}
          </button>
        </div>
      </div>

      <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
        <div v-if="sortedComposition.length" class="overflow-x-auto">
          <table class="w-full min-w-[720px] text-xs">
            <thead>
              <tr class="border-b border-[var(--theme-border-subtle)] text-left text-[10px] font-medium uppercase tracking-wide text-[var(--theme-text-quaternary)]">
                <th class="py-2 pr-3 font-medium">#</th>
                <th class="py-2 pr-3 font-medium">{{ t(locale, 'desktop.analytics.compName') }}</th>
                <th
                  v-for="col in sortableColumns"
                  :key="col.key"
                  class="cursor-pointer select-none py-2 pr-3 font-medium transition-colors duration-150 hover:text-[var(--theme-text-secondary)]"
                  :class="sortKey === col.key ? 'text-[var(--theme-accent-primary)]' : ''"
                  :aria-sort="sortKey === col.key ? (sortDir === 'desc' ? 'descending' : 'ascending') : 'none'"
                  @click="toggleSort(col.key)"
                >
                  <span class="inline-flex items-center gap-0.5">
                    {{ t(locale, col.labelKey) }}
                    <span v-if="sortKey === col.key" class="text-[9px]">{{ sortDir === 'desc' ? '↓' : '↑' }}</span>
                  </span>
                </th>
              </tr>
            </thead>
            <tbody class="divide-y divide-[var(--theme-border-subtle)]">
              <tr
                v-for="(row, index) in sortedComposition"
                :key="row.id"
                class="transition-colors duration-150 hover:bg-[var(--theme-bg-hover)]"
              >
                <td class="py-2 pr-3 text-[var(--theme-text-quaternary)]">{{ index + 1 }}</td>
                <td class="max-w-[180px] truncate py-2 pr-3 font-medium text-[var(--theme-text-primary)]" :title="row.name">
                  {{ row.name }}
                </td>
                <td class="py-2 pr-3">
                  <span class="flex items-center gap-2">
                    <span class="h-1 w-20 overflow-hidden rounded-full bg-[var(--theme-text-primary)]/10">
                      <span class="block h-full rounded-full" :style="{ width: `${Math.min(row.percent, 100)}%`, backgroundColor: 'var(--theme-accent-primary)' }"></span>
                    </span>
                    <span class="font-mono text-[10px] text-[var(--theme-text-tertiary)]">{{ formatRate(row.percent) }}%</span>
                  </span>
                </td>
                <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatRequestCount(row.requests) }}</td>
                <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]" :title="`${t(locale, 'desktop.analytics.compInput')} ${formatTokenValue(row.inputTokens)} · ${t(locale, 'desktop.analytics.compOutput')} ${formatTokenValue(row.outputTokens)}`">
                  {{ formatTokenValue(row.tokens) }}
                </td>
                <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatCost(row.cost, store.settings.currency) }}</td>
                <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">
                  {{ row.speed == null || !hasPerformance ? '—' : `${formatRate(row.speed)} t/s` }}
                </td>
                <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">
                  {{ row.errorRate == null ? '—' : `${formatRate(row.errorRate)}%` }}
                </td>
                <td class="py-2 text-right text-[var(--theme-text-tertiary)]" :title="row.lastSeen ? new Date(row.lastSeen).toLocaleString(locale.replace('_', '-')) : undefined">
                  {{ formatRelativeTime(row.lastSeen) }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <div
          v-else
          class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-10 text-center text-xs leading-5 text-[var(--theme-text-tertiary)]"
        >
          {{ t(locale, 'desktop.analytics.compEmpty') }}
        </div>
      </div>
    </div>

    <!-- ============ 活跃度视图 ============ -->
    <div v-else-if="analytics.analyticsView === 'activity'" class="flex flex-col gap-5">
      <!-- 月历 + 年度贡献图 -->
      <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
        <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
          <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
            <CalendarDays :size="14" class="shrink-0" aria-hidden="true" />
            {{ t(locale, 'desktop.analytics.monthCalendar') }}
            <span class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">
              {{ monthYear }}-{{ String(monthNumber).padStart(2, '0') }}
            </span>
          </h3>
          <div class="flex items-center gap-1.5">
            <div
              class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
              role="group"
              :aria-label="t(locale, 'desktop.analytics.toolbarMetric')"
            >
              <button
                v-for="m in METRICS"
                :key="m.value"
                type="button"
                class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
                :class="
                  analytics.analyticsMetric === m.value
                    ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
                    : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
                "
                :aria-pressed="analytics.analyticsMetric === m.value"
                @click="analytics.analyticsMetric = m.value"
              >
                {{ t(locale, m.key) }}
              </button>
            </div>
            <button
              type="button"
              class="grid h-7 w-7 place-items-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
              :title="t(locale, 'statistics.previousMonth')"
              :aria-label="t(locale, 'statistics.previousMonth')"
              @click="moveMonth(-1)"
            >
              <ChevronLeft :size="15" aria-hidden="true" />
            </button>
            <button
              type="button"
              class="grid h-7 w-7 place-items-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
              :title="t(locale, 'statistics.nextMonth')"
              :aria-label="t(locale, 'statistics.nextMonth')"
              @click="moveMonth(1)"
            >
              <ChevronRight :size="15" aria-hidden="true" />
            </button>
          </div>
        </div>

        <!-- 月历：完整周标题 + 日期 + 精确 tooltip（设计 7.6） -->
        <div class="grid grid-cols-7 gap-1.5">
          <div
            v-for="wd in weekDayLabels"
            :key="wd"
            class="pb-1 text-center text-[10px] font-medium text-[var(--theme-text-quaternary)]"
          >
            {{ wd }}
          </div>
          <div
            v-for="cell in calendarCells"
            :key="cell.key"
            class="grid aspect-square place-items-center"
          >
            <button
              v-if="cell.day"
              type="button"
              class="grid h-full w-full place-items-center rounded-md border text-[11px] font-mono transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
              :class="[
                intensityClass(monthCellRatio(cell.day)),
                selectedDate === cell.key
                  ? 'border-[var(--theme-accent-primary)] ring-2 ring-[var(--theme-ring-focus)]'
                  : 'border-transparent hover:border-[var(--theme-border-strong)]'
              ]"
              :title="monthCalendarTooltip(cell.day)"
              @click="selectDay(cell.day)"
            >
              {{ cell.dayNumber }}
            </button>
            <span v-else></span>
          </div>
        </div>
      </div>

      <!-- 年度贡献图：横向 53 周、纵向 7 天 -->
      <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
        <h3 class="mb-3 flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
          <BarChart3 :size="14" class="shrink-0" aria-hidden="true" />
          {{ t(locale, 'desktop.analytics.yearContribution') }}
          <span class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">{{ monthYear }}</span>
        </h3>
        <div
          v-if="store.yearActivityLoading && !store.yearActivity"
          class="h-[120px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"
          :aria-label="t(locale, 'common.syncing')"
        ></div>
        <div
          v-else-if="annualCells.some(c => c.day && valueOf(c.day, analytics.analyticsMetric) > 0)"
          class="grid grid-flow-col grid-rows-7 gap-[2px] overflow-hidden"
          :style="{ gridAutoColumns: 'minmax(0, 1fr)' }"
        >
          <span
            v-for="cell in annualCells"
            :key="cell.key"
            :class="[
              'aspect-square w-full rounded-[3px]',
              cell.day ? intensityClass(cellRatio(cell.day)) : 'bg-transparent'
            ]"
            :title="cell.day ? dayTooltip(cell.day) : undefined"
          ></span>
        </div>
        <div
          v-else
          class="grid place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center text-xs text-[var(--theme-text-tertiary)]"
        >
          {{ t(locale, 'desktop.analytics.yearEmpty') }}
        </div>
      </div>

      <!-- 小时 × 星期热力图：本期无数据源，占位说明 -->
      <div class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center">
        <p class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.hourlyHeatmapComingSoon') }}</p>
      </div>
    </div>

    <!-- ============ 性能视图 ============ -->
    <div v-else class="flex flex-col gap-5">
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
  </section>
</template>
