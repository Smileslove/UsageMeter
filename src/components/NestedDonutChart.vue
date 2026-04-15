<script setup lang="ts">
import { computed, ref } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

const store = useMonitorStore()

const hoveredRing = ref<'token' | 'request' | null>(null)

const tokenTextClass = computed(() => {
  if (props.tokenRiskLevel === 'critical') return 'text-red-500'
  if (props.tokenRiskLevel === 'warning') return 'text-orange-500'
  return 'text-gray-700 dark:text-gray-300'
})

const requestTextClass = computed(() => {
  if (props.requestRiskLevel === 'critical') return 'text-red-500'
  if (props.requestRiskLevel === 'warning') return 'text-orange-500'
  return 'text-gray-700 dark:text-gray-300'
})

interface Props {
  // Token 环（外圈）
  tokenPercent: number
  tokenRiskLevel: 'safe' | 'warning' | 'critical'
  tokenUsed?: number
  tokenLimit?: number | null
  inputTokens?: number
  outputTokens?: number

  // 请求环（内圈）
  requestPercent: number
  requestRiskLevel: 'safe' | 'warning' | 'critical'
  requestUsed?: number
  requestLimit?: number | null

  // 尺寸设置
  outerSize?: number
  outerStrokeWidth?: number
  innerSize?: number
  innerStrokeWidth?: number
}

const props = withDefaults(defineProps<Props>(), {
  outerSize: 84,
  outerStrokeWidth: 8,
  innerSize: 58,
  innerStrokeWidth: 8
})

// 按比例平滑混合颜色
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

// Token 环（外圈）：青绿(2dd4bf) -> 亮红(ef4444)
const outerColor = computed(() => {
  const p = Math.min(100, props.tokenPercent) / 100
  return blendColors('#2dd4bf', '#ef4444', p)
})

// Request 环（内圈）：亮紫(a78bfa) -> 亮红(ef4444)
const innerColor = computed(() => {
  const p = Math.min(100, props.requestPercent) / 100
  return blendColors('#a78bfa', '#ef4444', p)
})

// SVG 计算 - 外层圆环 (Token)
const outerRadius = computed(() => (props.outerSize - props.outerStrokeWidth) / 2)
const outerCircumference = computed(() => 2 * Math.PI * outerRadius.value)
// Apple Fitness 当超过 100% 时，圆圈会继续套上一层，所以计算超过 100% 时的偏移量
const outerOffset = computed(() => {
  const p = Math.min(100, Math.max(0, props.tokenPercent))
  return outerCircumference.value * (1 - p / 100)
})
const outerOffsetLap2 = computed(() => {
  let p = props.tokenPercent % 100
  if (p === 0 && props.tokenPercent > 100) p = 100
  return outerCircumference.value * (1 - Math.max(0, p) / 100)
})
const outerCapX = computed(() => {
  const p = Math.max(0, props.tokenPercent)
  const angle = 2 * Math.PI * (p / 100)
  return props.outerSize / 2 + outerRadius.value * Math.cos(angle)
})
const outerCapY = computed(() => {
  const p = Math.max(0, props.tokenPercent)
  const angle = 2 * Math.PI * (p / 100)
  return props.outerSize / 2 + outerRadius.value * Math.sin(angle)
})

// SVG 计算 - 内层圆环 (Request)
const innerRadius = computed(() => (props.innerSize - props.innerStrokeWidth) / 2)
const innerCircumference = computed(() => 2 * Math.PI * innerRadius.value)
const innerOffset = computed(() => {
  const p = Math.min(100, Math.max(0, props.requestPercent))
  return innerCircumference.value * (1 - p / 100)
})
const innerOffsetLap2 = computed(() => {
  let p = props.requestPercent % 100
  if (p === 0 && props.requestPercent > 100) p = 100
  return innerCircumference.value * (1 - Math.max(0, p) / 100)
})
const innerCapX = computed(() => {
  const p = Math.max(0, props.requestPercent)
  const angle = 2 * Math.PI * (p / 100)
  return props.outerSize / 2 + innerRadius.value * Math.cos(angle)
})
const innerCapY = computed(() => {
  const p = Math.max(0, props.requestPercent)
  const angle = 2 * Math.PI * (p / 100)
  return props.outerSize / 2 + innerRadius.value * Math.sin(angle)
})

