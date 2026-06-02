<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { LineChart, PieChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import VChart from 'vue-echarts'
import { Check, ChevronDown } from 'lucide-vue-next'
import { t } from '../../i18n'
import { formatCost, formatDurationMs, formatRate, formatRequestCount, formatTokenValue } from '../../utils/format'
import type { AppLocale, StatisticsModelBreakdown, StatisticsTrendPoint } from '../../types'
import { useMonitorStore } from '../../stores/monitor'

const store = useMonitorStore()

use([CanvasRenderer, PieChart, LineChart, GridComponent, TooltipComponent])

const props = defineProps<{
  locale: AppLocale
  models: StatisticsModelBreakdown[]
}>()

const selectedModelName = ref<string | null>(null)
const menuOpen = ref(false)
const selectRef = ref<HTMLElement | null>(null)
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

const displayModels = computed(() => rankedModels.value.slice(0, 6))

const selectedModel = computed(() => {
  if (!rankedModels.value.length) return null
  return rankedModels.value.find(model => model.modelName === selectedModelName.value) ?? rankedModels.value[0]
})

// Index of the selected model within the donut's top-N slice (-1 if not charted).
const selectedDisplayIndex = computed(() => {
  if (!selectedModel.value) return -1
  return displayModels.value.findIndex(model => model.modelName === selectedModel.value!.modelName)
})

function modelColor(modelName: string): string {
  const index = displayModels.value.findIndex(model => model.modelName === modelName)
  return index >= 0 ? chartColors[index % chartColors.length] : 'var(--theme-text-quaternary)'
}

const tokenPair = computed(() => {
  if (!selectedModel.value) return { input: '-', output: '-' }
  return {
    input: formatTokenValue(selectedModel.value.inputTokens),
    output: formatTokenValue(selectedModel.value.outputTokens),
  }
})

const cacheHitRate = computed(() => {
  const m = selectedModel.value
  if (!m) return null
  const total = m.inputTokens + m.cacheCreateTokens + m.cacheReadTokens
  if (total === 0) return null
  return (m.cacheReadTokens / total) * 100
})

const cacheHitRateDisplay = computed(() => {
  if (cacheHitRate.value === null) return '—'
  return `${cacheHitRate.value.toFixed(1)}%`
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

const localRequestCount = computed(() => selectedModel.value?.localRequestCount ?? 0)

const successRequests = computed(() => selectedModel.value?.successRequests ?? 0)
const failedRequests = computed(() => selectedModel.value?.errorRequests ?? 0)
const hasPerformanceTag = computed(() => displayModels.value.length > 0)
const hasPartialPerformanceCoverage = computed(() => store.snapshot?.note?.includes('NOTE_PARTIAL_PROXY_COVERAGE') ?? false)
const performanceTagTitle = computed(() => (
  hasPartialPerformanceCoverage.value
    ? t(props.locale, 'overview.proxyStatusPartial')
    : t(props.locale, 'overview.proxyStatusHint')
))

function metricValueSizeClass(text: string): string {
  const length = text.length
  if (length >= 12) return 'text-[9px]'
  if (length >= 10) return 'text-[10px]'
  return 'text-[11px]'
}

function trendValue(point: StatisticsTrendPoint, metric: 'requests' | 'tokens' | 'cost' | 'rate'): number {
  if (metric === 'requests') return point.requestCount
  if (metric === 'tokens') return point.totalTokens
  if (metric === 'cost') return point.cost
  return point.avgTokensPerSecond ?? 0
}

function formatTrendValue(metric: 'requests' | 'tokens' | 'cost' | 'rate', value: number): string {
  if (metric === 'requests') return formatRequestCount(value)
  if (metric === 'tokens') return formatTokenValue(value)
  if (metric === 'cost') return formatCost(value, store.settings.currency)
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

const chartOptions = computed(() => {
  const data = displayModels.value.map((model, index) => ({
    name: model.modelName,
    value: Number(model.percent.toFixed(2)),
    requestCount: model.requestCount,
    cost: model.cost,
    itemStyle: {
      color: chartColors[index % chartColors.length],
      opacity: selectedDisplayIndex.value < 0 || index === selectedDisplayIndex.value ? 1 : 0.32
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
        const model = displayModels.value[params.dataIndex]
        if (!model) return ''
        return `
          <div style="font-weight:600;margin-bottom:5px;">${model.modelName}</div>
          <div style="display:flex;justify-content:space-between;gap:16px;"><span>${t(props.locale, 'statistics.modelShare')}</span><b>${model.percent.toFixed(1)}%</b></div>
          <div style="display:flex;justify-content:space-between;gap:16px;"><span>${t(props.locale, 'statistics.requests')}</span><b>${formatRequestCount(model.requestCount)}</b></div>
          <div style="display:flex;justify-content:space-between;gap:16px;"><span>${t(props.locale, 'statistics.cost')}</span><b>${formatCost(model.cost, store.settings.currency)}</b></div>
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
      showSymbol: focusedTrendMetric.value === metric,
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
      }
    }))
  }
})

function selectModel(modelName: string) {
  selectedModelName.value = modelName
  menuOpen.value = false
}

function handleChartClick(params: any) {
  const model = typeof params?.dataIndex === 'number' ? displayModels.value[params.dataIndex] : null
  if (model) selectModel(model.modelName)
}

watch(
  rankedModels,
  models => {
    if (!models.length) {
      selectedModelName.value = null
      return
    }
    if (!models.some(model => model.modelName === selectedModelName.value)) {
      selectedModelName.value = models[0].modelName
    }
  },
  { immediate: true }
)

function handleOutsidePointer(event: MouseEvent) {
  if (!menuOpen.value) return
  if (selectRef.value && !selectRef.value.contains(event.target as Node)) {
    menuOpen.value = false
  }
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') menuOpen.value = false
}

onMounted(() => {
  document.addEventListener('mousedown', handleOutsidePointer)
  document.addEventListener('keydown', handleKeydown)
})

onBeforeUnmount(() => {
  document.removeEventListener('mousedown', handleOutsidePointer)
  document.removeEventListener('keydown', handleKeydown)
})
</script>

<template>
  <section class="rounded-xl bg-gray-50 p-3 dark:bg-neutral-800/70">
    <div class="mb-3 flex items-center justify-between">
      <p class="text-[11px] font-semibold text-gray-600 dark:text-gray-300">{{ t(locale, 'statistics.models') }}</p>
      <p class="text-[10px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.modelsUsed') }} {{ rankedModels.length }}</p>
    </div>

    <div v-if="displayModels.length && selectedModel" class="space-y-2">
      <div ref="selectRef" class="model-select">
        <button
          type="button"
          class="model-select__trigger"
          :aria-expanded="menuOpen"
          @click="menuOpen = !menuOpen"
        >
          <span class="model-select__dot" :style="{ backgroundColor: modelColor(selectedModel.modelName) }" />
          <span class="model-select__name">{{ selectedModel.modelName }}</span>
          <span class="model-select__meta">{{ formatRequestCount(selectedModel.requestCount) }} {{ t(locale, 'statistics.requests') }} · {{ selectedModel.percent.toFixed(1) }}%</span>
          <ChevronDown class="model-select__chevron" :class="{ 'model-select__chevron--open': menuOpen }" />
        </button>

        <transition name="model-menu">
          <div v-if="menuOpen" class="model-select__menu no-scrollbar">
            <button
              v-for="model in rankedModels"
              :key="model.modelName"
              type="button"
              class="model-select__option"
              :class="{ 'model-select__option--on': model.modelName === selectedModel.modelName }"
              @click="selectModel(model.modelName)"
            >
              <span class="model-select__dot" :style="{ backgroundColor: modelColor(model.modelName) }" />
              <span class="model-select__name">{{ model.modelName }}</span>
              <span class="model-select__meta">{{ formatRequestCount(model.requestCount) }} {{ t(locale, 'statistics.requests') }} · {{ model.percent.toFixed(1) }}%</span>
              <Check v-if="model.modelName === selectedModel.modelName" class="model-select__check" />
            </button>
          </div>
        </transition>
      </div>

      <div class="rounded-xl bg-white p-2.5 dark:bg-neutral-700/80">
        <div class="grid grid-cols-[minmax(0,1fr)_126px] gap-2.5">
          <div class="min-w-0">
            <div class="grid grid-cols-2 gap-1.5">
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-[9px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.cost') }}</p>
              <p class="font-mono text-[11px] font-bold text-gray-800 dark:text-gray-100">{{ formatCost(selectedModel.cost, store.settings.currency) }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="text-[9px] text-gray-400 dark:text-gray-500">{{ t(locale, 'statistics.metricTokens') }}</p>
              <p class="font-mono text-[11px] font-bold text-gray-800 dark:text-gray-100">{{ formatTokenValue(selectedModel.totalTokens) }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="flex items-center gap-1 text-[9px] text-gray-400 dark:text-gray-500">
                <span>{{ t(locale, 'statistics.avgTtft') }}</span>
                <span
                  v-if="hasPerformanceTag && hasPartialPerformanceCoverage"
                  class="inline-flex shrink-0 items-center rounded-full bg-indigo-50 px-1 py-px text-[7px] font-medium leading-none text-indigo-400 dark:bg-indigo-900/30 dark:text-indigo-400"
                  :title="performanceTagTitle"
                >{{ t(locale, 'overview.proxyPerformanceTag') }}</span>
              </p>
              <p class="font-mono text-[11px] font-bold text-gray-800 dark:text-gray-100">{{ avgTtft }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="flex items-center gap-1 text-[9px] text-gray-400 dark:text-gray-500">
                <span>{{ t(locale, 'statistics.avgSpeed') }}</span>
                <span
                  v-if="hasPerformanceTag && hasPartialPerformanceCoverage"
                  class="inline-flex shrink-0 items-center rounded-full bg-indigo-50 px-1 py-px text-[7px] font-medium leading-none text-indigo-400 dark:bg-indigo-900/30 dark:text-indigo-400"
                  :title="performanceTagTitle"
                >{{ t(locale, 'overview.proxyPerformanceTag') }}</span>
              </p>
              <p :class="['font-mono font-bold text-gray-800 dark:text-gray-100', metricValueSizeClass(avgSpeed)]">{{ avgSpeed }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="flex items-center gap-1 text-[9px] text-gray-400 dark:text-gray-500">
                <span>{{ t(locale, 'statistics.successRequests') }}</span>
                <span
                  v-if="hasPerformanceTag && hasPartialPerformanceCoverage"
                  class="inline-flex shrink-0 items-center rounded-full bg-indigo-50 px-1 py-px text-[7px] font-medium leading-none text-indigo-400 dark:bg-indigo-900/30 dark:text-indigo-400"
                  :title="performanceTagTitle"
                >{{ t(locale, 'overview.proxyPerformanceTag') }}</span>
              </p>
              <p class="truncate font-mono text-[11px] font-semibold text-emerald-600 dark:text-emerald-300">{{ formatRequestCount(successRequests) }}</p>
            </div>
            <div class="rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
              <p class="flex items-center gap-1 text-[9px] text-gray-400 dark:text-gray-500">
                <span>{{ t(locale, 'statistics.errorRequests') }}</span>
                <span
                  v-if="hasPerformanceTag && hasPartialPerformanceCoverage"
                  class="inline-flex shrink-0 items-center rounded-full bg-indigo-50 px-1 py-px text-[7px] font-medium leading-none text-indigo-400 dark:bg-indigo-900/30 dark:text-indigo-400"
                  :title="performanceTagTitle"
                >{{ t(locale, 'overview.proxyPerformanceTag') }}</span>
              </p>
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

        <div class="mt-1.5 rounded-lg bg-gray-50 px-2 py-1.5 dark:bg-neutral-800/80">
          <div class="mb-1 flex items-center justify-between">
            <p class="text-[9px] font-medium text-violet-500 dark:text-violet-400">{{ t(locale, 'statistics.cacheHitRate') }}</p>
            <p class="font-mono text-[11px] font-bold text-violet-600 dark:text-violet-300">{{ cacheHitRateDisplay }}</p>
          </div>
          <div v-if="cacheHitRate !== null" class="h-1 w-full overflow-hidden rounded-full bg-violet-100 dark:bg-violet-900/40">
            <div
              class="h-full rounded-full bg-violet-400 transition-all duration-500 dark:bg-violet-500"
              :style="{ width: `${Math.min(cacheHitRate, 100).toFixed(1)}%` }"
            />
          </div>
        </div>

        <div class="mt-2 rounded-lg bg-gray-50 p-1.5 dark:bg-neutral-800/80">
            <div class="mb-1 flex items-center justify-between">
              <p class="text-[10px] font-semibold text-gray-500 dark:text-gray-300">{{ t(locale, 'statistics.statusCodeDetail') }}</p>
              <p class="font-mono text-[10px] text-gray-400 dark:text-gray-500">{{ formatRequestCount(selectedModel.requestCount) }}</p>
            </div>
            <div v-if="selectedStatusCodes.length || localRequestCount > 0" class="flex flex-wrap gap-1">
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
              <span
                v-if="localRequestCount > 0"
                class="rounded-full bg-gray-200 px-1.5 py-0.5 font-mono text-[10px] font-semibold text-gray-600 dark:bg-neutral-600 dark:text-gray-300"
                :title="t(locale, 'overview.localRequestTagTitle')"
              >{{ t(locale, 'overview.localRequestTag') }} · {{ formatRequestCount(localRequestCount) }}</span>
            </div>
            <div v-else class="text-[10px] text-gray-400 dark:text-gray-500">
              {{ t(locale, 'statistics.noStatusCodeData') }}
            </div>
          </div>
      </div>

      <div class="rounded-xl bg-white p-2 dark:bg-neutral-700/80">
        <div class="mb-1.5 flex items-center justify-between">
          <p class="text-[10px] font-semibold text-gray-500 dark:text-gray-300">{{ t(locale, 'statistics.modelTrend') }}</p>
          <div class="trend-seg">
            <button
              v-for="metric in (['requests', 'tokens', 'cost', 'rate'] as const)"
              :key="metric"
              type="button"
              class="trend-seg__item"
              :class="{ 'trend-seg__item--on': focusedTrendMetric === metric }"
              @click="focusedTrendMetric = focusedTrendMetric === metric ? null : metric"
            >
              <span class="trend-seg__dot" :style="{ backgroundColor: trendColors[metric] }" />
              <span class="trend-seg__label">{{ trendMetricLabel(metric) }}</span>
            </button>
          </div>
        </div>
        <div v-if="selectedModel.trend.length" class="h-[116px]">
          <v-chart class="h-full w-full" :option="trendOptions" autoresize />
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

<style scoped>
/* Model picker — theme-adaptive dropdown (replaces horizontal-scroll chips).
   Scales to any number of models; the menu scrolls vertically. */
.model-select {
  position: relative;
}

.model-select__trigger {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  border-radius: 10px;
  border: 1px solid var(--theme-border-default);
  background: var(--theme-bg-elevated);
  text-align: left;
  cursor: pointer;
  outline: none;
  transition: border-color 0.18s ease, background 0.18s ease, box-shadow 0.18s ease;
}

.model-select__trigger:hover {
  border-color: var(--theme-border-strong);
}

.model-select__trigger:focus-visible {
  border-color: var(--theme-accent-primary);
  box-shadow: 0 0 0 3px var(--theme-ring-focus);
}

.model-select__dot {
  width: 8px;
  height: 8px;
  flex-shrink: 0;
  border-radius: 9999px;
}

.model-select__name {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.3;
  color: var(--theme-text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.model-select__meta {
  flex-shrink: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 9px;
  line-height: 1.3;
  color: var(--theme-text-tertiary);
  white-space: nowrap;
}

.model-select__chevron {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  color: var(--theme-text-tertiary);
  transition: transform 0.2s ease;
}

.model-select__chevron--open {
  transform: rotate(180deg);
}

.model-select__menu {
  position: absolute;
  top: calc(100% + 5px);
  left: 0;
  right: 0;
  z-index: 40;
  max-height: 216px;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 4px;
  border-radius: 12px;
  border: 1px solid var(--theme-border-default);
  background: var(--theme-bg-overlay);
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.16);
  backdrop-filter: blur(14px);
}

.model-select__option {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 5px 8px;
  border-radius: 8px;
  border: none;
  background: transparent;
  text-align: left;
  cursor: pointer;
  transition: background 0.14s ease;
}

.model-select__option:hover {
  background: var(--theme-bg-hover);
}

.model-select__option--on {
  background: var(--theme-accent-soft);
}

.model-select__option--on .model-select__name {
  color: var(--theme-accent-primary);
}

.model-select__check {
  width: 13px;
  height: 13px;
  flex-shrink: 0;
  color: var(--theme-accent-primary);
}

.model-menu-enter-active,
.model-menu-leave-active {
  transition: opacity 0.16s ease, transform 0.16s ease;
}

.model-menu-enter-from,
.model-menu-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

</style>
