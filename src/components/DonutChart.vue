<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

const store = useMonitorStore()

interface Props {
  percent: number
  riskLevel: 'safe' | 'warning' | 'critical'
  size?: number
  strokeWidth?: number
  // 新增 props
  usedValue?: number
  limitValue?: number | null
  valueType?: 'token' | 'request'
  showDetails?: boolean
  inputTokens?: number
  outputTokens?: number
}

const props = withDefaults(defineProps<Props>(), {
  size: 80,
  strokeWidth: 6,
  valueType: 'token',
  showDetails: false
})

// 按比例平滑混合两种颜色
const blendColors = (c1: string, c2: string, ratio: number) => {
  ratio = Math.max(0, Math.min(1, ratio))
  const hex = (c: string) => parseInt(c.slice(1), 16)
  const r1 = (hex(c1) >> 16) & 255,
    g1 = (hex(c1) >> 8) & 255,
    b1 = hex(c1) & 255
  const r2 = (hex(c2) >> 16) & 255,
    g2 = (hex(c2) >> 8) & 255,
    b2 = hex(c2) & 255
  const r = Math.round(r1 + (r2 - r1) * ratio)
  const g = Math.round(g1 + (g2 - g1) * ratio)
  const b = Math.round(b1 + (b2 - b1) * ratio)
  return `#${((1 << 24) | (r << 16) | (g << 8) | b).toString(16).slice(1)}`
}

// 基于进度百分比自动计算渐变色：初始色 -> 红色系警告色
const strokeColor = computed(() => {
  const p = Math.min(100, props.percent) / 100
  if (props.valueType === 'request') {
    // 请求数：亮紫(a78bfa) -> 亮红(ef4444)
    return blendColors('#a78bfa', '#ef4444', p)
  } else {
    // Token：青绿(2dd4bf) -> 亮红(ef4444)
    return blendColors('#2dd4bf', '#ef4444', p)
  }
})

// 根据风险级别动态设置文本颜色，从而传达警戒信号
const textClass = computed(() => {
  if (props.riskLevel === 'critical') return 'text-red-500 font-bold'
  if (props.riskLevel === 'warning') return 'text-orange-500 font-bold'
  return 'text-gray-800 dark:text-gray-100 font-bold'
})

// SVG 路径计算
const radius = computed(() => (props.size - props.strokeWidth) / 2)
const circumference = computed(() => 2 * Math.PI * radius.value)
// 取消 >100 的封顶，从而支持超过一圈
const offset = computed(() => {
  const p = Math.max(0, props.percent)
  return circumference.value * (1 - p / 100)
})

// 确定单位（基于最大值）
const determineUnit = (value: number): 'K' | 'M' | 'none' => {
  if (value >= 1_000_000) return 'M'
  if (value >= 1_000) return 'K'
  return 'none'
}

// 格式化 Token 数值（统一单位，处理无效零以省略显示）
const formatTokenValue = (value: number, unit: 'K' | 'M' | 'none'): string => {
  if (unit === 'M') return `${parseFloat((value / 1_000_000).toFixed(2))}M`
  if (unit === 'K') return `${parseFloat((value / 1_000).toFixed(2))}K`
  return parseFloat(value.toFixed(2)).toString()
}

// 格式化请求数（统一单位，基于单位显示）
const formatRequestValue = (value: number, unit: 'K' | 'M' | 'none'): string => {
  if (unit === 'M') return `${parseFloat((value / 1_000_000).toFixed(2))}M`
  if (unit === 'K') return `${parseFloat((value / 1_000).toFixed(2))}K`
  return String(Math.round(value))
}

// 获取输入输出的统一单位
const getUnifiedUnit = computed((): 'K' | 'M' | 'none' => {
  if (props.inputTokens === undefined || props.outputTokens === undefined) return 'none'
  return determineUnit(Math.max(props.inputTokens, props.outputTokens))
})

