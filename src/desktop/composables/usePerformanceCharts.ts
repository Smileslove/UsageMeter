/**
 * 性能视图（设计 7.7）图表配置逻辑：
 * TTFT 与生成速率双时序图（上下两个独立坐标区，按时间正序）。
 *
 * 输入：请求记录 ref + locale ref；输出响应式时序数据与 ECharts option。
 * 复用 useTrendChart 的 trendLineSeries（平滑折线 + 渐变面积）与 useChartTheme（主题色）。
 */
import { computed } from 'vue'
import type { Ref } from 'vue'
import { trendLineSeries, useChartTheme } from './useTrendChart'
import { t } from '../../i18n'
import { formatRate } from '../../utils/format'
import type { RequestRecord } from '../../types'

/** 时序图数据点。 */
export interface TimelinePoint {
  time: string
  full: string
  value: number
}

export function usePerformanceCharts(
  records: Ref<RequestRecord[]>,
  locale: Ref<string>,
) {
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
      second: '2-digit',
    })
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
    seriesName: string,
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
        },
      },
      xAxis: {
        type: 'category' as const,
        data: data.map(d => d.time),
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
        axisLabel: { color: colors.axis, fontSize: 10, formatter: (v: number) => valueFormatter(v) },
        splitLine: { lineStyle: { type: 'dashed' as const, color: colors.grid } },
      },
      series: [trendLineSeries({ name: seriesName, data: data.map(d => d.value), color, primary: true })],
    }
  }

  const ttftAxisFormatter = (v: number) => (v >= 1000 ? `${(v / 1000).toFixed(1)}s` : `${Math.round(v)}ms`)
  const rateAxisFormatter = (v: number) => `${formatRate(v)}t/s`

  const ttftChartOption = computed(() =>
    timelineOption(ttftSeries.value, chartTheme.value.requests, ttftAxisFormatter, t(locale.value, 'desktop.analytics.perfTtftChart')),
  )
  const rateChartOption = computed(() =>
    timelineOption(rateSeries.value, chartTheme.value.tokens, rateAxisFormatter, t(locale.value, 'desktop.analytics.perfRateChart')),
  )

  return {
    chartTimeLabel,
    chartFullLabel,
    ttftSeries,
    rateSeries,
    timelineOption,
    ttftChartOption,
    rateChartOption,
  }
}
