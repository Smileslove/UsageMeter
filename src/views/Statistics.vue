<script setup lang="ts">
import { computed, onMounted, watch } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t, windowNameLabel } from '../i18n'
import type { ModelRateStats, ModelTtftStats } from '../types'
import ModelDistribution from '../components/ModelDistribution.vue'

const store = useMonitorStore()

// 获取 30d 窗口数据用于统计
const window30d = computed(() => store.windows.find(w => w.window === '30d'))

// 格式化数字显示（请求数：小于 1000 显示整数，大于等于 1000 用 K/M 单位保留两位小数）
const formatNumber = (num: number): string => {
  if (num >= 1_000_000) return `${(num / 1_000_000).toFixed(2)}`
  if (num >= 1_000) return `${(num / 1_000).toFixed(2)}`
  return String(Math.round(num))
}

// 获取数字单位
const getNumberUnit = (num: number): string => {
  if (num >= 1_000_000) return 'M'
  if (num >= 1_000) return 'K'
  return ''
}

// 趋势数据
const trendData = computed(() => store.trendHistory['30d'] ?? [])

// 计算趋势图路径
const trendPath = computed(() => {
  if (trendData.value.length < 2) {
    // 默认占位路径
    return 'M0 40 L0 38 Q 10 20, 20 30 T 40 25 T 60 38 T 80 15 T 100 35 L100 40 Z'
  }

  const data = trendData.value.slice(-10)
  const max = Math.max(...data, 100)
  const points = data.map((v, i) => {
    const x = (i / (data.length - 1)) * 100
    const y = 40 - (v / max) * 35
    return `${x} ${y}`
  })

  // 创建平滑曲线路径
  const pathPoints = points.map((p, i) => {
    if (i === 0) return `M${p}`
    return `L${p}`
  }).join(' ')

  return `${pathPoints} L100 40 L0 40 Z`
})

const strokePath = computed(() => {
  if (trendData.value.length < 2) {
    return 'M0 38 Q 10 20, 20 30 T 40 25 T 60 38 T 80 15 T 100 35'
  }

  const data = trendData.value.slice(-10)
  const max = Math.max(...data, 100)
  const points = data.map((v, i) => {
    const x = (i / (data.length - 1)) * 100
    const y = 40 - (v / max) * 35
    return `${x} ${y}`
  })

  return points.map((p, i) => {
    if (i === 0) return `M${p}`
    return `L${p}`
  }).join(' ')
})

// 状态码分布数据
const statusCodeData = computed(() => {
  const summary = store.snapshot?.summary
  if (!summary) return null

  const total = summary.totalRequests || 0
  const success = summary.totalSuccessRequests || 0
  const clientError = summary.totalClientErrorRequests || 0
  const serverError = summary.totalServerErrorRequests || 0

  return {
    total,
    success,
    clientError,
    serverError,
    successRate: total > 0 ? ((success / total) * 100).toFixed(1) : '0'
  }
})

// 状态码饼图路径
const statusPiePath = computed(() => {
  if (!statusCodeData.value || statusCodeData.value.total === 0) {
    return { success: '', clientError: '', serverError: '' }
  }

  const { success, clientError, serverError, total } = statusCodeData.value

  // 计算 SVG 饼图路径（圆心 50,50 半径 40）
  const polarToCartesian = (centerX: number, centerY: number, radius: number, angleInDegrees: number) => {
    const angleInRadians = ((angleInDegrees - 90) * Math.PI) / 180.0
    return {
      x: centerX + radius * Math.cos(angleInRadians),
      y: centerY + radius * Math.sin(angleInRadians)
    }
  }

  const describeArc = (startAngle: number, endAngle: number, radius: number = 40) => {
    const start = polarToCartesian(50, 50, radius, endAngle)
    const end = polarToCartesian(50, 50, radius, startAngle)
    const largeArcFlag = endAngle - startAngle <= 180 ? '0' : '1'

    return [
      'M', 50, 50,
      'L', start.x, start.y,
      'A', radius, radius, 0, largeArcFlag, 0, end.x, end.y,
      'Z'
    ].join(' ')
  }

  const successAngle = (success / total) * 360
  const clientErrorAngle = (clientError / total) * 360
  const serverErrorAngle = (serverError / total) * 360

  let currentAngle = 0

  return {
    success: describeArc(currentAngle, currentAngle + successAngle),
    clientError: describeArc(currentAngle + successAngle, currentAngle + successAngle + clientErrorAngle),
    serverError: describeArc(currentAngle + successAngle + clientErrorAngle, currentAngle + successAngle + clientErrorAngle + serverErrorAngle)
  }
})

