<script setup lang="ts">
import { computed, watch, onMounted } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t, windowNameLabel } from '../i18n'
import { Zap, Activity, Gauge, Coins } from 'lucide-vue-next'
import { getCurrencySymbol, convertCost } from '../utils/format'

const store = useMonitorStore()

// 获取当前选择的汇总窗口数据
const summaryWindowData = computed(() => {
  const windowName = store.settings.summaryWindow
  return store.windows.find(w => w.window === windowName)
})

// 计算总输入 Token（包含缓存读取）
const totalInputTokens = computed(() => {
  const data = summaryWindowData.value
  if (!data) return 0
  return (data.inputTokens ?? 0) + (data.cacheReadTokens ?? 0)
})

// 格式化请求数（小于1000显示整数，大于等于1000用K单位保留2位小数）
const formatRequestNumber = (num: number): string => {
  if (num >= 1_000_000) return `${(num / 1_000_000).toFixed(2)}M`
  if (num >= 1_000) return `${(num / 1_000).toFixed(2)}K`
  return Math.round(num).toString()
}

// 格式化 Token 数值（统一单位，保留两位小数）
const formatTokenValue = (num: number, unit: 'K' | 'M' | 'none'): string => {
  if (unit === 'M') return `${(num / 1_000_000).toFixed(2)}M`
  if (unit === 'K') return `${(num / 1_000).toFixed(2)}K`
  return num.toFixed(2)
}

// 确定输入输出的统一单位（使用总输入）
const getTokenUnit = (totalInput: number, output: number): 'K' | 'M' | 'none' => {
  const maxVal = Math.max(totalInput, output)
  if (maxVal >= 1_000_000) return 'M'
  if (maxVal >= 1_000) return 'K'
  return 'none'
}

// 格式化速率显示（保留两位小数）
const formatRate = (rate: number): string => {
  if (rate === 0) return '0.00'
  return rate.toFixed(2)
}

// 智能格式化费用（根据金额大小选择精度，支持多货币）
const formatCost = (cost: number | undefined): string => {
  if (cost === undefined || cost === null) return `${getCurrencySymbol(store.settings.currency.displayCurrency)}0.00`
  const converted = convertCost(cost, store.settings.currency)
  const sym = getCurrencySymbol(store.settings.currency.displayCurrency)
  if (converted >= 1) return `${sym}${converted.toFixed(2)}`
  if (converted >= 0.01) return `${sym}${converted.toFixed(3)}`
  if (converted >= 0.001) return `${sym}${converted.toFixed(4)}`
  if (converted > 0) return `${sym}${converted.toFixed(6)}`
  return `${sym}0.00`
}

// 获取窗口标签
const getWindowLabel = (window: string): string => {
  return windowNameLabel(store.settings.locale, window)
}

// 智能分割标签文本为多行（竖排显示）
const labelLines = computed(() => {
  const label = getWindowLabel(summaryWindowData.value?.window || '24h')
  const locale = store.settings.locale

  // 中文模式：数字单独一行，其他字符每个一行
  if (locale === 'zh-CN' || locale === 'zh-TW') {
    // 检查是否包含数字（如"5小时"、"30天"）
    const match = label.match(/^(\d+)(.+)$/)
    if (match) {
      // 数字一行，后面每个字符一行
      const lines: string[] = [match[1]]
      for (const char of match[2]) {
        lines.push(char)
      }
      return lines
    }
    // 否则按字符分割
    return label.split('')
  }

  // 英文模式：按单词分割
  return label.split(' ')
})

// 速率摘要数据
const rateSummary = computed(() => store.rateSummary)

