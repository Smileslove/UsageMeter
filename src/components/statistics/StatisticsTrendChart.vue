<script setup lang="ts">
import { computed, ref } from 'vue'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { LineChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import VChart from 'vue-echarts'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenValue } from '../../utils/format'
import type { AppLocale, StatisticsMetric, StatisticsTrendPoint } from '../../types'
import { useMonitorStore } from '../../stores/monitor'
import { themeColorVar } from '../../theme'

const store = useMonitorStore()

use([CanvasRenderer, LineChart, GridComponent, TooltipComponent])

const props = defineProps<{
  locale: AppLocale
  metric: StatisticsMetric
  points: StatisticsTrendPoint[]
}>()

const emit = defineEmits<{
  setMetric: [value: StatisticsMetric]
}>()

const trendMetrics: Array<{ value: StatisticsMetric; color: string; key: string }> = [
  { value: 'requests', color: themeColorVar('--theme-chart-requests'), key: 'statistics.metricRequests' },
  { value: 'tokens', color: themeColorVar('--theme-chart-tokens'), key: 'statistics.metricTokens' },
  { value: 'cost', color: themeColorVar('--theme-chart-cost'), key: 'statistics.metricCost' }
]

// Local, nullable highlight: when nothing is picked, all series are shown.
// Clicking a chip toggles it; clicking it again clears back to "show all".
const selectedMetric = ref<StatisticsMetric | null>(null)
const hoveredMetric = ref<StatisticsMetric | null>(null)
const activeMetric = computed(() => hoveredMetric.value ?? selectedMetric.value)

function toggleMetric(metric: StatisticsMetric) {
  selectedMetric.value = selectedMetric.value === metric ? null : metric
  if (selectedMetric.value) emit('setMetric', selectedMetric.value)
}

// ECharts' canvas renderer cannot resolve `var(--x)` strings — and building a
// gradient with an invalid colorStop (e.g. `var(--x)26`) throws and aborts the
// whole chart render. Resolve theme variables to concrete colors, recomputing
// whenever the theme selection changes so the chart still adapts to themes.
function resolveCssVar(name: string): string {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  return value || '#7c8aa0'
}

function withAlpha(color: string, alphaHex: string): string {
  return /^#[0-9a-fA-F]{6}$/.test(color) ? `${color}${alphaHex}` : color
}

const themeColors = computed(() => {
  // Touch theme settings so this re-resolves on appearance / palette changes.
  void store.settings.theme.appearance
  void store.settings.theme.lightPalette
  void store.settings.theme.darkPalette
  return {
    requests: resolveCssVar('--theme-chart-requests'),
    tokens: resolveCssVar('--theme-chart-tokens'),
    cost: resolveCssVar('--theme-chart-cost'),
    axis: resolveCssVar('--theme-chart-axis'),
    grid: resolveCssVar('--theme-chart-grid'),
    elevated: resolveCssVar('--theme-bg-elevated'),
    tooltipBg: resolveCssVar('--theme-chart-tooltip-bg'),
    tooltipBorder: resolveCssVar('--theme-chart-tooltip-border'),
    tooltipText: resolveCssVar('--theme-chart-tooltip-text')
  } as Record<StatisticsMetric | 'axis' | 'grid' | 'elevated' | 'tooltipBg' | 'tooltipBorder' | 'tooltipText', string>
})

function valueOf(point: StatisticsTrendPoint, metric: StatisticsMetric): number {
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
  if (metric === 'cost') return t(props.locale, 'statistics.metricCost')
  if (metric === 'tokens') return t(props.locale, 'statistics.metricTokens')
  return t(props.locale, 'statistics.metricRequests')
}

function normalizedValues(metric: StatisticsMetric): number[] {
  const values = props.points.map(point => valueOf(point, metric))
  const max = Math.max(...values, 0)
  if (max <= 0) return values.map(() => 0)
  return values.map(value => Number(((value / max) * 100).toFixed(2)))
}