// ===== Token 生成速率统计 =====

// 格式化速率显示
const formatRate = (rate: number): string => {
  if (rate === 0) return '0'
  if (rate >= 100) return rate.toFixed(0)
  return rate.toFixed(1)
}

// 格式化时间显示（毫秒转为可读格式）
const formatDuration = (ms: number): string => {
  if (ms < 1000) return `${ms}ms`
  const seconds = Math.floor(ms / 1000)
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  const remainingSeconds = seconds % 60
  if (minutes < 60) return `${minutes}m ${remainingSeconds}s`
  const hours = Math.floor(minutes / 60)
  const remainingMinutes = minutes % 60
  return `${hours}h ${remainingMinutes}m`
}

// 获取前 5 个模型
const topModels = computed(() => {
  if (!store.rateSummary?.byModel) return []
  return store.rateSummary.byModel.slice(0, 5)
})

// 计算速率条宽度百分比
const getRateBarWidth = (model: ModelRateStats): string => {
  if (!store.rateSummary?.byModel?.length) return '0%'
  const maxRate = Math.max(...store.rateSummary.byModel.map(m => m.avgTokensPerSecond))
  if (maxRate === 0) return '0%'
  return `${(model.avgTokensPerSecond / maxRate) * 100}%`
}

// ===== TTFT 统计（首 Token 生成时间）=====

// 格式化 TTFT 显示（毫秒转为可读格式）
const formatTtft = (ms: number): string => {
  if (!ms || ms === 0) return '-'
  if (ms < 1000) return `${ms.toFixed(0)}ms`
  return `${(ms / 1000).toFixed(2)}s`
}

// 获取前 5 个模型（按 TTFT 升序，响应最快的在前）
const topTtftModels = computed(() => {
  if (!rateSummary.value?.ttftByModel) return []
  return [...rateSummary.value.ttftByModel]
    .sort((a, b) => a.avgTtftMs - b.avgTtftMs)
    .slice(0, 5)
})

// 计算 TTFT 条宽度（反向：TTFT 越小，条越长）
const getTtftBarWidth = (model: ModelTtftStats): string => {
  if (!rateSummary.value?.ttftByModel?.length) return '0%'
  const maxTtft = Math.max(...rateSummary.value.ttftByModel.map(m => m.avgTtftMs))
  if (maxTtft === 0) return '100%'
  // 反向：TTFT 越小，条越长（表示响应越快）
  return `${((maxTtft - model.avgTtftMs) / maxTtft) * 100}%`
}

// 速率摘要数据
const rateSummary = computed(() => store.rateSummary)

// 窗口名称国际化
const windowDisplayName = computed(() => {
  if (!rateSummary.value?.window) return ''
  return windowNameLabel(store.settings.locale, rateSummary.value.window)
})

// 监听代理模式变化，自动加载速率数据
watch(
  () => ({ isProxy: store.isProxyMode, window: store.settings.summaryWindow }),
  ({ isProxy, window }) => {
    if (isProxy && window) {
      store.fetchRateSummary(window)
    }
  },
  { immediate: true }
)

// 组件挂载时，如果是代理模式，加载速率数据
onMounted(() => {
  if (store.isProxyMode && store.settings.summaryWindow) {
    store.fetchRateSummary(store.settings.summaryWindow)
  }
})
</script>

