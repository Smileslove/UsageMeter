<script setup lang="ts">
import { ref, computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t, windowNameLabel } from '../i18n'
import { MessageSquare, Sigma, CircleDollarSign, Database } from 'lucide-vue-next'
import { formatRequestCount, formatTokenValue, formatCost } from '../utils/format'
import { WINDOW_ORDER, type WindowName } from '../types'
import SubscriptionQuotaCard from './SubscriptionQuotaCard.vue'
import GeminiSubscriptionQuotaCard from './GeminiSubscriptionQuotaCard.vue'

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
    store.settings.summaryWindow = window
    await store.saveSettings()
    await store.refreshUsage()
  } catch (e) {
    console.error('Failed to save settings:', e)
  }
}

// 速率摘要数据
const rateSummary = computed(() => store.rateSummary)
// 是否有速率数据（窗口匹配且有实际请求时展示速率）
const hasRateData = computed(() => {
  if (!rateSummary.value) return false
  if (rateSummary.value.window !== store.settings.summaryWindow) return false
  // 必须有实际请求才算有数据，避免错误时返回的空统计显示为 0.00
  return rateSummary.value.overall.requestCount > 0
})

const avgResponseDisplay = computed(() => {
  if (!hasRateData.value || !rateSummary.value) return '--'
  const value = rateSummary.value.ttft.avgTtftMs
  if (!value || value <= 0) return '--'
  if (value >= 1000) return `${(value / 1000).toFixed(2)}s`
  return `${Math.round(value)}ms`
})

// 格式化速率显示（保留两位小数）
const formatRate = (rate: number): string => {
  if (rate === 0) return '0.00'
  return rate.toFixed(2)
}

// 详细模式状态 - 统一控制所有卡片
const isDetailMode = ref(false)  // 详细模式（千位分隔符）
const showUsdCost = ref(false)   // 费用显示美元

// 统一切换所有状态：详细模式 + 费用币种
function toggleAllModes() {
  isDetailMode.value = !isDetailMode.value
  showUsdCost.value = !showUsdCost.value
}

function toggleAllModesByKey(event: KeyboardEvent) {
  if (event.key !== 'Enter' && event.key !== ' ') return
  event.preventDefault()
  toggleAllModes()
}

// 详细数字格式化（使用千位分隔符显示精确数字）
function detailedNumber(value: number | null | undefined): string {
  return new Intl.NumberFormat(store.settings.locale).format(Math.round(value ?? 0))
}

// 请求数显示
function requestDisplay(): string {
  const data = summaryWindowData.value
  if (!data) return '0'
  if (isDetailMode.value) {
    return detailedNumber(data.requestUsed)
  }
  return formatRequestCount(data.requestUsed)
}

// Token 显示
function tokenDisplay(key: 'total' | 'input' | 'output'): string {
  const data = summaryWindowData.value
  if (!data) return key === 'total' ? '0' : '0'
  if (isDetailMode.value) {
    if (key === 'total') return detailedNumber(data.tokenUsed)
    if (key === 'input') return detailedNumber(totalInputTokens.value)
    return detailedNumber(data.outputTokens)
  }
  if (key === 'total') return formatTokenValue(data.tokenUsed)
  if (key === 'input') return formatTokenValue(totalInputTokens.value)
  return formatTokenValue(data.outputTokens)
}

// 缓存显示
function cacheDisplay(key: 'total' | 'create' | 'read'): string {
  const data = summaryWindowData.value
  if (!data) return '0'
  const create = data.cacheCreateTokens ?? 0
  const read = data.cacheReadTokens ?? 0
  if (isDetailMode.value) {
    if (key === 'total') return detailedNumber(create + read)
    if (key === 'create') return detailedNumber(create)
    return detailedNumber(read)
  }
  if (key === 'total') return formatTokenValue(create + read)
  if (key === 'create') return formatTokenValue(create)
  return formatTokenValue(read)
}

// 费用显示 - 根据状态选择币种
const costDisplay = computed(() => {
  const cost = summaryWindowData.value?.cost ?? 0
  if (showUsdCost.value) {
    return `$${cost.toFixed(4)}`
  }
  return formatCost(cost, store.settings.currency)
})