// 确定单位（基于最大值）
const determineUnit = (value: number): 'K' | 'M' | 'none' => {
  if (value >= 1_000_000) return 'M'
  if (value >= 1_000) return 'K'
  return 'none'
}

// 格式化 Token (基于浮点去除无效0尾数)
const formatTokenValue = (value: number, unit: 'K' | 'M' | 'none'): string => {
  if (unit === 'M') return `${parseFloat((value / 1_000_000).toFixed(2))}M`
  if (unit === 'K') return `${parseFloat((value / 1_000).toFixed(2))}K`
  return parseFloat(value.toFixed(2)).toString()
}

// 格式化请求数
const formatRequestValue = (value: number, unit: 'K' | 'M' | 'none'): string => {
  if (unit === 'M') return `${parseFloat((value / 1_000_000).toFixed(2))}M`
  if (unit === 'K') return `${parseFloat((value / 1_000).toFixed(2))}K`
  return String(Math.round(value))
}

const formattedTokenUsed = computed(() => {
  if (props.tokenUsed === undefined) return ''
  const unit = determineUnit(Math.max(props.tokenUsed, props.tokenLimit ?? 0))
  return formatTokenValue(props.tokenUsed, unit)
})
const formattedTokenLimit = computed(() => {
  if (props.tokenLimit === null || props.tokenLimit === undefined) return '∞'
  const unit = determineUnit(Math.max(props.tokenUsed ?? 0, props.tokenLimit))
  return formatTokenValue(props.tokenLimit, unit)
})

const formattedReqUsed = computed(() => {
  if (props.requestUsed === undefined) return ''
  const unit = determineUnit(Math.max(props.requestUsed, props.requestLimit ?? 0))
  return formatRequestValue(props.requestUsed, unit)
})
const formattedReqLimit = computed(() => {
  if (props.requestLimit === null || props.requestLimit === undefined) return '∞'
  const unit = determineUnit(Math.max(props.requestUsed ?? 0, props.requestLimit))
  return formatRequestValue(props.requestLimit, unit)
})
</script>