function maxValue(metric: StatisticsMetric): number {
  return Math.max(...props.points.map(point => valueOf(point, metric)), 0)
}

function yAxisLabel(value: number): string {
  if (value !== 0 && value !== 50 && value !== 100) return ''
  if (!activeMetric.value) return `${value}%`
  return formatMetric(activeMetric.value, maxValue(activeMetric.value) * (value / 100))
}

const chartOptions = computed(() => {
  return {
    grid: { left: 26, right: 8, top: 10, bottom: 18, containLabel: false },
    tooltip: {
      trigger: 'axis',
      backgroundColor: themeColors.value.tooltipBg,
      borderColor: themeColors.value.tooltipBorder,
      borderRadius: 8,
      padding: [7, 9],
      textStyle: { color: themeColors.value.tooltipText, fontSize: 11 },
      formatter: (params: any) => {
        const point = props.points[params[0].dataIndex]
        const rows = trendMetrics.map(item => {
          const value = formatMetric(item.value, valueOf(point, item.value))
          return `<div style="display:flex;align-items:center;gap:6px;margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${themeColors.value[item.value]};"></span><span>${metricLabel(item.value)}: <b>${value}</b></span></div>`
        }).join('')
        return `<div style="font-weight:600;margin-bottom:4px;">${point.label}</div>${rows}`
      }
    },
    xAxis: {
      type: 'category',
      data: props.points.map(p => p.label),
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: themeColors.value.axis, fontSize: 9, hideOverlap: true, margin: 8 }
    },
    yAxis: {
      type: 'value',
      min: 0,
      max: 100,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: {
        color: themeColors.value.axis,
        fontSize: 9,
        formatter: yAxisLabel
      },
      splitNumber: 2,
      splitLine: { lineStyle: { type: 'dashed', color: themeColors.value.grid } }
    },
    series: trendMetrics.map(item => {
      const color = themeColors.value[item.value]
      const isActive = activeMetric.value === item.value
      // No selection → every series is shown at full strength.
      const dimmed = activeMetric.value !== null && !isActive
      return {
        name: item.value,
        type: 'line',
        data: normalizedValues(item.value),
        smooth: true,
        showSymbol: true,
        symbolSize: isActive ? 5 : 4,
        lineStyle: {
          width: isActive ? 2.8 : 1.8,
          color,
          opacity: dimmed ? 0.22 : 1
        },
        itemStyle: { color, borderColor: themeColors.value.elevated, borderWidth: 1.5 },
        areaStyle: {
          color: {
            type: 'linear',
            x: 0,
            y: 0,
            x2: 0,
            y2: 1,
            colorStops: [
              { offset: 0, color: withAlpha(color, '26') },
              { offset: 1, color: withAlpha(color, '04') }
            ]
          },
          opacity: dimmed ? 0.18 : 1
        }
      }
    })
  }
})
</script>

<template>
  <section class="trend-chart rounded-xl p-3">
    <div class="mb-2 flex items-center justify-between gap-2">
      <p class="text-[11px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'statistics.trend') }}</p>
      <div class="trend-seg">
        <button
          v-for="item in trendMetrics"
          :key="item.value"
          type="button"
          class="trend-seg__item"
          :class="{ 'trend-seg__item--on': activeMetric === item.value }"
          @mouseenter="hoveredMetric = item.value"
          @mouseleave="hoveredMetric = null"
          @click="toggleMetric(item.value)"
        >
          <span class="trend-seg__dot" :style="{ backgroundColor: item.color }" />
          <span class="trend-seg__label">{{ metricLabel(item.value) }}</span>
        </button>
      </div>
    </div>
    <div v-if="points.length" class="h-[138px]">
      <v-chart class="h-full w-full" :option="chartOptions" autoresize />
    </div>
    <div v-else class="grid h-[138px] place-items-center text-[11px] text-[var(--theme-text-tertiary)]">
      {{ t(locale, 'statistics.noData') }}
    </div>
  </section>
</template>

<style scoped>
.trend-chart {
  background: var(--theme-surface-muted-gradient);
}

</style>