// 按输入/输出比例拆分费用
function splitCost(key: 'input' | 'output'): string {
  const data = summaryWindowData.value
  if (!data || !data.cost || data.cost <= 0) {
    return showUsdCost.value ? '$0.0000' : formatCost(0, store.settings.currency)
  }
  const total = data.inputTokens + data.outputTokens
  if (total <= 0) {
    return showUsdCost.value ? '$0.0000' : formatCost(0, store.settings.currency)
  }
  const tokens = key === 'input' ? data.inputTokens : data.outputTokens
  const splitValue = data.cost * (tokens / total)

  if (showUsdCost.value) {
    return `$${splitValue.toFixed(4)}`
  }
  return formatCost(splitValue, store.settings.currency)
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

</script>

<template>
  <div v-if="summaryWindowData" class="summary-panel dark:!border-white/10 dark:!bg-[#0F1013]">
    <!-- 时间范围选择器 -->
    <div class="summary-window-tabs">
      <button
        v-for="window in WINDOW_ORDER"
        :key="window"
        :class="[
          'summary-window-tab',
          store.settings.summaryWindow === window ? 'active' : ''
        ]"
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
        :aria-pressed="isDetailMode"
        @click="toggleAllModes()"
        @keydown="toggleAllModesByKey($event)"
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
              <div v-if="hasRateData" class="metric-detail-row text-emerald-600 dark:text-emerald-300">
                <span class="metric-dot bg-emerald-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'overview.responseShort') }}</span>
                <span class="metric-detail-value ml-auto text-right">{{ avgResponseDisplay }}</span>
              </div>
              <div v-if="hasRateData" class="metric-detail-row text-emerald-600 dark:text-emerald-300">
                <span class="metric-dot bg-emerald-400/70"></span>
                <span class="metric-detail-label">{{ t(store.settings.locale, 'common.avgRate') }}</span>
                <span class="metric-detail-value ml-auto text-right">{{ formatRate(rateSummary!.overall.avgTokensPerSecond) }} t/s</span>
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
        :aria-pressed="isDetailMode"
        @click="toggleAllModes()"
        @keydown="toggleAllModesByKey($event)"
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
      <div
        class="metric-card metric-card-toggle metric-card-amber group !bg-white border-amber-200/90 dark:!bg-[#1C1C1E] dark:border-amber-500/15"
        role="button"
        tabindex="0"
        :aria-pressed="showUsdCost"
        @click="toggleAllModes()"
        @keydown="toggleAllModesByKey($event)"
      >
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
        :aria-pressed="isDetailMode"
        @click="toggleAllModes()"
        @keydown="toggleAllModesByKey($event)"
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

    <!-- 订阅配额卡片（仅在 ChatGPT OAuth 模式下显示） -->
    <SubscriptionQuotaCard v-if="store.hasChatGptOAuth" />

    <!-- Gemini 额度卡片（仅在检测到 Gemini CLI OAuth 凭据时显示） -->
    <GeminiSubscriptionQuotaCard v-if="store.hasGeminiOAuth" />
  </div>
</template>

<style scoped>
.summary-panel {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
  padding: 0.5rem;
  background: var(--theme-surface-gradient);
  border-radius: 0.75rem;
  border: 1px solid var(--theme-border-default);
  box-shadow: var(--theme-shadow-inline);
}

.summary-window-tabs {
  display: flex;
  gap: 0.25rem;
  padding: 0.125rem;
  background: color-mix(in srgb, var(--theme-text-primary) 10%, transparent);
  border-radius: 0.5rem;
}

:root[data-appearance='dark'] .summary-window-tabs {
  background: var(--theme-dark-track-fill);
}

.summary-window-tab {
  flex: 1;
  padding: 0.25rem 0.5rem;
  font-size: 11px;
  font-weight: 500;
  color: var(--theme-text-tertiary);
  background: transparent;
  border: none;
  border-radius: 0.375rem;
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
  text-align: center;
}

.summary-window-tab:hover {
  color: var(--theme-text-primary);
  background: var(--theme-bg-hover);
}

.summary-window-tab.active {
  color: var(--theme-accent-contrast);
  background: var(--theme-accent-primary);
  font-weight: 600;
  box-shadow: 0 2px 6px color-mix(in srgb, var(--theme-accent-primary) 28%, transparent);
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
  --metric-separator-soft: color-mix(in srgb, var(--theme-text-primary) 8%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-text-primary) 16%, transparent);
  min-width: 0;
  overflow: hidden;
  min-height: 108px;
  border-radius: 1rem;
  border-width: 1px;
  background: var(--theme-surface-gradient);
  border-color: var(--theme-border-default);
  box-shadow: var(--theme-shadow-inline);
}

.metric-card-emerald {
  --metric-separator-soft: color-mix(in srgb, var(--theme-chart-requests) 18%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-chart-requests) 34%, transparent);
}

.metric-card-sky {
  --metric-separator-soft: color-mix(in srgb, var(--theme-chart-tokens) 18%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-chart-tokens) 34%, transparent);
}

.metric-card-amber {
  --metric-separator-soft: color-mix(in srgb, var(--theme-chart-cost) 18%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-chart-cost) 36%, transparent);
}

.metric-card-violet {
  --metric-separator-soft: color-mix(in srgb, var(--theme-chart-series-3) 18%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-chart-series-3) 34%, transparent);
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
  box-shadow: var(--theme-shadow-card);
}

.metric-card-toggle:focus-visible {
  outline: none;
  box-shadow:
    0 0 0 2px var(--theme-bg-overlay),
    0 0 0 4px var(--theme-ring-focus);
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
  color: var(--theme-text-tertiary);
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
  color: var(--theme-text-primary);
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