<template>
  <div class="flex flex-col w-full h-full items-center">
    <!-- 双圆环图形 -->
    <div class="relative flex items-center justify-center group" :style="{ width: outerSize + 'px', height: outerSize + 'px' }" @mouseleave="hoveredRing = null">
      <svg :width="outerSize" :height="outerSize" class="transform -rotate-90 origin-center pointer-events-none overflow-visible">
        <!-- 外层 Token 圆环 -->
        <!-- 外层背景轨 -->
        <circle :cx="outerSize / 2" :cy="outerSize / 2" :r="outerRadius" :stroke-width="outerStrokeWidth" stroke="currentColor" fill="none" class="text-gray-100 dark:text-neutral-800 transition-opacity" :class="{ 'ring-dimmed': hoveredRing === 'request' }" />
        <!-- 外层进度 -->
        <circle
          @mouseenter="hoveredRing = 'token'"
          :cx="outerSize / 2"
          :cy="outerSize / 2"
          :r="outerRadius"
          :stroke-width="outerStrokeWidth"
          :stroke="outerColor"
          fill="none"
          :stroke-dasharray="outerCircumference"
          :stroke-dashoffset="outerOffset"
          stroke-linecap="round"
          class="interactive-ring"
          :class="[hoveredRing === 'request' ? 'ring-dimmed' : '', hoveredRing === 'token' ? 'ring-hover' : '', props.tokenPercent <= 100 ? 'drop-shadow-sm' : '']"
          :style="
            {
              '--hover-stroke-width': `${outerStrokeWidth + 4}px`,
              '--hover-color': `${outerColor}60`
            } as any
          "
        />
        <!-- 外层进度Lap2 (>100%) -->
        <circle
          v-if="props.tokenPercent > 100"
          @mouseenter="hoveredRing = 'token'"
          :cx="outerSize / 2"
          :cy="outerSize / 2"
          :r="outerRadius"
          :stroke-width="outerStrokeWidth"
          :stroke="outerColor"
          fill="none"
          :stroke-dasharray="outerCircumference"
          :stroke-dashoffset="outerOffsetLap2"
          stroke-linecap="round"
          class="interactive-ring"
          :class="[hoveredRing === 'request' ? 'ring-dimmed' : '', hoveredRing === 'token' ? 'ring-hover' : '']"
          :style="
            {
              '--hover-stroke-width': `${outerStrokeWidth + 4}px`,
              '--hover-color': `${outerColor}60`
            } as any
          "
        />
        <!-- 外层套圈头部阴影盖帽 -->
        <g v-if="props.tokenPercent > 100" class="pointer-events-none transition-opacity" :class="[hoveredRing === 'request' ? 'ring-dimmed' : '']">
          <circle :cx="outerCapX" :cy="outerCapY" :r="outerStrokeWidth / 2 + 1" :fill="outerColor" style="filter: drop-shadow(0px 0px 4px rgba(0, 0, 0, 0.6))" />
          <text :x="outerCapX" :y="outerCapY" :transform="`rotate(90, ${outerCapX}, ${outerCapY})`" fill="#ffffff" font-size="6" font-weight="900" font-family="sans-serif" text-anchor="middle" dominant-baseline="central" dy="0.5">{{ Math.floor(props.tokenPercent / 100) }}x</text>
        </g>

        <!-- 内层 Request 圆环 -->
        <!-- 内层背景轨 -->
        <circle :cx="outerSize / 2" :cy="outerSize / 2" :r="innerRadius" :stroke-width="innerStrokeWidth" stroke="currentColor" fill="none" class="text-gray-100 dark:text-neutral-800 transition-opacity" :class="{ 'ring-dimmed': hoveredRing === 'token' }" />
        <!-- 内层进度 -->
        <circle
          @mouseenter="hoveredRing = 'request'"
          :cx="outerSize / 2"
          :cy="outerSize / 2"
          :r="innerRadius"
          :stroke-width="innerStrokeWidth"
          :stroke="innerColor"
          fill="none"
          :stroke-dasharray="innerCircumference"
          :stroke-dashoffset="innerOffset"
          stroke-linecap="round"
          class="interactive-ring"
          :class="[hoveredRing === 'token' ? 'ring-dimmed' : '', hoveredRing === 'request' ? 'ring-hover' : '', props.requestPercent <= 100 ? 'drop-shadow-sm' : '']"
          :style="
            {
              '--hover-stroke-width': `${innerStrokeWidth + 4}px`,
              '--hover-color': `${innerColor}60`
            } as any
          "
        />
        <!-- 内层进度Lap2 (>100%) -->
        <circle
          v-if="props.requestPercent > 100"
          @mouseenter="hoveredRing = 'request'"
          :cx="outerSize / 2"
          :cy="outerSize / 2"
          :r="innerRadius"
          :stroke-width="innerStrokeWidth"
          :stroke="innerColor"
          fill="none"
          :stroke-dasharray="innerCircumference"
          :stroke-dashoffset="innerOffsetLap2"
          stroke-linecap="round"
          class="interactive-ring"
          :class="[hoveredRing === 'token' ? 'ring-dimmed' : '', hoveredRing === 'request' ? 'ring-hover' : '']"
          :style="
            {
              '--hover-stroke-width': `${innerStrokeWidth + 4}px`,
              '--hover-color': `${innerColor}60`
            } as any
          "
        />
        <!-- 内层套圈头部阴影盖帽 -->
        <g v-if="props.requestPercent > 100" class="pointer-events-none transition-opacity" :class="[hoveredRing === 'token' ? 'ring-dimmed' : '']">
          <circle :cx="innerCapX" :cy="innerCapY" :r="innerStrokeWidth / 2 + 1" :fill="innerColor" style="filter: drop-shadow(0px 0px 4px rgba(0, 0, 0, 0.6))" />
          <text :x="innerCapX" :y="innerCapY" :transform="`rotate(90, ${innerCapX}, ${innerCapY})`" fill="#ffffff" font-size="6" font-weight="900" font-family="sans-serif" text-anchor="middle" dominant-baseline="central" dy="0.5">{{ Math.floor(props.requestPercent / 100) }}x</text>
        </g>
      </svg>

      <!-- 鼠标悬浮居中百分比展示 -->
      <transition enter-active-class="transition-all duration-200 ease-out" enter-from-class="opacity-0 scale-90" enter-to-class="opacity-100 scale-100" leave-active-class="transition-all duration-150 ease-in" leave-from-class="opacity-100 scale-100" leave-to-class="opacity-0 scale-90">
        <div v-if="hoveredRing" class="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
          <!-- 这里文字大小可根据实际圆心区域留白调整。由于双环占据了较多空间，文字大小相对克制 -->
          <span class="text-[17px] font-mono font-bold tracking-tight drop-shadow-md transition-colors" :class="hoveredRing === 'token' ? 'text-teal-600 dark:text-teal-400' : 'text-purple-600 dark:text-purple-400'">
            {{ hoveredRing === 'token' ? Math.round(tokenPercent) : Math.round(requestPercent) }}%
          </span>
        </div>
      </transition>
    </div>

    <!-- 统一底部的图例说明 / 数值区 -->
    <div class="mt-1.5 flex flex-col w-full px-1 gap-0.5">
      <!-- Token 数据 -->
      <div class="flex items-center justify-between text-[11px] font-mono">
        <div class="flex items-center gap-1.5 min-w-0">
          <div class="w-1.5 h-1.5 rounded-full shrink-0" :style="{ backgroundColor: outerColor }"></div>
          <span class="text-gray-500 dark:text-gray-400 truncate">{{ t(store.settings.locale, 'common.token') }}</span>
        </div>
        <div class="whitespace-nowrap ml-1 transition-colors" :class="tokenTextClass">
          <span class="font-semibold">{{ Math.round(tokenPercent) }}%</span>
          <span class="text-[10px] opacity-70"> ({{ formattedTokenUsed }}/{{ formattedTokenLimit }})</span>
        </div>
      </div>

      <!-- 请求数据 -->
      <div class="flex items-center justify-between text-[11px] font-mono leading-tight">
        <div class="flex items-center gap-1.5 min-w-0">
          <div class="w-1.5 h-1.5 rounded-full shrink-0" :style="{ backgroundColor: innerColor }"></div>
          <span class="text-gray-500 dark:text-gray-400 truncate">{{ t(store.settings.locale, 'common.requests') }}</span>
        </div>
        <div class="whitespace-nowrap ml-1 transition-colors" :class="requestTextClass">
          <span class="font-semibold">{{ Math.round(requestPercent) }}%</span>
          <span class="text-[10px] opacity-70"> ({{ formattedReqUsed }}/{{ formattedReqLimit }})</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.interactive-ring {
  pointer-events: stroke;
  cursor: pointer;
  transition: all 0.4s cubic-bezier(0.34, 1.56, 0.64, 1);
}
.ring-hover {
  stroke-width: var(--hover-stroke-width);
  filter: drop-shadow(0 0 4px var(--hover-color));
}
.ring-dimmed {
  opacity: 0.3;
}
</style>
