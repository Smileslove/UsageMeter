<script setup lang="ts">
import { ref, computed, watch, onMounted } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t, windowNameLabel } from '../i18n'
import { MessageSquare, Sigma, CircleDollarSign, Database } from 'lucide-vue-next'
import { formatRequestCount, formatTokenValue, formatTokenPair, formatCost } from '../utils/format'
import { WINDOW_ORDER, type WindowName } from '../types'

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

// 获取窗口标签
const getWindowLabel = (window: string): string => {
  return windowNameLabel(store.settings.locale, window)
}

// 切换时间窗口
async function selectWindow(window: WindowName) {
  try {
    // 自动启用选中窗口的配额
    const quota = store.settings.quotas.find(q => q.window === window)
    if (quota && !quota.enabled) {
      quota.enabled = true
    }

    store.settings.summaryWindow = window
    await store.saveSettings()

    // 如果是代理模式，加载对应的速率数据
    if (store.isProxyMode) {
      store.fetchRateSummary(window)
    }
  } catch (e) {
    console.error('Failed to save settings:', e)
  }
}

// 速率摘要数据
const rateSummary = computed(() => store.rateSummary)

// 是否有速率数据（代理模式下且窗口匹配时展示速率）
const hasRateData = computed(() => {
  if (!store.isProxyMode || !rateSummary.value) return false
  return rateSummary.value.window === store.settings.summaryWindow
})

// 格式化速率显示（保留两位小数）
const formatRate = (rate: number): string => {
  if (rate === 0) return '0.00'
  return rate.toFixed(2)
}

// 详细模式状态
type DetailMetric = 'requests' | 'tokens' | 'cache'

const detailMode = ref<Record<DetailMetric, boolean>>({
  requests: false,
  tokens: false,
  cache: false
})

function toggleDetail(metric: DetailMetric) {
  detailMode.value[metric] = !detailMode.value[metric]
}

function toggleDetailByKey(event: KeyboardEvent, metric: DetailMetric) {
  if (event.key !== 'Enter' && event.key !== ' ') return
  event.preventDefault()
  toggleDetail(metric)
}

// 详细数字格式化（使用千位分隔符显示精确数字）
function detailedNumber(value: number | null | undefined): string {
  return new Intl.NumberFormat(store.settings.locale).format(Math.round(value ?? 0))
}

// 请求数显示
function requestDisplay(): string {
  const data = summaryWindowData.value
  if (!data) return '0'
  if (detailMode.value.requests) {
    return detailedNumber(data.requestUsed)
  }
  return formatRequestCount(data.requestUsed)
}

// Token 显示
function tokenDisplay(key: 'total' | 'input' | 'output'): string {
  const data = summaryWindowData.value
  if (!data) return key === 'total' ? '0' : '0'
  if (detailMode.value.tokens) {
    if (key === 'total') return detailedNumber(data.tokenUsed)
    if (key === 'input') return detailedNumber(totalInputTokens.value)
    return detailedNumber(data.outputTokens)
  }
  if (key === 'total') return formatTokenValue(data.tokenUsed)
  const pair = formatTokenPair(totalInputTokens.value, data.outputTokens)
  if (key === 'input') return pair.input
  return pair.output
}

// 缓存显示
function cacheDisplay(key: 'total' | 'create' | 'read'): string {
  const data = summaryWindowData.value
  if (!data) return '0'
  const create = data.cacheCreateTokens ?? 0
  const read = data.cacheReadTokens ?? 0
  if (detailMode.value.cache) {
    if (key === 'total') return detailedNumber(create + read)
    if (key === 'create') return detailedNumber(create)
    return detailedNumber(read)
  }
  if (key === 'total') return formatTokenValue(create + read)
  if (key === 'create') return formatTokenValue(create)
  return formatTokenValue(read)
}

// 费用显示
const costDisplay = computed(() => {
  return formatCost(summaryWindowData.value?.cost ?? 0, store.settings.currency)
})

