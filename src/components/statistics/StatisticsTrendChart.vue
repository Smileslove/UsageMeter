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
  { value: 'requests', color: '#10B981', key: 'statistics.metricRequests' },
  { value: 'tokens', color: '#3B82F6', key: 'statistics.metricTokens' },
  { value: 'cost', color: '#F59E0B', key: 'statistics.metricCost' }
]

const hoveredMetric = ref<StatisticsMetric | null>(null)
const activeMetric = computed(() => hoveredMetric.value ?? props.metric)

function valueOf(point: StatisticsTrendPoint, metric: StatisticsMetric): number {
  if (metric === 'cost') return point.cost
  if (metric === 'tokens') return point.totalTokens
  return point.requestCount
}

function formatMetric(metric: StatisticsMetric, value: number): string {
  if (metric === 'cost') return formatCost(value)
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
  if (!hoveredMetric.value) return `${value}%`
  return formatMetric(hoveredMetric.value, maxValue(hoveredMetric.value) * (value / 100))
}

function handleChartMouseover(params: any) {
  const metric = params?.seriesName as StatisticsMetric | undefined
  if (metric === 'requests' || metric === 'tokens' || metric === 'cost') {
    hoveredMetric.value = metric
  }
}

function handleChartMouseout() {
  hoveredMetric.value = null
}

const chartOptions = computed(() => {
  return {
    grid: { left: 26, right: 8, top: 10, bottom: 18, containLabel: false },
    tooltip: {
      trigger: 'axis',
      backgroundColor: '#ffffff',
      borderColor: '#E5E7EB',
      borderRadius: 8,
      padding: [7, 9],
      textStyle: { color: '#374151', fontSize: 11 },
      formatter: (params: any) => {
        const point = props.points[params[0].dataIndex]
        const rows = trendMetrics.map(item => {
          const value = formatMetric(item.value, valueOf(point, item.value))
          return `<div style="display:flex;align-items:center;gap:6px;margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${item.color};"></span><span>${metricLabel(item.value)}: <b>${value}</b></span></div>`
        }).join('')
        return `<div style="font-weight:600;margin-bottom:4px;">${point.label}</div>${rows}`
      }
    },
    xAxis: {
      type: 'category',
      data: props.points.map(p => p.label),
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: '#9CA3AF', fontSize: 9, hideOverlap: true, margin: 8 }
    },
    yAxis: {
      type: 'value',
      min: 0,
      max: 100,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: {
        color: '#9CA3AF',
        fontSize: 9,
        formatter: yAxisLabel
      },
      splitNumber: 2,
      splitLine: { lineStyle: { type: 'dashed', color: '#E5E7EB' } }
    },
    series: trendMetrics.map(item => ({
      name: item.value,
      type: 'line',
      data: normalizedValues(item.value),
      smooth: true,
      showSymbol: true,
      symbolSize: 5,
      lineStyle: {
        width: activeMetric.value === item.value ? 2.8 : 1.8,
        color: item.color,
        opacity: activeMetric.value === item.value ? 1 : 0.22
      },
      itemStyle: { color: item.color, borderColor: '#FFFFFF', borderWidth: 1.5 },
      areaStyle: {
        color: {
          type: 'linear',
          x: 0,
          y: 0,
          x2: 0,
          y2: 1,
          colorStops: [
            { offset: 0, color: `${item.color}26` },
            { offset: 1, color: `${item.color}04` }
          ]
        },
        opacity: activeMetric.value === item.value ? 1 : 0.18
      },
      emphasis: { focus: 'series' }
    }))
  }
})
</script>

<template>
  <section class="rounded-xl bg-gray-50 p-3 dark:bg-neutral-800/70">
    <div class="mb-2 flex items-center justify-between gap-2">
      <p class="text-[11px] font-semibold text-gray-600 dark:text-gray-300">{{ t(locale, 'statistics.trend') }}</p>
      <div class="flex min-w-0 items-center gap-1">
        <div
          v-for="item in trendMetrics"
          :key="item.value"
          class="flex min-w-0 items-center gap-1 rounded-full bg-white px-1.5 py-0.5 text-[9px] font-semibold text-gray-500 transition dark:bg-neutral-700/80 dark:text-gray-300"
          :class="activeMetric === item.value ? 'ring-1 ring-gray-300 dark:ring-neutral-500' : 'opacity-45'"
          @mouseenter="hoveredMetric = item.value"
          @mouseleave="hoveredMetric = null"
          @click="emit('setMetric', item.value)"
        >
          <span class="h-1.5 w-1.5 shrink-0 rounded-full" :style="{ backgroundColor: item.color }" />
          <span class="shrink-0">{{ metricLabel(item.value) }}</span>
        </div>
      </div>
    </div>
    <div v-if="points.length" class="h-[138px]">
      <v-chart class="h-full w-full" :option="chartOptions" autoresize @mouseover="handleChartMouseover" @mouseout="handleChartMouseout" />
    </div>
    <div v-else class="grid h-[138px] place-items-center text-[11px] text-gray-400 dark:text-gray-500">
      {{ t(locale, 'statistics.noData') }}
    </div>
  </section>
</template>
