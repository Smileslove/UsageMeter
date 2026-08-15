/**
 * 桌面主窗口趋势图共享逻辑（消除 Overview 6.5 与 Analytics 7.4 的重复实现）。
 *
 * 设计文档引用（图表风格规范）：
 * - 曲线使用 smooth 平滑样式；主序列带渐变色 areaStyle（withAlpha 透明度 26 → 04）；
 * - 隐藏非必要轴线（axisLine/axisTick 均不显示），splitLine 使用 dashed 细线；
 * - axisLabel 小字号（10px）+ hideOverlap，grid 边距统一 { left: 48, right: 12, top: 14, bottom: 22 }；
 * - tooltip 极简：圆点 + 粗体值，trigger: 'axis'。
 *
 * 两页差异不强行统一，通过参数化保留：
 * - tooltip formatter 内容（Overview 多指标 + 平均速率；Analytics 当前/上一周期 + 变化率）由页面注入；
 * - series 集合（Overview 多指标叠加 vs Analytics 当前 + 对比周期）由页面构建后传入；
 * - 副序列宽度/虚线（Overview 1.6 实线；Analytics 对比序列 1.8 虚线）由 trendLineSeries 参数化。
 *
 * 导出：
 * - registerChartComponents()：echarts use() 注册（幂等，重复调用无害）。
 * - useChartTheme()：touch theme 设置的响应式主题色（同 StatisticsTrendChart 的做法）。
 * - metricValue / formatMetric / metricLabel / metricColor：趋势指标取值与格式化工具（两页同口径）。
 * - trendLineSeries()：平滑折线系列构建（颜色/宽度/渐变面积/虚线参数化）。
 * - buildTrendChartOption()：grid / xAxis / yAxis / tooltip 骨架 + 页面注入 series 与 tooltip formatter。
 */
import { computed } from 'vue'
import type { ComputedRef } from 'vue'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { LineChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import { useMonitorStore } from '../../stores/monitor'
import { resolveChartTheme, withAlpha } from '../stores/desktopAnalytics'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenValue } from '../../utils/format'
import type { StatisticsMetric, StatisticsTrendPoint } from '../../types'

/** resolveChartTheme() 的返回类型（主题色表）。 */
export type TrendChartTheme = ReturnType<typeof resolveChartTheme>

/**
 * ECharts 按需注册（与桌面窗口其他图表组件一致：CanvasRenderer + LineChart +
 * GridComponent + TooltipComponent）。echarts 的 use() 对重复注册同一组件是安全的，
 * 此函数幂等：多个页面各自调用不会产生重复注册或副作用。
 */
export function registerChartComponents(): void {
  use([CanvasRenderer, LineChart, GridComponent, TooltipComponent])
}

/**
 * 主题颜色 computed：touch theme 设置（appearance/lightPalette/darkPalette）触发重算，
 * 并把 CSS 变量解析为具体颜色（canvas 渲染器无法解析 var(--x) 字符串）。
 * 每次调用创建独立 computed 实例，均在组件 setup 作用域内使用（不跨实例缓存，
 * 避免测试中 pinia 实例替换后引用旧 store）。
 */
export function useChartTheme(): ComputedRef<TrendChartTheme> {
  return computed(() => {
    const store = useMonitorStore()
    void store.settings.theme.appearance
    void store.settings.theme.lightPalette
    void store.settings.theme.darkPalette
    return resolveChartTheme()
  })
}

/** 趋势点取值：按指标口径返回 cost / totalTokens / requestCount。 */
export function metricValue(point: StatisticsTrendPoint, metric: StatisticsMetric): number {
  if (metric === 'cost') return point.cost
  if (metric === 'tokens') return point.totalTokens
  return point.requestCount
}

/** 指标值格式化：cost 用当前货币，tokens 用 token 缩写，requests 用请求数缩写。 */
export function formatMetric(metric: StatisticsMetric, value: number): string {
  const store = useMonitorStore()
  if (metric === 'cost') return formatCost(value, store.settings.currency)
  if (metric === 'tokens') return formatTokenValue(value)
  return formatRequestCount(value)
}

