<script setup lang="ts">
import { computed, ref } from 'vue'
import { CircleDollarSign, CircleX, Database, MessageSquare, Sigma, CheckCircle2 } from 'lucide-vue-next'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenValue } from '../../utils/format'
import type { AppLocale, StatisticsTotals } from '../../types'
import { useMonitorStore } from '../../stores/monitor'

const store = useMonitorStore()

const props = defineProps<{
  locale: AppLocale
  totals: StatisticsTotals | null
}>()

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

function value(key: 'requests' | 'tokens' | 'input' | 'output' | 'cost' | 'cacheCreate' | 'cacheRead'): string {
  const totals = props.totals
  if (!totals) return key === 'cost' ? formatCost(0, store.settings.currency) : '0'
  if (key === 'requests') return formatRequestCount(totals.requestCount)
  if (key === 'tokens') return formatTokenValue(totals.totalTokens)
  // 输入显示总输入（包含缓存读取）
  if (key === 'input') return formatTokenValue(totals.inputTokens + (totals.cacheReadTokens ?? 0))
  if (key === 'output') return formatTokenValue(totals.outputTokens)
  if (key === 'cost') return formatCost(totals.cost, store.settings.currency)
  if (key === 'cacheCreate') return formatTokenValue(totals.cacheCreateTokens ?? 0)
  if (key === 'cacheRead') return formatTokenValue(totals.cacheReadTokens ?? 0)
  return '0'
}


function cacheValue(value: number | null | undefined): string {
  if (!value || value <= 0) return '0'
  return formatTokenValue(value)
}

function detailedNumber(value: number | null | undefined): string {
  return new Intl.NumberFormat(props.locale).format(Math.round(value ?? 0))
}

// 请求来源拆分
const localCount  = computed(() => props.totals?.localRequestCount  ?? 0)
const proxyCount  = computed(() => props.totals?.proxyRequestCount  ?? 0)
const hasMixedSources = computed(() => localCount.value > 0 && proxyCount.value > 0)
const hasProxyRequests = computed(() => proxyCount.value > 0)

// 是否有状态码数据（代理请求才有）
const hasStatusData = computed(() => props.totals?.successRequests != null)

function requestCountDisplay(count: number): string {
  return isDetailMode.value
    ? detailedNumber(count)
    : formatRequestCount(count)
}

function requestStatusValue(raw: number | null | undefined): string {
  if (raw == null) return '--'
  return isDetailMode.value ? detailedNumber(raw) : formatRequestCount(raw)
}

function requestDisplay(key: 'total' | 'success' | 'error'): string {
  const totals = props.totals
  if (key === 'total') {
    return isDetailMode.value ? detailedNumber(totals?.requestCount) : value('requests')
  }
  if (key === 'success') return requestStatusValue(totals?.successRequests)
  return requestStatusValue(totals?.errorRequests)
}

function tokenDisplay(key: 'total' | 'input' | 'output'): string {
  const totals = props.totals
  if (isDetailMode.value) {
    if (key === 'total') return detailedNumber(totals?.totalTokens)
    if (key === 'input') return detailedNumber((totals?.inputTokens ?? 0) + (totals?.cacheReadTokens ?? 0))
    return detailedNumber(totals?.outputTokens)
  }
  if (key === 'total') return value('tokens')
  if (key === 'input') return value('input')
  return value('output')
}

function cacheDisplay(key: 'total' | 'create' | 'read'): string {
  const totals = props.totals
  if (isDetailMode.value) {
    if (key === 'total') return detailedNumber((totals?.cacheCreateTokens ?? 0) + (totals?.cacheReadTokens ?? 0))
    if (key === 'create') return detailedNumber(totals?.cacheCreateTokens)
    return detailedNumber(totals?.cacheReadTokens)
  }
  if (key === 'total') return cacheValue((totals?.cacheCreateTokens ?? 0) + (totals?.cacheReadTokens ?? 0))
  if (key === 'create') return value('cacheCreate')
  return value('cacheRead')
}

