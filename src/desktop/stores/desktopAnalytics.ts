import { ref } from 'vue'
import { defineStore } from 'pinia'
import { useMonitorStore } from '../../stores/monitor'
import type { StatisticsBucket, StatisticsMetric, StatisticsQuery, WindowName } from '../../types'

/**
 * 主窗口的窗口局部状态（设计文档 3.5：主窗口持久化的分析范围、分析指标等）。
 *
 * M1 先存于内存（跨页面切换保持），后续版本可对接后端 storage 做持久化。
 * 时间范围字段供 DesktopPageHeader 顶栏读写：概览页 → overviewWindow，
 * 分析页 → analyticsWindow，其余页面（会话/活动占位页）沿用概览范围。
 */

export type AnalyticsView = 'trend' | 'composition' | 'activity' | 'performance'
export type AnalyticsGranularity = 'auto' | 'hour' | 'day'
export type AnalyticsCompare = 'none' | 'previous'

export const useDesktopAnalyticsStore = defineStore('desktopAnalytics', () => {
  const monitor = useMonitorStore()

  /** 概览页时间范围；首次进入继承快速面板 summaryWindow（设计 6.2），之后主窗口记忆自己的值。 */
  const overviewWindow = ref<WindowName>('24h')
  /** 分析页时间范围。 */
  const analyticsWindow = ref<WindowName>('24h')
  /** 概览/分析主指标：费用 / 请求 / Token（设计 6.4、7.3）。 */
  const analyticsMetric = ref<StatisticsMetric>('cost')
  /** 分析页二级视图（设计 4.2：同一页面内切换，不加入侧栏）。 */
  const analyticsView = ref<AnalyticsView>('trend')
  /** 趋势粒度：自动 / 小时 / 天（设计 7.3）。 */
  const analyticsGranularity = ref<AnalyticsGranularity>('auto')
  /** 比较方式：无 / 上一等长周期（设计 7.3）。 */
  const analyticsCompare = ref<AnalyticsCompare>('none')

  /** 是否已从快速面板继承过 summaryWindow（只继承一次，避免覆盖用户后续手动选择）。 */
  const overviewWindowInherited = ref(false)

  /**
   * 首次进入概览页时，以快速面板 summaryWindow 初始化概览范围。
   * 调用方需保证 settings 已从后端加载（例如 monitor.snapshot 非空时调用）。
   */
  function inheritSummaryWindowOnce() {
    if (overviewWindowInherited.value) return
    overviewWindowInherited.value = true
    overviewWindow.value = monitor.settings.summaryWindow
  }

  return {
    overviewWindow,
    analyticsWindow,
    analyticsMetric,
    analyticsView,
    analyticsGranularity,
    analyticsCompare,
    inheritSummaryWindowOnce
  }
})

/** 时间窗口 → 默认粒度：5h/24h/today 小时，7d/30d/current_month 天（设计 6.5）。 */
export function bucketForWindow(window: WindowName): StatisticsBucket {
  if (window === '5h' || window === '24h' || window === 'today') return 'hour'
  return 'day'
}

function startOfBusinessDay(date: Date, dayBoundaryHour: number): Date {
  const candidate = new Date(date.getFullYear(), date.getMonth(), date.getDate(), dayBoundaryHour, 0, 0, 0)
  if (date.getHours() < dayBoundaryHour) {
    candidate.setDate(candidate.getDate() - 1)
  }
  return candidate
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date)
  next.setDate(next.getDate() + days)
  return next
}

/**
 * 时间窗口 → 统计查询区间（半开区间 [start, end)，结束时刻为开区间，避免边界重复，设计 7.3）。
 * 与快速面板 Statistics.vue 的 presetRangeDates 语义保持一致。
 */
export function rangeQueryForWindow(
  window: WindowName,
  timezone: string,
  dayBoundaryHour: number,
  now: Date = new Date()
): StatisticsQuery {
  const bucket = bucketForWindow(window)
  let start: Date
  if (window === '5h') {
    start = new Date(now.getTime() - 5 * 60 * 60 * 1000)
  } else if (window === '24h') {
    start = new Date(now.getTime() - 24 * 60 * 60 * 1000)
  } else if (window === 'today') {
    start = startOfBusinessDay(now, dayBoundaryHour)
  } else if (window === '7d') {
    start = addDays(startOfBusinessDay(now, dayBoundaryHour), -6)
  } else if (window === '30d') {
    start = addDays(startOfBusinessDay(now, dayBoundaryHour), -29)
  } else {
    // current_month：本月 1 号（业务日界）起
    start = new Date(now.getFullYear(), now.getMonth(), 1, dayBoundaryHour, 0, 0, 0)
  }
  return {
    startEpoch: Math.floor(start.getTime() / 1000),
    endEpoch: Math.floor(now.getTime() / 1000),
    timezone,
    bucket
  }
}

/** 上一等长周期（与给定查询等长、紧邻其前，用于分析页"比较"）。 */
export function previousRangeQuery(query: StatisticsQuery): StatisticsQuery {
  const span = query.endEpoch - query.startEpoch
  return {
    ...query,
    startEpoch: query.startEpoch - span,
    endEpoch: query.startEpoch
  }
}

/** 将相对秒数格式化为 "1d2h / 3h4m / 5m" 风格倒计时（复用 subscription.* 单位键由调用方传入）。 */
export function formatCountdownSeconds(seconds: number, unitDay: string, unitHour: string, unitMinute: string): string {
  if (seconds <= 0) return '0' + unitMinute
  const mins = Math.floor(seconds / 60)
  const hours = Math.floor(mins / 60)
  const days = Math.floor(hours / 24)
  if (days > 0) {
    const remainHours = hours % 24
    return remainHours > 0 ? `${days}${unitDay}${remainHours}${unitHour}` : `${days}${unitDay}`
  }
  if (hours > 0) {
    const remainMins = mins % 60
    return remainMins > 0 ? `${hours}${unitHour}${remainMins}${unitMinute}` : `${hours}${unitHour}`
  }
  return `${mins}${unitMinute}`
}

/**
 * ECharts canvas 渲染器无法解析 var(--x) 字符串，需要把主题 CSS 变量解析为具体颜色。
 * 页面内把 theme 设置（appearance/palette）作为响应式依赖 touch 后重算，图表即可跟随主题。
 */
export function resolveChartTheme(): {
  requests: string
  tokens: string
  cost: string
  series3: string
  axis: string
  grid: string
  elevated: string
  tooltipBg: string
  tooltipBorder: string
  tooltipText: string
  tooltipSub: string
} {
  function cssVar(name: string): string {
    const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
    return value || '#7c8aa0'
  }
  return {
    requests: cssVar('--theme-chart-requests'),
    tokens: cssVar('--theme-chart-tokens'),
    cost: cssVar('--theme-chart-cost'),
    series3: cssVar('--theme-chart-series-3'),
    axis: cssVar('--theme-chart-axis'),
    grid: cssVar('--theme-chart-grid'),
    elevated: cssVar('--theme-bg-elevated'),
    tooltipBg: cssVar('--theme-chart-tooltip-bg'),
    tooltipBorder: cssVar('--theme-chart-tooltip-border'),
    tooltipText: cssVar('--theme-chart-tooltip-text'),
    tooltipSub: cssVar('--theme-chart-tooltip-subtext')
  }
}

/** 给 6 位 hex 颜色追加透明度（如 #10b981 + '26' → #10b98126），非法颜色原样返回避免图表渲染报错。 */
export function withAlpha(color: string, alphaHex: string): string {
  return /^#[0-9a-fA-F]{6}$/.test(color) ? `${color}${alphaHex}` : color
}