// 是否有速率数据（代理模式且有请求）
const hasRateData = computed(() => {
  return store.isProxyMode && rateSummary.value && rateSummary.value.overall.requestCount > 0
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
  <div v-if="summaryWindowData" class="bg-white dark:bg-[#1C1C1E] rounded-xl p-2 shadow-sm border border-gray-100 dark:border-neutral-800">
    <div class="flex items-stretch gap-1.5">
      <!-- 左侧窗口竖排文字 -->
      <div class="flex-shrink-0 flex items-center justify-center px-1.5 py-0.5">
        <div class="flex flex-col items-center gap-0">
          <span
            v-for="(line, idx) in labelLines"
            :key="idx"
            class="text-sm font-bold text-gray-800 dark:text-gray-100 tracking-wide leading-tight select-none"
          >{{ line }}</span>
        </div>
      </div>

      <!-- 右侧核心数据区 -->
      <!-- 代理模式：4列（请求、Token、速率、费用） -->
      <div v-if="hasRateData" class="flex-1 grid grid-cols-4 gap-1.5">
        <!-- 总请求数 -->
        <div class="flex items-center justify-center bg-purple-50 dark:bg-purple-500/10 border border-purple-100 dark:border-purple-900/40 rounded-lg py-1.5 px-2">
          <div class="flex flex-col items-center">
            <Zap class="w-3.5 h-3.5 text-purple-500 mb-0.5" />
            <span class="text-[9px] font-semibold text-purple-600 dark:text-purple-400 leading-tight">{{ t(store.settings.locale, 'common.requests') }}</span>
            <span class="text-sm font-bold text-gray-800 dark:text-gray-100 font-mono leading-tight">{{ formatRequestNumber(summaryWindowData.requestUsed) }}</span>
          </div>
        </div>

        <!-- 总 Token（输入/输出） -->
        <div class="flex items-center justify-center bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-100 dark:border-emerald-900/40 rounded-lg py-1.5 px-2">
          <div class="flex flex-col items-center">
            <Activity class="w-3.5 h-3.5 text-emerald-500 mb-0.5" />
            <span class="text-[9px] font-semibold text-emerald-600 dark:text-emerald-400 leading-tight">{{ t(store.settings.locale, 'common.token') }}</span>
            <span class="text-sm font-bold text-gray-800 dark:text-gray-100 font-mono leading-tight">{{ formatTokenValue(summaryWindowData.tokenUsed, getTokenUnit(totalInputTokens, summaryWindowData.outputTokens)) }}</span>
            <div class="flex items-center gap-0.5 text-[8px] leading-tight">
              <span class="text-blue-500 font-medium">{{ formatTokenValue(totalInputTokens, getTokenUnit(totalInputTokens, summaryWindowData.outputTokens)) }}</span>
              <span class="text-gray-400">/</span>
              <span class="text-teal-500 font-medium">{{ formatTokenValue(summaryWindowData.outputTokens, getTokenUnit(totalInputTokens, summaryWindowData.outputTokens)) }}</span>
            </div>
          </div>
        </div>

        <!-- 平均生成速率 -->
        <div class="flex items-center justify-center bg-cyan-50 dark:bg-cyan-500/10 border border-cyan-100 dark:border-cyan-900/40 rounded-lg py-1.5 px-2">
          <div class="flex flex-col items-center">
            <Gauge class="w-3.5 h-3.5 text-cyan-500 mb-0.5" />
            <span class="text-[9px] font-semibold text-cyan-600 dark:text-cyan-400 leading-tight">{{ t(store.settings.locale, 'common.avgRate') }}</span>
            <div class="flex items-baseline gap-0.5 leading-tight">
              <span class="text-sm font-bold text-gray-800 dark:text-gray-100 font-mono">{{ formatRate(rateSummary!.overall.avgTokensPerSecond) }}</span>
              <span class="text-[8px] text-gray-500">t/s</span>
            </div>
          </div>
        </div>

        <!-- 总费用 -->
        <div class="flex items-center justify-center bg-amber-50 dark:bg-amber-500/10 border border-amber-100 dark:border-amber-900/40 rounded-lg py-1.5 px-2">
          <div class="flex flex-col items-center">
            <Coins class="w-3.5 h-3.5 text-amber-500 mb-0.5" />
            <span class="text-[9px] font-semibold text-amber-600 dark:text-amber-400 leading-tight">{{ t(store.settings.locale, 'common.cost') }}</span>
            <span class="text-sm font-bold text-gray-800 dark:text-gray-100 font-mono leading-tight">{{ formatCost(summaryWindowData?.cost) }}</span>
          </div>
        </div>
      </div>

      <!-- 非代理模式：3列（请求、Token详情、费用） -->
      <div v-else class="flex-1 grid grid-cols-3 gap-1.5">
        <!-- 总请求数 -->
        <div class="flex items-center justify-center bg-purple-50 dark:bg-purple-500/10 border border-purple-100 dark:border-purple-900/40 rounded-lg py-1.5 px-2">
          <div class="flex flex-col items-center">
            <Zap class="w-3.5 h-3.5 text-purple-500 mb-0.5" />
            <span class="text-[9px] font-semibold text-purple-600 dark:text-purple-400 leading-tight">{{ t(store.settings.locale, 'common.requests') }}</span>
            <span class="text-sm font-bold text-gray-800 dark:text-gray-100 font-mono leading-tight">{{ formatRequestNumber(summaryWindowData.requestUsed) }}</span>
          </div>
        </div>

        <!-- Token 详情（输入/输出） -->
        <div class="flex items-center justify-center bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-100 dark:border-emerald-900/40 rounded-lg py-1.5 px-2">
          <div class="flex flex-col items-center">
            <Activity class="w-3.5 h-3.5 text-emerald-500 mb-0.5" />
            <span class="text-[9px] font-semibold text-emerald-600 dark:text-emerald-400 leading-tight">{{ t(store.settings.locale, 'common.token') }}</span>
            <span class="text-sm font-bold text-gray-800 dark:text-gray-100 font-mono leading-tight">{{ formatTokenValue(summaryWindowData.tokenUsed, getTokenUnit(totalInputTokens, summaryWindowData.outputTokens)) }}</span>
            <div class="flex items-center gap-0.5 text-[8px] leading-tight">
              <span class="text-blue-500 font-medium">{{ formatTokenValue(totalInputTokens, getTokenUnit(totalInputTokens, summaryWindowData.outputTokens)) }}</span>
              <span class="text-gray-400">/</span>
              <span class="text-teal-500 font-medium">{{ formatTokenValue(summaryWindowData.outputTokens, getTokenUnit(totalInputTokens, summaryWindowData.outputTokens)) }}</span>
            </div>
          </div>
        </div>

        <!-- 总费用 -->
        <div class="flex items-center justify-center bg-amber-50 dark:bg-amber-500/10 border border-amber-100 dark:border-amber-900/40 rounded-lg py-1.5 px-2">
          <div class="flex flex-col items-center">
            <Coins class="w-3.5 h-3.5 text-amber-500 mb-0.5" />
            <span class="text-[9px] font-semibold text-amber-600 dark:text-amber-400 leading-tight">{{ t(store.settings.locale, 'common.cost') }}</span>
            <span class="text-sm font-bold text-gray-800 dark:text-gray-100 font-mono leading-tight">{{ formatCost(summaryWindowData?.cost) }}</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