<template>
  <div class="space-y-4 animate-in fade-in zoom-in-95 duration-300 pb-2">
    <!-- Development Notice -->
    <div class="bg-amber-50 dark:bg-amber-900/20 border border-amber-200 dark:border-amber-800/30 rounded-xl p-3 flex items-center gap-2">
      <svg class="w-4 h-4 text-amber-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
      </svg>
      <span class="text-[11px] font-medium text-amber-700 dark:text-amber-300">{{ t(store.settings.locale, 'common.underDevelopment') }}</span>
    </div>
    <!-- 2x2 Grid -->
    <div class="grid grid-cols-2 gap-3">
      <!-- Requests -->
      <div class="bg-[#F0FDF4] dark:bg-green-900/10 p-3.5 rounded-xl border border-green-100 dark:border-green-900/30 flex justify-between items-center h-16">
        <div class="flex flex-col">
          <p class="text-[11px] font-semibold text-green-600/80 mb-0.5">{{ t(store.settings.locale, 'common.requests') }}</p>
          <p class="text-xl font-bold font-mono text-gray-800 dark:text-gray-100">
            {{ window30d?.requestUsed ? formatNumber(window30d.requestUsed) : '0' }}
            <span class="text-xs text-gray-500 font-sans">{{ window30d?.requestUsed ? getNumberUnit(window30d.requestUsed) : '' }}</span>
          </p>
        </div>
      </div>
      <!-- Duration - 占位，后端暂不提供 -->
      <div class="bg-white dark:bg-[#1C1C1E] p-3.5 rounded-xl border border-gray-100 dark:border-neutral-800 flex justify-between items-center h-16">
        <div class="flex flex-col">
          <p class="text-[11px] font-semibold text-gray-500 mb-0.5">{{ t(store.settings.locale, 'metrics.tokenUsageRate') }}</p>
          <p class="text-xl font-bold font-mono text-gray-800 dark:text-gray-100">
            {{ window30d?.tokenUsed ? formatNumber(window30d.tokenUsed) : '0' }}
            <span class="text-xs text-gray-500 font-sans">{{ window30d?.tokenUsed ? getNumberUnit(window30d.tokenUsed) : '' }}</span>
          </p>
        </div>
      </div>
      <!-- Token -->
      <div class="bg-white dark:bg-[#1C1C1E] p-3.5 rounded-xl border border-gray-100 dark:border-neutral-800 flex justify-between items-center h-16">
        <div class="flex flex-col">
          <p class="text-[11px] font-semibold text-gray-500 mb-0.5">{{ t(store.settings.locale, 'common.token') }}</p>
          <p class="text-xl font-bold font-mono text-gray-800 dark:text-gray-100">
            {{ window30d?.tokenUsed ? formatNumber(window30d.tokenUsed) : '0' }}
            <span class="text-xs text-gray-500 font-sans">{{ window30d?.tokenUsed ? getNumberUnit(window30d.tokenUsed) : '' }}</span>
          </p>
        </div>
      </div>
      <!-- Price - 占位，后端暂不提供 -->
      <div class="bg-white dark:bg-[#1C1C1E] p-3.5 rounded-xl border border-gray-100 dark:border-neutral-800 flex justify-between items-center h-16">
        <div class="flex flex-col">
          <p class="text-[11px] font-semibold text-gray-500 mb-0.5">Source</p>
          <p class="text-sm font-medium text-gray-600 dark:text-gray-300">
            {{ store.snapshot?.source ?? '-' }}
          </p>
        </div>
      </div>
    </div>

    <!-- 模型分布统计 -->
    <ModelDistribution />

    <!-- Status Code Distribution (Proxy mode only) -->
    <div v-if="store.isProxyMode && statusCodeData && statusCodeData.total > 0" class="bg-white dark:bg-[#1C1C1E] p-4 rounded-xl border border-gray-100 dark:border-neutral-800">
      <div class="flex justify-between items-center mb-3">
        <h3 class="text-[12px] font-bold text-gray-600 dark:text-gray-300">{{ t(store.settings.locale, 'metrics.statusCodeDistribution') }}</h3>
        <span class="text-[10px] text-gray-500">{{ t(store.settings.locale, 'metrics.successRate') }}: {{ statusCodeData.successRate }}%</span>
      </div>
      <div class="flex items-center gap-4">
        <!-- Pie Chart -->
        <div class="w-20 h-20 shrink-0">
          <svg viewBox="0 0 100 100" class="w-full h-full">
            <!-- Success (2xx) - Green -->
            <path v-if="statusCodeData.success > 0" :d="statusPiePath.success" fill="#22C55E" class="transition-all duration-300" />
            <!-- Client Error (4xx) - Orange -->
            <path v-if="statusCodeData.clientError > 0" :d="statusPiePath.clientError" fill="#FB923C" class="transition-all duration-300" />
            <!-- Server Error (5xx) - Red -->
            <path v-if="statusCodeData.serverError > 0" :d="statusPiePath.serverError" fill="#EF4444" class="transition-all duration-300" />
            <!-- Empty state -->
            <circle v-if="statusCodeData.total === 0" cx="50" cy="50" r="40" fill="#E5E7EB" class="dark:fill-neutral-700" />
          </svg>
        </div>
        <!-- Legend -->
        <div class="flex-1 space-y-1.5">
          <div class="flex items-center justify-between text-[11px]">
            <div class="flex items-center gap-1.5">
              <div class="w-2 h-2 rounded-full bg-green-500"></div>
              <span class="text-gray-600 dark:text-gray-400">{{ t(store.settings.locale, 'metrics.success') }} (2xx)</span>
            </div>
            <span class="font-mono text-gray-800 dark:text-gray-200">{{ statusCodeData.success }}</span>
          </div>
          <div class="flex items-center justify-between text-[11px]">
            <div class="flex items-center gap-1.5">
              <div class="w-2 h-2 rounded-full bg-orange-400"></div>
              <span class="text-gray-600 dark:text-gray-400">{{ t(store.settings.locale, 'metrics.clientError') }} (4xx)</span>
            </div>
            <span class="font-mono text-gray-800 dark:text-gray-200">{{ statusCodeData.clientError }}</span>
          </div>
          <div class="flex items-center justify-between text-[11px]">
            <div class="flex items-center gap-1.5">
              <div class="w-2 h-2 rounded-full bg-red-500"></div>
              <span class="text-gray-600 dark:text-gray-400">{{ t(store.settings.locale, 'metrics.serverError') }} (5xx)</span>
            </div>
            <span class="font-mono text-gray-800 dark:text-gray-200">{{ statusCodeData.serverError }}</span>
          </div>
        </div>
      </div>
    </div>

    <!-- Token Generation Rate (Proxy mode only) -->
    <div v-if="store.isProxyMode && rateSummary && rateSummary.overall.requestCount > 0" class="bg-white dark:bg-[#1C1C1E] p-4 rounded-xl border border-gray-100 dark:border-neutral-800">
      <div class="flex justify-between items-center mb-3">
        <h3 class="text-[12px] font-bold text-gray-600 dark:text-gray-300">{{ t(store.settings.locale, 'metrics.tokenGenerationRate') }}</h3>
        <span class="text-[10px] text-gray-500">
          {{ windowDisplayName }} · {{ rateSummary.overall.requestCount }} {{ t(store.settings.locale, 'common.requests') }}
        </span>
      </div>

      <!-- Overall Average Rate -->
      <div class="flex items-baseline gap-1 mb-3">
        <span class="text-2xl font-bold font-mono text-gray-800 dark:text-gray-100">
          {{ formatRate(rateSummary.overall.avgTokensPerSecond) }}
        </span>
        <span class="text-xs text-gray-500">t/s</span>
        <span class="text-[10px] text-gray-400 ml-2">{{ t(store.settings.locale, 'metrics.avgSpeed') }}</span>
      </div>

      <!-- Additional Stats Row -->
      <div class="flex gap-4 mb-4 text-[10px] text-gray-500">
        <div class="flex items-center gap-1">
          <span>{{ t(store.settings.locale, 'metrics.totalOutput') }}:</span>
          <span class="font-mono text-gray-700 dark:text-gray-300">{{ formatNumber(rateSummary.overall.totalOutputTokens) }}{{ getNumberUnit(rateSummary.overall.totalOutputTokens) }}</span>
        </div>
        <div class="flex items-center gap-1">
          <span>{{ t(store.settings.locale, 'metrics.totalDuration') }}:</span>
          <span class="font-mono text-gray-700 dark:text-gray-300">{{ formatDuration(rateSummary.overall.totalDurationMs) }}</span>
        </div>
      </div>

      <!-- Model Rate Ranking (Top 5) -->
      <div v-if="topModels.length > 0" class="space-y-2">
        <div class="flex justify-between items-center">
          <p class="text-[11px] text-gray-500">{{ t(store.settings.locale, 'metrics.modelRateRanking') }}</p>
          <p class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'metrics.rateRange') }}</p>
        </div>
        <div v-for="model in topModels" :key="model.modelName" class="flex items-center gap-2">
          <span class="text-[11px] text-gray-600 dark:text-gray-400 w-24 truncate" :title="model.modelName">{{ model.modelName }}</span>
          <div class="flex-1 h-2 bg-gray-100 dark:bg-neutral-700 rounded-full overflow-hidden">
            <div class="h-full bg-green-500 rounded-full transition-all duration-300" :style="{ width: getRateBarWidth(model) }"></div>
          </div>
          <span class="text-[11px] font-mono text-gray-800 dark:text-gray-200 w-20 text-right">
            {{ formatRate(model.avgTokensPerSecond) }} t/s
            <span v-if="model.minTokensPerSecond > 0 || model.maxTokensPerSecond > 0" class="text-gray-400">
              ({{ formatRate(model.minTokensPerSecond) }}-{{ formatRate(model.maxTokensPerSecond) }})
            </span>
          </span>
        </div>
      </div>
    </div>

    <!-- TTFT 统计 (Proxy mode only) -->
    <div v-if="store.isProxyMode && rateSummary?.ttft && rateSummary.ttft.requestCount > 0" class="bg-white dark:bg-[#1C1C1E] p-4 rounded-xl border border-gray-100 dark:border-neutral-800">
      <div class="flex justify-between items-center mb-3">
        <h3 class="text-[12px] font-bold text-gray-600 dark:text-gray-300">{{ t(store.settings.locale, 'metrics.ttft') }}</h3>
        <span class="text-[10px] text-gray-500">
          {{ rateSummary.ttft.requestCount }} {{ t(store.settings.locale, 'common.requests') }}
        </span>
      </div>

      <!-- 平均 TTFT -->
      <div class="flex items-baseline gap-1 mb-3">
        <span class="text-2xl font-bold font-mono text-gray-800 dark:text-gray-100">
          {{ formatTtft(rateSummary.ttft.avgTtftMs) }}
        </span>
        <span class="text-[10px] text-gray-400 ml-2">{{ t(store.settings.locale, 'metrics.avgTtft') }}</span>
      </div>

      <!-- 范围 -->
      <div class="flex gap-4 mb-4 text-[10px] text-gray-500">
        <div class="flex items-center gap-1">
          <span>{{ t(store.settings.locale, 'metrics.minTtft') }}:</span>
          <span class="font-mono text-gray-700 dark:text-gray-300">{{ formatTtft(rateSummary.ttft.minTtftMs) }}</span>
        </div>
        <div class="flex items-center gap-1">
          <span>{{ t(store.settings.locale, 'metrics.maxTtft') }}:</span>
          <span class="font-mono text-gray-700 dark:text-gray-300">{{ formatTtft(rateSummary.ttft.maxTtftMs) }}</span>
        </div>
      </div>

      <!-- 模型 TTFT 排行 -->
      <div v-if="topTtftModels.length > 0" class="space-y-2">
        <p class="text-[11px] text-gray-500">{{ t(store.settings.locale, 'metrics.modelTtftRanking') }}</p>
        <div v-for="model in topTtftModels" :key="model.modelName" class="flex items-center gap-2">
          <span class="text-[11px] text-gray-600 dark:text-gray-400 w-24 truncate" :title="model.modelName">{{ model.modelName }}</span>
          <div class="flex-1 h-2 bg-gray-100 dark:bg-neutral-700 rounded-full overflow-hidden">
            <div class="h-full bg-cyan-500 rounded-full transition-all duration-300" :style="{ width: getTtftBarWidth(model) }"></div>
          </div>
          <span class="text-[11px] font-mono text-gray-800 dark:text-gray-200 w-20 text-right">
            {{ formatTtft(model.avgTtftMs) }}
          </span>
        </div>
      </div>
    </div>

    <!-- Chart component placeholder -->
    <div class="bg-white dark:bg-[#1C1C1E] p-4 rounded-xl border border-gray-100 dark:border-neutral-800 flex flex-col mt-4">
      <div class="flex justify-between items-center mb-4">
        <h3 class="text-[12px] font-bold text-gray-600 dark:text-gray-300">{{ t(store.settings.locale, 'metrics.shortTrend') }}</h3>
        <div class="bg-gray-100 dark:bg-[#2C2C2E] text-[10px] flex p-0.5 rounded text-gray-500 font-medium">
          <button class="px-2 py-0.5 rounded-sm hover:text-gray-700">{{ t(store.settings.locale, 'common.token') }}</button>
          <button class="px-2 py-0.5 rounded-sm bg-white dark:bg-[#3A3A3C] shadow-sm text-gray-800 dark:text-gray-100">{{ t(store.settings.locale, 'common.requests') }}</button>
        </div>
      </div>
      <div class="h-32 w-full bg-gradient-to-t from-green-50 to-transparent dark:from-green-900/10 border-b border-green-200 dark:border-green-800 flex items-end">
        <svg viewBox="0 0 100 40" class="w-full h-full preserve-aspect-ratio-none">
          <path :d="trendPath" fill="rgba(34, 197, 94, 0.15)"/>
          <path :d="strokePath" stroke="#22C55E" stroke-width="1.5" fill="none"/>
        </svg>
      </div>
    </div>
  </div>
</template>