// 费用显示 - 根据状态选择币种
function costDisplay(): string {
  const totals = props.totals
  const cost = totals?.cost ?? 0
  if (showUsdCost.value) {
    return `$${cost.toFixed(4)}`
  }
  return formatCost(cost, store.settings.currency)
}

// 按输入/输出比例拆分费用
function splitCost(key: 'input' | 'output'): string {
  const totals = props.totals
  if (!totals || totals.cost <= 0) {
    return showUsdCost.value ? '$0.0000' : formatCost(0, store.settings.currency)
  }
  const total = totals.inputTokens + totals.outputTokens
  if (total <= 0) {
    return showUsdCost.value ? '$0.0000' : formatCost(0, store.settings.currency)
  }
  const tokens = key === 'input' ? totals.inputTokens : totals.outputTokens
  const splitValue = totals.cost * (tokens / total)

  if (showUsdCost.value) {
    return `$${splitValue.toFixed(4)}`
  }
  return formatCost(splitValue, store.settings.currency)
}

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
  <section class="grid grid-cols-2 gap-2">
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
          <p class="writing-vertical metric-rail-title">{{ t(locale, 'statistics.requestStats') }}</p>
        </div>
        <div class="metric-body">
          <div class="metric-total">
            <p class="metric-label">{{ t(locale, 'statistics.requests') }}</p>
            <p :class="['metric-value dark:!text-gray-50', metricValueSizeClass(requestDisplay('total'))]">{{ requestDisplay('total') }}</p>
          </div>
          <div class="metric-details">
            <!-- 混合来源：分别展示本地 / 代理行 -->
            <template v-if="hasMixedSources">
              <!-- 本地行 -->
              <div class="metric-detail-row text-slate-500 dark:text-slate-400">
                <span class="metric-dot bg-slate-400/70"></span>
                <span class="metric-detail-label">{{ t(locale, 'statistics.localRequests') }}</span>
                <span :class="['metric-detail-value', detailPairSizeClass(requestCountDisplay(localCount), requestCountDisplay(proxyCount))]">{{ requestCountDisplay(localCount) }}</span>
              </div>
              <!-- 代理行（含成功/失败子级） -->
              <div class="metric-detail-row text-emerald-600 dark:text-emerald-300">
                <span class="metric-dot bg-emerald-400/70"></span>
                <span class="metric-detail-label">
                  {{ t(locale, 'statistics.proxyRequests') }}
                  <template v-if="hasStatusData">
                    <span class="opacity-60 inline-flex items-center gap-0.5">
                      (<CheckCircle2 class="inline w-2.5 h-2.5" />{{ requestStatusValue(props.totals?.successRequests) }}
                      <CircleX class="inline w-2.5 h-2.5 text-rose-400" />{{ requestStatusValue(props.totals?.errorRequests) }})
                    </span>
                  </template>
                </span>
                <span :class="['metric-detail-value', detailPairSizeClass(requestCountDisplay(localCount), requestCountDisplay(proxyCount))]">{{ requestCountDisplay(proxyCount) }}</span>
              </div>
            </template>
            <!-- 纯代理：本地 = 0，只展示成功/失败 -->
            <template v-else-if="hasProxyRequests">
              <div class="metric-detail-row text-emerald-600 dark:text-emerald-300">
                <CheckCircle2 class="w-3 h-3 shrink-0" />
                <span class="metric-detail-label">{{ t(locale, 'statistics.successRequests') }}</span>
                <span :class="['metric-detail-value', detailPairSizeClass(requestDisplay('success'), requestDisplay('error'))]">{{ requestDisplay('success') }}</span>
              </div>
              <div class="metric-detail-row text-rose-500 dark:text-rose-300">
                <CircleX class="w-3 h-3 shrink-0" />
                <span class="metric-detail-label">{{ t(locale, 'statistics.errorRequests') }}</span>
                <span :class="['metric-detail-value', detailPairSizeClass(requestDisplay('success'), requestDisplay('error'))]">{{ requestDisplay('error') }}</span>
              </div>
            </template>
            <!-- 纯本地文件：无代理数据 -->
            <template v-else>
              <div class="metric-detail-row text-slate-500 dark:text-slate-400">
                <span class="metric-dot bg-slate-400/70"></span>
                <span class="metric-detail-label">{{ t(locale, 'statistics.localRequests') }}</span>
                <span class="metric-detail-value">{{ requestCountDisplay(localCount) }}</span>
              </div>
              <div class="metric-detail-row text-gray-400 dark:text-gray-500">
                <span class="metric-dot bg-gray-300/70 dark:bg-gray-600/70"></span>
                <span class="metric-detail-label">{{ t(locale, 'statistics.statusNotAvailable') }}</span>
              </div>
            </template>
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
          <p class="writing-vertical metric-rail-title">{{ t(locale, 'statistics.consumptionStats') }}</p>
        </div>
        <div class="metric-body">
          <div class="metric-total">
            <p class="metric-label">{{ t(locale, 'statistics.consumedTokens') }}</p>
            <p :class="['metric-value dark:!text-gray-50', metricValueSizeClass(tokenDisplay('total'))]">{{ tokenDisplay('total') }}</p>
          </div>
          <div class="metric-details">
            <div class="metric-detail-row text-sky-600 dark:text-sky-300">
              <span class="metric-dot bg-sky-400/70"></span>
              <span class="metric-detail-label">{{ t(locale, 'statistics.input') }}</span>
              <span :class="['metric-detail-value', detailPairSizeClass(tokenDisplay('input'), tokenDisplay('output'))]">{{ tokenDisplay('input') }}</span>
            </div>
            <div class="metric-detail-row text-indigo-600 dark:text-indigo-300">
              <span class="metric-dot bg-indigo-400/70"></span>
              <span class="metric-detail-label">{{ t(locale, 'statistics.output') }}</span>
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
          <p class="writing-vertical metric-rail-title">{{ t(locale, 'statistics.consumedCost') }}</p>
        </div>
        <div class="metric-body">
          <div class="metric-total">
            <p class="metric-label">{{ t(locale, 'statistics.cost') }}</p>
            <p :class="['metric-value metric-value-cost dark:!text-gray-50', metricValueSizeClass(costDisplay())]">{{ costDisplay() }}</p>
          </div>
          <div class="metric-details">
            <div class="metric-detail-row text-amber-600 dark:text-amber-300">
              <span class="metric-dot bg-amber-400/70"></span>
              <span class="metric-detail-label">{{ t(locale, 'statistics.inputCost') }}</span>
              <span class="metric-detail-value">{{ splitCost('input') }}</span>
            </div>
            <div class="metric-detail-row text-orange-600 dark:text-orange-300">
              <span class="metric-dot bg-orange-400/70"></span>
              <span class="metric-detail-label">{{ t(locale, 'statistics.outputCost') }}</span>
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
          <p class="writing-vertical metric-rail-title">{{ t(locale, 'statistics.cacheStats') }}</p>
        </div>
        <div class="metric-body">
          <div class="metric-total">
            <p class="metric-label">{{ t(locale, 'statistics.cache') }}</p>
            <p :class="['metric-value dark:!text-gray-50', metricValueSizeClass(cacheDisplay('total'))]">{{ cacheDisplay('total') }}</p>
          </div>
          <div class="metric-details">
            <div class="metric-detail-row text-violet-600 dark:text-violet-300">
              <span class="metric-dot bg-violet-400/70"></span>
              <span class="metric-detail-label">{{ t(locale, 'statistics.cacheCreateShort') }}</span>
              <span :class="['metric-detail-value', detailPairSizeClass(cacheDisplay('create'), cacheDisplay('read'))]">{{ cacheDisplay('create') }}</span>
            </div>
            <div class="metric-detail-row text-fuchsia-600 dark:text-fuchsia-300">
              <span class="metric-dot bg-fuchsia-400/70"></span>
              <span class="metric-detail-label">{{ t(locale, 'statistics.cacheReadShort') }}</span>
              <span :class="['metric-detail-value', detailPairSizeClass(cacheDisplay('create'), cacheDisplay('read'))]">{{ cacheDisplay('read') }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
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

.writing-vertical {
  writing-mode: vertical-rl;
  text-orientation: upright;
}
</style>
