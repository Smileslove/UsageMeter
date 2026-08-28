/**
 * 趋势指标工具函数：取值 / 格式化 / 标签 / 颜色。
 * 注意：metricValue/trendLineSeries/buildTrendChartOption 是纯函数；
 * formatMetric/metricLabel/metricColor 内部调用 useMonitorStore() 获取设置，
 * 应在 computed 或模板内调用以建立响应式依赖。
 */
import { useMonitorStore } from '../../stores/monitor'
import { resolveChartTheme, withAlpha } from '../../utils/chartTheme'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenValue } from '../../utils/format'
import type { StatisticsMetric, StatisticsTrendPoint } from '../../types'

export function metricValue(point: StatisticsTrendPoint, metric: StatisticsMetric): number {
  if (metric === 'cost') return point.cost
  if (metric === 'tokens') return point.totalTokens
  return point.requestCount
}

export function formatMetric(metric: StatisticsMetric, value: number): string {
  const store = useMonitorStore()
  if (metric === 'cost') return formatCost(value, store.settings.currency)
  if (metric === 'tokens') return formatTokenValue(value)
  return formatRequestCount(value)
}

export function metricLabel(metric: StatisticsMetric): string {
  const store = useMonitorStore()
  if (metric === 'cost') return t(store.settings.locale, 'statistics.metricCost')
  if (metric === 'tokens') return t(store.settings.locale, 'statistics.metricTokens')
  return t(store.settings.locale, 'statistics.metricRequests')
}

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
  primary?: boolean
  width?: number
  dashed?: boolean
}

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
              x: 0, y: 0, x2: 0, y2: 1,
              colorStops: [
                { offset: 0, color: withAlpha(color, '26') },
                { offset: 1, color: withAlpha(color, '04') },
              ],
            },
          },
        }
      : {}),
  }
}

export interface TrendChartOptionInput {
  points: StatisticsTrendPoint[]
  metric: StatisticsMetric
  colors: ReturnType<typeof resolveChartTheme>
  tooltipFormatter: (params: any) => string
  series: ReturnType<typeof trendLineSeries>[]
}

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
      formatter: tooltipFormatter,
    },
    xAxis: {
      type: 'category' as const,
      data: points.map(p => p.label),
      boundaryGap: false,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: colors.axis, fontSize: 10, hideOverlap: true, margin: 8 },
    },
    yAxis: {
      type: 'value' as const,
      min: 0,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: {
        color: colors.axis,
        fontSize: 10,
        formatter: (value: number) => formatMetric(metric, value),
      },
      splitLine: { lineStyle: { type: 'dashed' as const, color: colors.grid } },
    },
    series,
  }
}
