<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { LineChart, PieChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import VChart from 'vue-echarts'
import { t } from '../../i18n'
import { formatCost, formatDurationMs, formatRate, formatRequestCount, formatTokenPair, formatTokenValue } from '../../utils/format'
import type { AppLocale, StatisticsModelBreakdown, StatisticsTrendPoint } from '../../types'

use([CanvasRenderer, PieChart, LineChart, GridComponent, TooltipComponent])

const props = defineProps<{
  locale: AppLocale
  models: StatisticsModelBreakdown[]
}>()

const selectedModelIndex = ref(0)
const focusedTrendMetric = ref<'requests' | 'tokens' | 'cost' | 'rate' | null>(null)

const chartColors = ['#2DD4BF', '#818CF8', '#FBBF24', '#F45B69', '#60A5FA', '#A78BFA']
const trendColors = {
  requests: '#10B981',
  tokens: '#3B82F6',
  cost: '#F59E0B',
  rate: '#8B5CF6'
}

const rankedModels = computed(() => {
  return [...props.models].filter(model => model.percent > 0).sort((a, b) => b.percent - a.percent)
})

const selectedModel = computed(() => {
  return rankedModels.value[selectedModelIndex.value] ?? rankedModels.value[0] ?? null
})

const selectedIndex = computed(() => {
  return selectedModel.value ? Math.max(0, Math.min(selectedModelIndex.value, rankedModels.value.length - 1)) : -1
})

const tokenPair = computed(() => {
  if (!selectedModel.value) return { input: '-', output: '-' }
  return formatTokenPair(selectedModel.value.inputTokens, selectedModel.value.outputTokens)
})

const avgSpeed = computed(() => {
  const value = selectedModel.value?.avgTokensPerSecond
  if (!value) return '-'
  return `${formatRate(value)} ${t(props.locale, 'metrics.tokensPerSecond')}`
})

const avgTtft = computed(() => {
  const value = selectedModel.value?.avgTtftMs
  if (!value) return '-'
  return formatDurationMs(value)
})

const selectedStatusCodes = computed(() => {
  return [...(selectedModel.value?.statusCodes ?? [])].sort((a, b) => a.statusCode - b.statusCode)
})

const successRequests = computed(() => selectedModel.value?.successRequests ?? 0)
const failedRequests = computed(() => selectedModel.value?.errorRequests ?? 0)

function trendValue(point: StatisticsTrendPoint, metric: 'requests' | 'tokens' | 'cost' | 'rate'): number {
  if (metric === 'requests') return point.requestCount
  if (metric === 'tokens') return point.totalTokens
  if (metric === 'cost') return point.cost
  return point.avgTokensPerSecond ?? 0
}

function formatTrendValue(metric: 'requests' | 'tokens' | 'cost' | 'rate', value: number): string {
  if (metric === 'requests') return formatRequestCount(value)
  if (metric === 'tokens') return formatTokenValue(value)
  if (metric === 'cost') return formatCost(value)
  return `${formatRate(value)} ${t(props.locale, 'metrics.tokensPerSecond')}`
}

function trendMetricLabel(metric: 'requests' | 'tokens' | 'cost' | 'rate'): string {
  if (metric === 'requests') return t(props.locale, 'statistics.metricRequests')
  if (metric === 'tokens') return t(props.locale, 'statistics.metricTokens')
  if (metric === 'cost') return t(props.locale, 'statistics.metricCost')
  return t(props.locale, 'statistics.avgSpeed')
}

function normalizedTrend(metric: 'requests' | 'tokens' | 'cost' | 'rate'): number[] {
  const values = selectedModel.value?.trend.map(point => trendValue(point, metric)) ?? []
  const max = Math.max(...values, 0)
  if (max <= 0) return values.map(() => 0)
  return values.map(value => Number(((value / max) * 100).toFixed(2)))
}

function maxTrendValue(metric: 'requests' | 'tokens' | 'cost' | 'rate'): number {
  return Math.max(...(selectedModel.value?.trend.map(point => trendValue(point, metric)) ?? []), 0)
}

function yAxisLabel(value: number): string {
  if (value !== 0 && value !== 50 && value !== 100) return ''
  if (!focusedTrendMetric.value) return `${value}%`
  return formatTrendValue(focusedTrendMetric.value, maxTrendValue(focusedTrendMetric.value) * (value / 100))
}

function handleTrendMouseover(params: any) {
  const metric = params?.seriesName as 'requests' | 'tokens' | 'cost' | 'rate' | undefined
  if (metric === 'requests' || metric === 'tokens' || metric === 'cost' || metric === 'rate') {
    focusedTrendMetric.value = metric
  }
}

const chartOptions = computed(() => {
  const data = rankedModels.value.map((model, index) => ({
    name: model.modelName,
    value: Number(model.percent.toFixed(2)),
    requestCount: model.requestCount,
    cost: model.cost,
    itemStyle: {
      color: chartColors[index % chartColors.length],
      opacity: selectedIndex.value < 0 || index === selectedIndex.value ? 1 : 0.32
    }
  }))

  return {
    color: chartColors,
    tooltip: {
      trigger: 'item',
      backgroundColor: '#ffffff',
      borderColor: '#E5E7EB',
      borderRadius: 8,
      padding: [7, 9],
      textStyle: { color: '#374151', fontSize: 11 },
      formatter: (params: any) => {
        const model = rankedModels.value[params.dataIndex]
        if (!model) return ''
        return `
          <div style="font-weight:600;margin-bottom:5px;">${model.modelName}</div>
          <div style="display:flex;justify-content:space-between;gap:16px;"><span>${t(props.locale, 'statistics.modelShare')}</span><b>${model.percent.toFixed(1)}%</b></div>
          <div style="display:flex;justify-content:space-between;gap:16px;"><span>${t(props.locale, 'statistics.requests')}</span><b>${formatRequestCount(model.requestCount)}</b></div>
          <div style="display:flex;justify-content:space-between;gap:16px;"><span>${t(props.locale, 'statistics.cost')}</span><b>${formatCost(model.cost)}</b></div>
        `
      }
    },
    series: [
      {
        type: 'pie',
        radius: ['60%', '82%'],
        center: ['50%', '50%'],
        avoidLabelOverlap: true,
        padAngle: 3,
        minAngle: 4,
        itemStyle: {
          borderRadius: 5,
          borderColor: '#FFFFFF',
          borderWidth: 2
        },
        emphasis: {
          scale: true,
          scaleSize: 4,
          itemStyle: {
            shadowBlur: 8,
            shadowColor: 'rgba(0,0,0,0.08)'
          }
        },
        label: { show: false },
        labelLine: { show: false },
        data
      }
    ]
  }
})

const trendOptions = computed(() => {
  const points = selectedModel.value?.trend ?? []
  const metrics: Array<'requests' | 'tokens' | 'cost' | 'rate'> = ['requests', 'tokens', 'cost', 'rate']

  return {
    grid: { left: 20, right: 8, top: 10, bottom: 18, containLabel: false },
    tooltip: {
      trigger: 'axis',
      backgroundColor: '#ffffff',
      borderColor: '#E5E7EB',
      borderRadius: 8,
      padding: [7, 9],
      textStyle: { color: '#374151', fontSize: 11 },
      formatter: (params: any) => {
        const point = points[params[0]?.dataIndex]
        if (!point) return ''
        const rows = metrics
          .map(metric => {
            const color = trendColors[metric]
            const value = formatTrendValue(metric, trendValue(point, metric))
            return `<div style="display:flex;align-items:center;gap:6px;margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${color};"></span><span>${trendMetricLabel(metric)}: <b>${value}</b></span></div>`
          })
          .join('')
        return `<div style="font-weight:600;margin-bottom:4px;">${point.label}</div>${rows}`
      }
    },
    xAxis: {
      type: 'category',
      data: points.map(point => point.label),
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
    series: metrics.map(metric => ({
      name: metric,
      type: 'line',
      data: normalizedTrend(metric),
      smooth: true,
      showSymbol: true,
      symbolSize: 4,
      lineStyle: {
        width: focusedTrendMetric.value === metric ? 2.8 : 1.7,
        color: trendColors[metric],
        opacity: !focusedTrendMetric.value || focusedTrendMetric.value === metric ? 1 : 0.22
      },
      itemStyle: { color: trendColors[metric], borderColor: '#FFFFFF', borderWidth: 1.5 },
      areaStyle: {
        color: {
          type: 'linear',
          x: 0,
          y: 0,
          x2: 0,
          y2: 1,
          colorStops: [
            { offset: 0, color: `${trendColors[metric]}22` },
            { offset: 1, color: `${trendColors[metric]}04` }
          ]
        },
        opacity: !focusedTrendMetric.value || focusedTrendMetric.value === metric ? 1 : 0.16
      },
      emphasis: { focus: 'series' }
    }))
  }
})

function selectModel(index: number) {
  selectedModelIndex.value = index
}

function handleChartClick(params: any) {
  if (typeof params?.dataIndex === 'number' && rankedModels.value[params.dataIndex]) {
    selectModel(params.dataIndex)
  }
}

watch(
  rankedModels,
  models => {
    if (!models.length) {
      selectedModelIndex.value = 0
      return
    }
    if (selectedModelIndex.value >= models.length) {
      selectedModelIndex.value = 0
    }
  },
  { immediate: true }
)
</script>

<template>
  <section class="rounded-xl bg-gray-50 p-3 dark:bg-neutral-800/70">
    <div class="mb-3 flex items-center justify-between">
      <p class="text-[11px] font-semibold text-gray-600 dark:text-gray-300">{{ t(locale, 'statistics.models') }}</p>
      <p class="text-[10px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.modelsUsed') }} {{ rankedModels.length }}</p>
    </div>

    <div v-if="rankedModels.length && selectedModel" class="space-y-2">
      <div class="flex max-w-full gap-1.5 overflow-x-auto overscroll-x-contain pb-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
        <button
          v-for="(model, index) in rankedModels.slice(0, 6)"
          :key="`${index}-${model.modelName}`"
          type="button"
          class="flex w-[124px] shrink-0 items-center gap-1.5 rounded-full border px-2 py-1.5 text-left transition focus:outline-none"
          :aria-pressed="selectedModelIndex === index"
          :class="
            selectedModelIndex === index
              ? 'border-gray-300 bg-white shadow-[0_2px_8px_rgba(15,23,42,0.10)] ring-1 ring-gray-200 dark:border-neutral-500 dark:bg-neutral-700 dark:ring-neutral-600'
              : 'border-transparent bg-white/50 opacity-65 shadow-none ring-0 hover:opacity-90 dark:bg-neutral-700/30'
          "
          @click="selectModel(index)"
        >
          <span class="h-2 w-2 shrink-0 rounded-full" :style="{ backgroundColor: chartColors[index % chartColors.length] }" />
          <span class="min-w-0 flex-1">
            <span
              class="block truncate text-[10px] font-semibold leading-tight"
              :class="selectedModelIndex === index ? 'text-gray-700 dark:text-gray-100' : 'text-gray-500 dark:text-gray-400'"
            >
              {{ model.modelName }}
            </span>
            <span class="block font-mono text-[9px] leading-tight text-gray-400 dark:text-gray-500">{{ formatRequestCount(model.requestCount) }} {{ t(locale, 'statistics.requests') }}</span>
          </span>
        </button>
      </div>

      <div class="rounded-xl bg-white p-2.5 dark:bg-neutral-700/80">
        <div class="grid grid-cols-[minmax(0,1fr)_126px] gap-2.5">
          <div class="min-w-0">
            <div class="grid grid-cols-2 gap-1.5">
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-[9px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.cost') }}</p>
              <p class="font-mono text-[11px] font-bold text-gray-800 dark:text-gray-100">{{ formatCost(selectedModel.cost) }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-[9px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.metricTokens') }}</p>
              <p class="font-mono text-[11px] font-bold text-gray-800 dark:text-gray-100">{{ formatTokenValue(selectedModel.totalTokens) }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-[9px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.avgTtft') }}</p>
              <p class="font-mono text-[11px] font-bold text-gray-800 dark:text-gray-100">{{ avgTtft }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-[9px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.avgSpeed') }}</p>
              <p class="truncate font-mono text-[11px] font-bold text-gray-800 dark:text-gray-100">{{ avgSpeed }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-[9px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.successRequests') }}</p>
              <p class="truncate font-mono text-[11px] font-semibold text-emerald-600 dark:text-emerald-300">{{ formatRequestCount(successRequests) }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-[9px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.errorRequests') }}</p>
              <p class="truncate font-mono text-[11px] font-semibold text-rose-600 dark:text-rose-300">{{ formatRequestCount(failedRequests) }}</p>
            </div>
          </div>
          </div>

          <div class="min-w-0">
            <div class="relative mx-auto h-[124px] w-[124px]">
              <v-chart class="h-full w-full" :option="chartOptions" autoresize @click="handleChartClick" />
              <div class="pointer-events-none absolute inset-0 grid place-items-center">
                <div class="text-center">
                  <p class="font-mono text-[18px] font-bold leading-none text-gray-900 dark:text-gray-100">{{ selectedModel.percent.toFixed(0) }}%</p>
                  <p class="mt-0.5 text-[9px] font-semibold uppercase text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.modelShare') }}</p>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div class="mt-2 grid grid-cols-4 gap-1.5 text-[10px]">
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.input') }}</p>
              <p class="truncate font-mono font-semibold text-gray-700 dark:text-gray-200">{{ tokenPair.input }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.output') }}</p>
              <p class="truncate font-mono font-semibold text-gray-700 dark:text-gray-200">{{ tokenPair.output }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.cacheCreate') }}</p>
              <p class="truncate font-mono font-semibold text-gray-700 dark:text-gray-200">{{ formatTokenValue(selectedModel.cacheCreateTokens) }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.cacheRead') }}</p>
              <p class="truncate font-mono font-semibold text-gray-700 dark:text-gray-200">{{ formatTokenValue(selectedModel.cacheReadTokens) }}</p>
            </div>
          </div>

        <div class="mt-2 rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
            <div class="mb-1 flex items-center justify-between">
              <p class="text-[10px] font-semibold text-gray-500 dark:text-gray-300">{{ t(locale, 'statistics.statusCodeDetail') }}</p>
              <p class="font-mono text-[10px] text-gray-400 dark:text-gray-500">{{ formatRequestCount(selectedModel.requestCount) }}</p>
            </div>
            <div v-if="selectedStatusCodes.length" class="flex flex-wrap gap-1">
              <span
                v-for="item in selectedStatusCodes"
                :key="item.statusCode"
                class="rounded-full px-1.5 py-0.5 font-mono text-[10px] font-semibold"
                :class="
                  item.statusCode < 400
                    ? 'bg-emerald-50 text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300'
                    : item.statusCode < 500
                      ? 'bg-amber-50 text-amber-700 dark:bg-amber-950/40 dark:text-amber-300'
                      : 'bg-rose-50 text-rose-700 dark:bg-rose-950/40 dark:text-rose-300'
                "
              >
                {{ item.statusCode }} · {{ formatRequestCount(item.count) }}
              </span>
            </div>
            <div v-else class="text-[10px] text-gray-400 dark:text-gray-500">
              {{ t(locale, 'statistics.noStatusCodeData') }}
            </div>
          </div>
      </div>

      <div class="rounded-xl bg-white p-2 dark:bg-neutral-700/80">
        <div class="mb-1.5 flex items-center justify-between">
          <p class="text-[10px] font-semibold text-gray-500 dark:text-gray-300">{{ t(locale, 'statistics.modelTrend') }}</p>
          <div class="flex items-center gap-1">
            <button
              v-for="metric in (['requests', 'tokens', 'cost', 'rate'] as const)"
              :key="metric"
              type="button"
              class="flex items-center gap-0.5 rounded-full px-1 py-0.5 text-[9px] font-semibold transition focus:outline-none"
              :class="focusedTrendMetric === metric ? 'bg-gray-100 text-gray-700 dark:bg-neutral-800 dark:text-gray-200' : focusedTrendMetric ? 'text-gray-300 dark:text-gray-600' : 'text-gray-400 dark:text-gray-500'"
              @mouseenter="focusedTrendMetric = metric"
              @focus="focusedTrendMetric = metric"
              @click="focusedTrendMetric = focusedTrendMetric === metric ? null : metric"
            >
              <span class="h-1.5 w-1.5 rounded-full" :style="{ backgroundColor: trendColors[metric] }" />
              {{ trendMetricLabel(metric) }}
            </button>
          </div>
        </div>
        <div v-if="selectedModel.trend.length" class="h-[116px]">
          <v-chart class="h-full w-full" :option="trendOptions" autoresize @mouseover="handleTrendMouseover" />
        </div>
        <div v-else class="grid h-[116px] place-items-center text-[11px] text-gray-400 dark:text-gray-500">
          {{ t(locale, 'statistics.noData') }}
        </div>
      </div>
    </div>

    <div v-else class="py-5 text-center text-[11px] text-gray-400 dark:text-gray-500">
      {{ t(locale, 'statistics.noData') }}
    </div>
  </section>
</template>