// 按输入/输出比例拆分费用
function splitCost(key: 'input' | 'output'): string {
  const data = summaryWindowData.value
  if (!data || !data.cost || data.cost <= 0) {
    return formatCost(0, store.settings.currency)
  }
  const total = data.inputTokens + data.outputTokens
  if (total <= 0) {
    return formatCost(0, store.settings.currency)
  }
  const tokens = key === 'input' ? data.inputTokens : data.outputTokens
  return formatCost(data.cost * (tokens / total), store.settings.currency)
}

// 根据文本长度动态调整数值大小
function metricValueSizeClass(text: string): string {
  const length = text.length
  if (length >= 16) return 'metric-value-4xs'
  if (length >= 14) return 'metric-value-3xs'
  if (length >= 12) return 'metric-value-2xs'
  if (length >= 10) return 'metric-value-xs'
  if (length >= 7) return 'metric-value-sm'
  return ''
}

function detailValueSizeClass(text: string): string {
  const length = text.length
  if (length >= 16) return 'metric-detail-value-2xs'
  if (length >= 13) return 'metric-detail-value-xs'
  if (length >= 10) return 'metric-detail-value-sm'
  return ''
}

function detailPairSizeClass(first: string, second: string): string {
  return detailValueSizeClass(first.length >= second.length ? first : second)
}

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
  <div v-if="summaryWindowData" class="summary-panel">
    <!-- 时间范围选择器 -->
    <div class="summary-window-tabs">
      <button
        v-for="window in WINDOW_ORDER"
        :key="window"
        :class="['summary-window-tab', { active: store.settings.summaryWindow === window }]"
        @click="selectWindow(window)"
      >
        {{ getWindowLabel(window) }}
      </button>
    </div>

    <!-- 2x2 卡片网格 -->
    <div class="summary-grid">
      <!-- 请求统计 -->
      <div
        class="metric-card metric-card-toggle metric-card-emerald group !bg-white border-emerald-200/90 dark:!bg-[#1C1C1E] dark:border-emerald-500/15"
        role="button"
        tabindex="0"
        :aria-pressed="detailMode.requests"
        @click="toggleDetail('requests')"
        @keydown="toggleDetailByKey($event, 'requests')"
      >
        <div class="flex h-full items-stretch">
          <div class="metric-rail text-emerald-600 dark:text-emerald-300">
            <div class="metric-rail-icon text-emerald-500 dark:text-emerald-300">
              <MessageSquare class="h-3.5 w-3.5 shrink-0" />
            </div>
            <p class="writing-vertical metric-rail-title">{{ t(store.settings.locale, 'statistics.requestStats') }}</p>
          </div>
          <div class="metric-body">
            <div class="metric-total">
              <p class="metric-label">{{ t(store.settings.locale, 'statistics.requests') }}</p>
              <p :class="['metric-value dark:!text-gray-50', metricValueSizeClass(requestDisplay())]">{{ requestDisplay() }}</p>
            </div>
            <div class="metric-details">
              <!-- 有速率数据时显示速率，无速率数据时显示占位 -->
              <div v-if="hasRateData" class="metric-detail-row text-cyan-600 dark:text-cyan-300">
                <span class="metric-dot bg-cyan-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'common.avgRate') }}</span>
                <span class="metric-detail-value">{{ formatRate(rateSummary!.overall.avgTokensPerSecond) }} t/s</span>
              </div>
              <div v-else class="metric-detail-row text-gray-400 dark:text-gray-500">
                <span class="metric-dot bg-gray-300/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'common.avgRate') }}</span>
                <span class="metric-detail-value">--</span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 消耗统计 -->
      <div
        class="metric-card metric-card-toggle metric-card-sky group !bg-white border-sky-200/90 dark:!bg-[#1C1C1E] dark:border-sky-500/15"
        role="button"
        tabindex="0"
        :aria-pressed="detailMode.tokens"
        @click="toggleDetail('tokens')"
        @keydown="toggleDetailByKey($event, 'tokens')"
      >
        <div class="flex h-full items-stretch">
          <div class="metric-rail text-sky-600 dark:text-sky-300">
            <div class="metric-rail-icon text-sky-500 dark:text-sky-300">
              <Sigma class="h-3.5 w-3.5 shrink-0" />
            </div>
            <p class="writing-vertical metric-rail-title">{{ t(store.settings.locale, 'statistics.consumptionStats') }}</p>
          </div>
          <div class="metric-body">
            <div class="metric-total">
              <p class="metric-label">{{ t(store.settings.locale, 'statistics.consumedTokens') }}</p>
              <p :class="['metric-value dark:!text-gray-50', metricValueSizeClass(tokenDisplay('total'))]">{{ tokenDisplay('total') }}</p>
            </div>
            <div class="metric-details">
              <div class="metric-detail-row text-sky-600 dark:text-sky-300">
                <span class="metric-dot bg-sky-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'statistics.input') }}</span>
                <span :class="['metric-detail-value', detailPairSizeClass(tokenDisplay('input'), tokenDisplay('output'))]">{{ tokenDisplay('input') }}</span>
              </div>
              <div class="metric-detail-row text-indigo-600 dark:text-indigo-300">
                <span class="metric-dot bg-indigo-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'statistics.output') }}</span>
                <span :class="['metric-detail-value', detailPairSizeClass(tokenDisplay('input'), tokenDisplay('output'))]">{{ tokenDisplay('output') }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 消耗金额 -->
      <div class="metric-card metric-card-amber group !bg-white border-amber-200/90 dark:!bg-[#1C1C1E] dark:border-amber-500/15">
        <div class="flex h-full items-stretch">
          <div class="metric-rail text-amber-600 dark:text-amber-300">
            <div class="metric-rail-icon text-amber-500 dark:text-amber-300">
              <CircleDollarSign class="h-3.5 w-3.5 shrink-0" />
            </div>
            <p class="writing-vertical metric-rail-title">{{ t(store.settings.locale, 'statistics.consumedCost') }}</p>
          </div>
          <div class="metric-body">
            <div class="metric-total">
              <p class="metric-label">{{ t(store.settings.locale, 'statistics.cost') }}</p>
              <p :class="['metric-value metric-value-cost dark:!text-gray-50', metricValueSizeClass(costDisplay)]">{{ costDisplay }}</p>
            </div>
            <div class="metric-details">
              <div class="metric-detail-row text-amber-600 dark:text-amber-300">
                <span class="metric-dot bg-amber-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'statistics.inputCost') }}</span>
                <span class="metric-detail-value">{{ splitCost('input') }}</span>
              </div>
              <div class="metric-detail-row text-orange-600 dark:text-orange-300">
                <span class="metric-dot bg-orange-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'statistics.outputCost') }}</span>
                <span class="metric-detail-value">{{ splitCost('output') }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 缓存统计 -->
      <div
        class="metric-card metric-card-toggle metric-card-violet group !bg-white border-violet-200/90 dark:!bg-[#1C1C1E] dark:border-violet-500/15"
        role="button"
        tabindex="0"
        :aria-pressed="detailMode.cache"
        @click="toggleDetail('cache')"
        @keydown="toggleDetailByKey($event, 'cache')"
      >
        <div class="flex h-full items-stretch">
          <div class="metric-rail text-violet-600 dark:text-violet-300">
            <div class="metric-rail-icon text-violet-500 dark:text-violet-300">
              <Database class="h-3.5 w-3.5 shrink-0" />
            </div>
            <p class="writing-vertical metric-rail-title">{{ t(store.settings.locale, 'statistics.cacheStats') }}</p>
          </div>
          <div class="metric-body">
            <div class="metric-total">
              <p class="metric-label">{{ t(store.settings.locale, 'statistics.cache') }}</p>
              <p :class="['metric-value dark:!text-gray-50', metricValueSizeClass(cacheDisplay('total'))]">{{ cacheDisplay('total') }}</p>
            </div>
            <div class="metric-details">
              <div class="metric-detail-row text-violet-600 dark:text-violet-300">
                <span class="metric-dot bg-violet-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'statistics.cacheCreateShort') }}</span>
                <span :class="['metric-detail-value', detailPairSizeClass(cacheDisplay('create'), cacheDisplay('read'))]">{{ cacheDisplay('create') }}</span>
              </div>
              <div class="metric-detail-row text-fuchsia-600 dark:text-fuchsia-300">
                <span class="metric-dot bg-fuchsia-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'statistics.cacheReadShort') }}</span>
                <span :class="['metric-detail-value', detailPairSizeClass(cacheDisplay('create'), cacheDisplay('read'))]">{{ cacheDisplay('read') }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.summary-panel {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
  padding: 0.5rem;
  background: #fff;
  border-radius: 0.75rem;
  border: 1px solid rgba(243, 244, 246, 1);
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.025);
}

:global(html.dark) .summary-panel {
  background: #1c1c1e !important;
  border-color: rgba(38, 38, 38, 1);
}

.summary-window-tabs {
  display: flex;
  gap: 0.25rem;
  padding: 0.125rem;
  background: rgba(243, 244, 246, 0.5);
  border-radius: 0.5rem;
}

:global(html.dark) .summary-window-tabs {
  background: rgba(55, 65, 81, 0.3);
}

.summary-window-tab {
  flex: 1;
  padding: 0.25rem 0.5rem;
  font-size: 11px;
  font-weight: 500;
  color: #6b7280;
  background: transparent;
  border: none;
  border-radius: 0.375rem;
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
  text-align: center;
}

.summary-window-tab:hover {
  color: #374151;
  background: rgba(255, 255, 255, 0.5);
}

:global(html.dark) .summary-window-tab:hover {
  color: #d1d5db;
  background: rgba(75, 85, 99, 0.3);
}

.summary-window-tab.active {
  color: #0f766e;
  background: #fff;
  font-weight: 600;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
}

:global(html.dark) .summary-window-tab.active {
  color: #5eead4;
  background: rgba(30, 41, 59, 0.8);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
}

.summary-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 0.5rem;
  flex: 1;
  min-width: 0;
}

/* Metric card styles - aligned with StatisticsMetricCards */
.metric-card {
  --metric-separator-soft: rgba(17, 24, 39, 0.1);
  --metric-separator-strong: rgba(17, 24, 39, 0.18);
  min-width: 0;
  overflow: hidden;
  min-height: 108px;
  border-radius: 1rem;
  border-width: 1px;
  background: #fff;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.025);
}

.metric-card-emerald {
  --metric-separator-soft: rgba(16, 185, 129, 0.18);
  --metric-separator-strong: rgba(16, 185, 129, 0.34);
}

.metric-card-sky {
  --metric-separator-soft: rgba(14, 165, 233, 0.18);
  --metric-separator-strong: rgba(14, 165, 233, 0.34);
}

.metric-card-amber {
  --metric-separator-soft: rgba(245, 158, 11, 0.2);
  --metric-separator-strong: rgba(245, 158, 11, 0.38);
}

.metric-card-violet {
  --metric-separator-soft: rgba(139, 92, 246, 0.18);
  --metric-separator-strong: rgba(139, 92, 246, 0.34);
}

.metric-card-toggle {
  cursor: pointer;
  outline: none;
  transition:
    border-color 0.15s ease,
    box-shadow 0.15s ease,
    transform 0.15s ease;
}

.metric-card-toggle:hover {
  box-shadow: 0 3px 14px rgba(0, 0, 0, 0.04);
}

.metric-card-toggle:focus-visible {
  box-shadow:
    0 0 0 2px rgba(255, 255, 255, 0.9),
    0 0 0 4px var(--metric-separator-strong);
}

:global(html.dark) .metric-card {
  background: #1c1c1e !important;
}

:global(html.dark) .metric-card-emerald {
  --metric-separator-soft: rgba(52, 211, 153, 0.34);
  --metric-separator-strong: rgba(52, 211, 153, 0.72);
}

:global(html.dark) .metric-card-sky {
  --metric-separator-soft: rgba(56, 189, 248, 0.34);
  --metric-separator-strong: rgba(56, 189, 248, 0.72);
}

:global(html.dark) .metric-card-amber {
  --metric-separator-soft: rgba(251, 191, 36, 0.36);
  --metric-separator-strong: rgba(251, 191, 36, 0.76);
}

:global(html.dark) .metric-card-violet {
  --metric-separator-soft: rgba(167, 139, 250, 0.34);
  --metric-separator-strong: rgba(167, 139, 250, 0.72);
}

:global(html.dark) .metric-value {
  color: #f9fafb !important;
}

:global(html.dark) .metric-label {
  color: #8b8b92;
}

.metric-rail {
  position: relative;
  display: flex;
  width: 2.5rem;
  flex-shrink: 0;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.375rem;
  padding: 0.5rem 0.25rem;
}

.metric-rail::after {
  position: absolute;
  top: 50%;
  right: 0;
  width: 1px;
  height: 4.5rem;
  content: '';
  transform: translateY(-50%);
  background: linear-gradient(
    to bottom,
    transparent,
    var(--metric-separator-strong) 18%,
    var(--metric-separator-strong) 82%,
    transparent
  );
}

.metric-rail-icon {
  display: flex;
  width: 1.125rem;
  height: 1.125rem;
  align-items: center;
  justify-content: center;
  transform: translateY(-0.1875rem);
}

.metric-rail-title {
  display: block;
  flex-shrink: 0;
  overflow: visible;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0;
  line-height: 1.08;
  transform: translateY(-0.1875rem);
  white-space: nowrap;
}

.metric-body {
  min-width: 0;
  flex: 1 1 0%;
  padding: 0.625rem 0.625rem 0.5rem;
}

.metric-total {
  position: relative;
  min-width: 0;
  height: 2.875rem;
  padding-bottom: 0.25rem;
}

.metric-total::after {
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  height: 1px;
  content: '';
  background: linear-gradient(to right, var(--metric-separator-strong), var(--metric-separator-soft), transparent);
}

.metric-label {
  overflow: hidden;
  color: #9ca3af;
  font-size: 11px;
  font-weight: 600;
  line-height: 1rem;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.metric-value {
  min-width: 0;
  height: 1.75rem;
  overflow: hidden;
  color: #030712;
  font-family: var(--font-mono);
  font-size: 22px;
  font-weight: 700;
  letter-spacing: 0;
  line-height: 1.75rem;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.metric-value-sm {
  font-size: 18px;
  line-height: 1.75rem;
}

.metric-value-xs {
  font-size: 16px;
  line-height: 1.75rem;
}

.metric-value-2xs {
  font-size: 14px;
  line-height: 1.75rem;
}

.metric-value-3xs {
  font-size: 12px;
  line-height: 1.75rem;
}

.metric-value-4xs {
  font-size: 10px;
  line-height: 1.75rem;
}

.metric-value-cost {
  font-size: 19px;
  line-height: 1.75rem;
}

.metric-value-cost.metric-value-sm {
  font-size: 17px;
  line-height: 1.75rem;
}

.metric-value-cost.metric-value-xs {
  font-size: 15px;
  line-height: 1.75rem;
}

.metric-value-cost.metric-value-2xs {
  font-size: 13px;
  line-height: 1.75rem;
}

.metric-value-cost.metric-value-3xs {
  font-size: 12px;
  line-height: 1.75rem;
}

.metric-value-cost.metric-value-4xs {
  font-size: 11px;
  line-height: 1.75rem;
}

.metric-details {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 0.25rem;
  margin-top: 0.375rem;
}

.metric-detail-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 0.375rem;
  font-size: 10px;
  line-height: 0.875rem;
}

.metric-dot {
  width: 0.375rem;
  height: 0.375rem;
  flex-shrink: 0;
  border-radius: 9999px;
}

.metric-detail-label {
  min-width: 0;
  flex: 1 1 0%;
  overflow: hidden;
  color: inherit;
  opacity: 0.82;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.metric-detail-value {
  min-width: max-content;
  max-width: none;
  overflow: hidden;
  color: inherit;
  font-family: var(--font-mono);
  font-weight: 700;
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.metric-detail-value-sm {
  font-size: 10px;
}

.metric-detail-value-xs {
  font-size: 9px;
}

.metric-detail-value-2xs {
  font-size: 8px;
}

.writing-vertical {
  writing-mode: vertical-rl;
  text-orientation: upright;
}
</style>
