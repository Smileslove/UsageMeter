<script setup lang="ts">
import { ref } from 'vue'
import { CircleDollarSign, Database, MessageSquare, Sigma } from 'lucide-vue-next'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenPair, formatTokenValue } from '../../utils/format'
import type { AppLocale, StatisticsTotals } from '../../types'
import { useMonitorStore } from '../../stores/monitor'

const store = useMonitorStore()

const props = defineProps<{
  locale: AppLocale
  totals: StatisticsTotals | null
}>()

type DetailMetric = 'requests' | 'tokens' | 'cache'

const detailMode = ref<Record<DetailMetric, boolean>>({
  requests: false,
  tokens: false,
  cache: false
})

function value(key: 'requests' | 'tokens' | 'input' | 'output' | 'cost' | 'cacheCreate' | 'cacheRead'): string {
  const totals = props.totals
  if (!totals) return key === 'cost' ? formatCost(0, store.settings.currency) : '0'
  if (key === 'requests') return formatRequestCount(totals.requestCount)
  if (key === 'tokens') return formatTokenValue(totals.totalTokens)
  // 输入显示总输入（包含缓存读取）
  if (key === 'input') return formatTokenPair(totals.inputTokens + (totals.cacheReadTokens ?? 0), totals.outputTokens).input
  if (key === 'output') return formatTokenPair(totals.inputTokens + (totals.cacheReadTokens ?? 0), totals.outputTokens).output
  if (key === 'cost') return formatCost(totals.cost, store.settings.currency)
  if (key === 'cacheCreate') return formatTokenValue(totals.cacheCreateTokens ?? 0)
  if (key === 'cacheRead') return formatTokenValue(totals.cacheReadTokens ?? 0)
  return '0'
}

function countValue(value: number | null | undefined): string {
  return formatRequestCount(value ?? 0)
}

function splitCost(key: 'input' | 'output'): string {
  const totals = props.totals
  if (!totals || totals.cost <= 0) return formatCost(0, store.settings.currency)
  const total = totals.inputTokens + totals.outputTokens
  if (total <= 0) return formatCost(0, store.settings.currency)
  const tokens = key === 'input' ? totals.inputTokens : totals.outputTokens
  return formatCost(totals.cost * (tokens / total), store.settings.currency)
}

function cacheValue(value: number | null | undefined): string {
  if (!value || value <= 0) return '0'
  return formatTokenValue(value)
}

function toggleDetail(metric: DetailMetric) {
  detailMode.value[metric] = !detailMode.value[metric]
}

function toggleDetailByKey(event: KeyboardEvent, metric: DetailMetric) {
  if (event.key !== 'Enter' && event.key !== ' ') return
  event.preventDefault()
  toggleDetail(metric)
}

function detailedNumber(value: number | null | undefined): string {
  return new Intl.NumberFormat(props.locale).format(Math.round(value ?? 0))
}

function requestDisplay(key: 'total' | 'success' | 'error'): string {
  const totals = props.totals
  if (detailMode.value.requests) {
    if (key === 'total') return detailedNumber(totals?.requestCount)
    if (key === 'success') return detailedNumber(totals?.successRequests)
    return detailedNumber(totals?.errorRequests)
  }
  if (key === 'total') return value('requests')
  if (key === 'success') return countValue(totals?.successRequests)
  return countValue(totals?.errorRequests)
}

function tokenDisplay(key: 'total' | 'input' | 'output'): string {
  const totals = props.totals
  if (detailMode.value.tokens) {
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
  if (detailMode.value.cache) {
    if (key === 'total') return detailedNumber((totals?.cacheCreateTokens ?? 0) + (totals?.cacheReadTokens ?? 0))
    if (key === 'create') return detailedNumber(totals?.cacheCreateTokens)
    return detailedNumber(totals?.cacheReadTokens)
  }
  if (key === 'total') return cacheValue((totals?.cacheCreateTokens ?? 0) + (totals?.cacheReadTokens ?? 0))
  if (key === 'create') return value('cacheCreate')
  return value('cacheRead')
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
      :aria-pressed="detailMode.requests"
      @click="toggleDetail('requests')"
      @keydown="toggleDetailByKey($event, 'requests')"
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
            <div class="metric-detail-row text-emerald-600 dark:text-emerald-300">
              <span class="metric-dot bg-emerald-400/70"></span>
              <span class="metric-detail-label">{{ t(locale, 'statistics.successRequests') }}</span>
              <span :class="['metric-detail-value', detailPairSizeClass(requestDisplay('success'), requestDisplay('error'))]">{{ requestDisplay('success') }}</span>
            </div>
            <div class="metric-detail-row text-rose-500 dark:text-rose-300">
              <span class="metric-dot bg-rose-400/70"></span>
              <span class="metric-detail-label">{{ t(locale, 'statistics.errorRequests') }}</span>
              <span :class="['metric-detail-value', detailPairSizeClass(requestDisplay('success'), requestDisplay('error'))]">{{ requestDisplay('error') }}</span>
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
    <div class="metric-card metric-card-amber group !bg-white border-amber-200/90 dark:!bg-[#1C1C1E] dark:border-amber-500/15">
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
            <p :class="['metric-value metric-value-cost dark:!text-gray-50', metricValueSizeClass(value('cost'))]">{{ value('cost') }}</p>
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
      :aria-pressed="detailMode.cache"
      @click="toggleDetail('cache')"
      @keydown="toggleDetailByKey($event, 'cache')"
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
</style>