// 格式化已用值
const formattedUsed = computed(() => {
  if (props.usedValue === undefined) return ''
  if (props.valueType === 'request') {
    // 请求数：根据 usedValue 和 limitValue 确定统一单位
    const unit = determineUnit(Math.max(props.usedValue, props.limitValue ?? 0))
    return formatRequestValue(props.usedValue, unit)
  }
  // Token 类型：根据 usedValue 和 limitValue 确定单位
  const unit = determineUnit(Math.max(props.usedValue, props.limitValue ?? 0))
  return formatTokenValue(props.usedValue, unit)
})

// 格式化限额
const formattedLimit = computed(() => {
  if (props.limitValue === null || props.limitValue === undefined) return '∞'
  if (props.valueType === 'request') {
    // 请求数：根据 usedValue 和 limitValue 确定统一单位
    const unit = determineUnit(Math.max(props.usedValue ?? 0, props.limitValue))
    return formatRequestValue(props.limitValue, unit)
  }
  // Token 类型
  const unit = determineUnit(Math.max(props.usedValue ?? 0, props.limitValue))
  return formatTokenValue(props.limitValue, unit)
})

// 格式化输入/输出（统一单位）
const formattedInput = computed(() => {
  if (props.inputTokens === undefined) return ''
  return formatTokenValue(props.inputTokens, getUnifiedUnit.value)
})

const formattedOutput = computed(() => {
  if (props.outputTokens === undefined) return ''
  return formatTokenValue(props.outputTokens, getUnifiedUnit.value)
})
</script>

<template>
  <div class="flex flex-col w-full h-full items-center">
    <!-- 圆环图 -->
    <div class="relative flex items-center justify-center" :style="{ width: size + 'px', height: size + 'px' }">
      <svg :width="size" :height="size" class="transform -rotate-90 origin-center overflow-visible">
        <!-- 背景圆环 -->
        <circle :cx="size / 2" :cy="size / 2" :r="radius" :stroke-width="strokeWidth" stroke="currentColor" fill="none" class="text-gray-100 dark:text-neutral-800" />
        <!-- 进度圆环 -->
        <circle :cx="size / 2" :cy="size / 2" :r="radius" :stroke-width="strokeWidth" :stroke="strokeColor" fill="none" :stroke-dasharray="circumference" :stroke-dashoffset="offset" stroke-linecap="round" class="transition-all duration-500 ease-out drop-shadow-sm" />
      </svg>
      <!-- 中心百分比 -->
      <div class="absolute inset-0 flex items-center justify-center">
        <span class="text-[19px] font-mono transition-colors tracking-tight" :class="textClass">{{ Math.round(percent) }}%</span>
      </div>
    </div>

    <!-- 统一底部的图例说明 / 数值区 -->
    <div class="mt-3 flex flex-col w-full px-1 gap-0.5">
      <div class="flex items-center justify-between text-[12px] font-mono" v-if="usedValue !== undefined">
        <div class="flex items-center gap-1.5 min-w-0">
          <div class="w-1.5 h-1.5 rounded-full shrink-0" :style="{ backgroundColor: strokeColor }"></div>
          <span class="text-gray-500 dark:text-gray-400 truncate">
            {{ valueType === 'request' ? t(store.settings.locale, 'common.requests') : t(store.settings.locale, 'common.token') }}
          </span>
        </div>
        <div class="whitespace-nowrap ml-1 transition-colors" :class="textClass">
          <span class="font-semibold">{{ Math.round(percent) }}%</span>
          <span class="text-[10px] opacity-70"> ({{ formattedUsed }}/{{ formattedLimit }})</span>
        </div>
      </div>

      <!-- 输入/输出详情（仅 Token 环） -->
      <div v-if="showDetails && inputTokens !== undefined && outputTokens !== undefined" class="text-[10px] text-gray-400 text-right w-full pr-1">{{ t(store.settings.locale, 'statistics.input') }}: {{ formattedInput }} | {{ t(store.settings.locale, 'statistics.output') }}: {{ formattedOutput }}</div>
    </div>
  </div>
</template>
