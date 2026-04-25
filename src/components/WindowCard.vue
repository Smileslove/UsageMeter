<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import DonutChart from './DonutChart.vue'
import NestedDonutChart from './NestedDonutChart.vue'
import type { WindowUsage } from '../types'
import { AlertTriangle, AlertCircle, ShieldCheck } from 'lucide-vue-next'

interface Props {
  window: WindowUsage
  displayMode: 'auto' | 'exact'
}

defineProps<Props>()
const store = useMonitorStore()

// 判断是否为单环模式
const isSingleRing = computed(() => store.settings.billingType === 'token' || store.settings.billingType === 'request')

// 环尺寸：单环 80px，双环 64px
// const ringSize = computed(() => (isSingleRing.value ? 80 : 64))

// 窗口标签
const getWindowLabel = (name: string): string => {
  const map: Record<string, string> = {
    '5h': 'settings.window5h',
    '24h': 'settings.window24h',
    'today': 'settings.windowToday',
    '7d': 'settings.window7d',
    '30d': 'settings.window30d',
    'current_month': 'settings.windowCurrentMonth',
    '1m': 'settings.window1m' // 保留向后兼容
  }
  return t(store.settings.locale, map[name] || name)
}

// 判断是否显示 Token 环
const showTokenRing = computed(() => store.settings.billingType === 'token' || store.settings.billingType === 'both')

// 判断是否显示请求环
const showRequestRing = computed(() => store.settings.billingType === 'request' || store.settings.billingType === 'both')

// 是否显示输入/输出详情（仅 Token 环）
const showTokenDetails = computed(() => showTokenRing.value)
</script>

<template>
  <div class="bg-white dark:bg-[#1C1C1E] rounded-xl p-2.5 shadow-sm border border-gray-100 dark:border-neutral-800 cursor-pointer active:scale-[0.98] transition-transform">
    <!-- 窗口标题 (左上角) 与 状态徽章 (右上角) -->
    <div class="flex items-center justify-between mb-2 min-h-[22px]">
      <span class="text-sm font-bold text-gray-800 dark:text-gray-100 tracking-wide">{{ getWindowLabel(window.window) }}</span>
      <!-- 醒目的状态徽章标示 -->
      <div v-if="window.riskLevel === 'critical'" class="flex items-center gap-0.5 text-[9px] font-bold tracking-wider text-red-600 dark:text-red-400 bg-red-50 dark:bg-red-500/10 px-1.5 py-0.5 rounded shadow-sm border border-red-100 dark:border-red-900 animate-pulse">
        <AlertCircle class="w-[10px] h-[10px]" />
        <span>{{ t(store.settings.locale, 'common.critical') }}</span>
      </div>
      <div v-else-if="window.riskLevel === 'warning'" class="flex items-center gap-0.5 text-[9px] font-bold tracking-wider text-orange-600 dark:text-orange-400 bg-orange-50 dark:bg-orange-500/10 px-1.5 py-0.5 rounded shadow-sm border border-orange-100 dark:border-orange-900">
        <AlertTriangle class="w-[10px] h-[10px]" />
        <span>{{ t(store.settings.locale, 'common.warning') }}</span>
      </div>
      <div v-else class="flex items-center gap-0.5 text-[9px] font-bold tracking-wider text-emerald-600 dark:text-emerald-400 bg-emerald-50 dark:bg-emerald-500/10 px-1.5 py-0.5 rounded shadow-sm border border-emerald-100 dark:border-emerald-900">
        <ShieldCheck class="w-[10px] h-[10px]" />
        <span>{{ t(store.settings.locale, 'common.safe') }}</span>
      </div>
    </div>

    <!-- 单环模式 -->
    <div v-if="isSingleRing" class="flex flex-col items-center pt-0.5 pb-0 min-h-[125px] justify-between">
      <!-- Token 环 -->
      <DonutChart
        v-if="showTokenRing"
        :percent="window.tokenPercent ?? 0"
        :risk-level="window.riskLevel"
        :size="80"
        :stroke-width="8"
        :used-value="window.tokenUsed"
        :limit-value="window.tokenLimit"
        value-type="token"
        :show-details="showTokenDetails"
        :input-tokens="window.inputTokens"
        :output-tokens="window.outputTokens"
      />
      <!-- 请求环 -->
      <DonutChart v-if="showRequestRing" :percent="window.requestPercent ?? 0" :risk-level="window.riskLevel" :size="80" :stroke-width="8" :used-value="window.requestUsed" :limit-value="window.requestLimit" value-type="request" :show-details="false" />
    </div>

    <!-- 双环模式 (Apple Fitness Rings Style) -->
    <div v-else class="flex flex-col items-center pt-0.5 pb-0 min-h-[125px] justify-between">
      <NestedDonutChart
        :token-percent="window.tokenPercent ?? 0"
        :token-risk-level="window.riskLevel"
        :token-used="window.tokenUsed"
        :token-limit="window.tokenLimit"
        :input-tokens="window.inputTokens"
        :output-tokens="window.outputTokens"
        :request-percent="window.requestPercent ?? 0"
        :request-risk-level="window.riskLevel"
        :request-used="window.requestUsed"
        :request-limit="window.requestLimit"
        :outer-size="80"
        :outer-stroke-width="8"
        :inner-size="60"
        :inner-stroke-width="8"
      />
    </div>
  </div>
</template>