/** 指标名（i18n）：statistics.metricCost / metricTokens / metricRequests。 */
export function metricLabel(metric: StatisticsMetric): string {
  const store = useMonitorStore()
  if (metric === 'cost') return t(store.settings.locale, 'statistics.metricCost')
  if (metric === 'tokens') return t(store.settings.locale, 'statistics.metricTokens')
  return t(store.settings.locale, 'statistics.metricRequests')
}

/** 指标主题色：实时 touch theme 设置并解析（模板/事件回调中调用均可安全建立依赖）。 */
export function metricColor(metric: StatisticsMetric): string {
  const store = useMonitorStore()
  void store.settings.theme.appearance
  void store.settings.theme.lightPalette
  void store.settings.theme.darkPalette
  const colors = resolveChartTheme()
  if (metric === 'cost') return colors.cost
  if (metric === 'tokens') return colors.tokens
  return colors.requests
}

export interface TrendLineSeriesInput {
  name: string
  data: number[]
  color: string
  /** 主序列：2.5 宽 + 渐变 areaStyle；副序列默认 1.6 宽无面积（Analytics 对比序列传 width: 1.8 + dashed）。 */
  primary?: boolean
  /** 线宽覆盖（默认 primary ? 2.5 : 1.6）。 */
  width?: number
  /** 虚线（Analytics 上一周期对比序列）。 */
  dashed?: boolean
}

/**
 * 平滑折线系列构建：smooth、showSymbol: false、emphasis.focus 'series'，
 * 主序列带线性渐变 areaStyle（withAlpha '26' → '04'），副序列可选虚线。
 */
export function trendLineSeries(input: TrendLineSeriesInput) {
  const { name, data, color, primary = false, dashed = false } = input
  const width = input.width ?? (primary ? 2.5 : 1.6)
  return {
    name,
    type: 'line' as const,
    data,
    smooth: true,
    showSymbol: false,
    lineStyle: { width, color, ...(dashed ? { type: 'dashed' as const } : {}) },
    itemStyle: { color },
    emphasis: { focus: 'series' as const },
    ...(primary
      ? {
          areaStyle: {
            color: {
              type: 'linear' as const,
              x: 0,
              y: 0,
              x2: 0,
              y2: 1,
              colorStops: [
                { offset: 0, color: withAlpha(color, '26') },
                { offset: 1, color: withAlpha(color, '04') }
              ]
            }
          }
        }
      : {})
  }
}

export interface TrendChartOptionInput {
  points: StatisticsTrendPoint[]
  /** 主指标：决定 y 轴刻度格式化与主序列颜色（页面 tooltip 内也按此口径取值）。 */
  metric: StatisticsMetric
  colors: TrendChartTheme
  /** 页面自定义 tooltip 内容（两页 formatter 差异大，保留在页面内注入）。 */
  tooltipFormatter: (params: any) => string
  /** 页面构建的折线系列（多指标叠加 / 当前 + 对比周期等页面差异）。 */
  series: ReturnType<typeof trendLineSeries>[]
}

/**
 * 趋势图 option 骨架：grid / tooltip（基础样式）/ xAxis / yAxis 为两页完全一致的
 * 公共配置，series 与 tooltip formatter 由页面注入（差异参数化，不强行统一）。
 */
export function buildTrendChartOption(input: TrendChartOptionInput) {
  const { points, metric, colors, tooltipFormatter, series } = input
  return {
    grid: { left: 48, right: 12, top: 14, bottom: 22 },
    tooltip: {
      trigger: 'axis' as const,
      backgroundColor: colors.tooltipBg,
      borderColor: colors.tooltipBorder,
      borderRadius: 8,
      padding: [7, 9],
      textStyle: { color: colors.tooltipText, fontSize: 11 },
      formatter: tooltipFormatter
    },
    xAxis: {
      type: 'category' as const,
      data: points.map(p => p.label),
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
      axisLabel: {
        color: colors.axis,
        fontSize: 10,
        formatter: (value: number) => formatMetric(metric, value)
      },
      splitLine: { lineStyle: { type: 'dashed' as const, color: colors.grid } }
    },
    series
  }
}


