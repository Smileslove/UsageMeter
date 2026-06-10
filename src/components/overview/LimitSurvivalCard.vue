<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import { Clock, Flame, Activity } from 'lucide-vue-next'
import LobeIcon from '../LobeIcon.vue'
import { formatTokenValue } from '../../utils/format'
import type { SubscriptionQuota, QuotaTier } from '../../types'

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

// ===== 真实配额（T1）：优先 Claude，其次 ChatGPT/Codex =====
function pickQuota(result: { success?: boolean; quota?: SubscriptionQuota } | null): SubscriptionQuota | null {
  if (result?.success && result.quota && (result.quota.tiers?.length ?? 0) > 0) {
    return result.quota
  }
  return null
}
const claudeQuota = computed(() => pickQuota(store.claudeQuota))
const gptQuota = computed(() => pickQuota(store.subscriptionQuota))

// 各工具已配置来源的中转额度（一行一来源；A 静默降级：空数组 = 不可得）
const configuredSourceQuotas = computed<SubscriptionQuota[]>(() =>
  store.configuredSourceQuotas.filter(q => (q.tiers?.length ?? 0) > 0)
)

// 真值窗口配额：仅官方（Claude 优先，其次 ChatGPT/Codex）。已配置来源单独成行展示。
const realQuota = computed(() => claudeQuota.value ?? gptQuota.value)
const realProvider = computed<'claude' | 'gpt' | null>(() =>
  claudeQuota.value ? 'claude' : gptQuota.value ? 'gpt' : null
)
const providerIcon = computed(() => (realProvider.value === 'claude' ? 'claude' : 'codex'))
const providerName = computed(() =>
  realProvider.value === 'claude' ? 'Claude' : t(locale.value, 'subscription.codex')
)
const refreshing = computed(() =>
  realProvider.value === 'claude' ? store.claudeLoading : store.subscriptionLoading
)

// ===== 每来源行展示辅助 =====
const TOOL_LABEL_KEYS: Record<string, string> = {
  'claude-code': 'survival.tool.claudeCode',
  codex: 'survival.tool.codex',
  opencode: 'survival.tool.opencode'
}
function toolLabelOf(q: SubscriptionQuota): string {
  if (q.provider === 'source-config') {
    return q.credentialMessage || t(locale.value, 'survival.sourceSection')
  }
  const key = q.sourceTool ? TOOL_LABEL_KEYS[q.sourceTool] : undefined
  return key ? t(locale.value, key) : (q.sourceTool ?? t(locale.value, 'survival.title'))
}
// 优先 5h 窗口，否则第一个非余额 tier
function primaryWindowTierOf(q: SubscriptionQuota): QuotaTier | undefined {
  return q.tiers.find(tt => tt.name === 'five_hour' && tt.kind !== 'balance')
    ?? q.tiers.find(tt => tt.kind !== 'balance')
}
function balanceTierOf(q: SubscriptionQuota): QuotaTier | undefined {
  return q.tiers.find(tt => tt.kind === 'balance')
}
function formatBalanceTier(tier: QuotaTier): string {
  if (tier.remainingValue == null) return ''
  const cur = tier.currency ?? ''
  const sym = cur === 'USD' ? '$' : cur === 'CNY' ? '¥' : ''
  return `${sym}${tier.remainingValue.toFixed(2)}${sym ? '' : ' ' + cur}`
}

const tiers = computed(() => realQuota.value?.tiers ?? [])
const fiveHourTier = computed(() => tiers.value.find(tier => tier.name === 'five_hour'))
const sevenDayTier = computed(() => tiers.value.find(tier => tier.name === 'seven_day'))

// ===== 本地信号（burn / 锚定块 / 基线） =====
const survival = computed(() => store.limitSurvival)
const block = computed(() => survival.value?.block ?? null)
const burn = computed(() => survival.value?.burn ?? null)
const baseline = computed(() => survival.value?.baseline ?? null)

// 是否展示 burn 子行：有速率且置信度非 low
const showBurn = computed(
  () => !!burn.value && burn.value.tokensPerHour > 0 && burn.value.confidence !== 'low'
)
const burnText = computed(() =>
  burn.value ? t(locale.value, 'survival.perHour', { value: formatTokenValue(burn.value.tokensPerHour) }) : ''
)
const relativeText = computed(() => {
  const r = baseline.value?.relativeToBaseline
  if (!r || r <= 0) return ''
  return t(locale.value, 'survival.relativeToAvg', { x: r.toFixed(1) })
})


function sourceCaptionOf(q: SubscriptionQuota): string {
  if (q.provider === 'source-config') return 'generic'
  return q.tool
}

// 卡片是否有内容：有官方配额、任一已配置来源、活跃锚定块、或有历史基线（空闲态）
const hasContent = computed(
  () => !!realQuota.value || configuredSourceQuotas.value.length > 0 || !!block.value || !!baseline.value
)

// 5h 窗口时间消耗进度（0-100），与 token 上限无关，纯时间维度
const BLOCK_SECONDS = 5 * 3600
const timeElapsedPct = computed<number>(() => {
  if (!block.value) return 0
  const elapsed = BLOCK_SECONDS - Math.max(0, block.value.remainingSeconds)
  return Math.min(100, Math.max(0, (elapsed / BLOCK_SECONDS) * 100))
})
function timeBarColor(pct: number): string {
  if (pct >= 90) return 'bg-red-400'
  if (pct >= 70) return 'bg-amber-400'
  return 'bg-cyan-400'
}

// 空闲态平时均速文本（无活跃块但有历史时）
const avgPaceText = computed(() => {
  const avg = baseline.value?.avgTokensPerHour
  if (!avg || avg <= 0) return ''
  return t(locale.value, 'survival.avgPace', { value: formatTokenValue(avg) })
})

// ===== 时间格式化 =====
function formatDurationFromSeconds(seconds: number): string {
  if (seconds <= 0) return t(locale.value, 'subscription.resetNow')
  const mins = Math.floor(seconds / 60)
  const hours = Math.floor(mins / 60)
  const days = Math.floor(hours / 24)
  const dayUnit = t(locale.value, 'subscription.unitDayShort')
  const hourUnit = t(locale.value, 'subscription.unitHourShort')
  const minuteUnit = t(locale.value, 'subscription.unitMinuteShort')
  if (days > 0) {
    const remainHours = hours % 24
    return remainHours > 0 ? `${days}${dayUnit}${remainHours}${hourUnit}` : `${days}${dayUnit}`
  }
  if (hours > 0) {
    const remainMins = mins % 60
    return remainMins > 0 ? `${hours}${hourUnit}${remainMins}${minuteUnit}` : `${hours}${hourUnit}`
  }
  return `${mins}${minuteUnit}`
}
function formatResetFromIso(resetsAt?: string): string {
  if (!resetsAt) return '--'
  const diffMs = new Date(resetsAt).getTime() - Date.now()
  return formatDurationFromSeconds(Math.floor(diffMs / 1000))
}
const blockResetText = computed(() =>
  block.value ? formatDurationFromSeconds(block.value.remainingSeconds) : '--'
)
const blockUsedText = computed(() =>
  block.value ? formatTokenValue(block.value.usedTokens) : '0'
)

// 利用率配色（与既有阈值一致）
function tierColorClass(utilization: number): string {
  if (utilization >= 90) return 'text-red-500 dark:text-red-400'
  if (utilization >= 70) return 'text-amber-500 dark:text-amber-400'
  return 'text-cyan-500 dark:text-cyan-400'
}
function tierDotClass(utilization: number): string {
  if (utilization >= 90) return 'bg-red-400/80 dark:bg-red-400/70'
  if (utilization >= 70) return 'bg-amber-400/85 dark:bg-amber-400/70'
  return 'bg-cyan-400/85 dark:bg-cyan-300/70'
}

async function refreshReal() {
  if (realProvider.value === 'claude') {
    await store.refreshClaudeQuota()
  } else if (realProvider.value === 'gpt') {
    await store.refreshSubscriptionQuota()
  }
}
</script>

<template>
  <div
    v-if="hasContent"
    class="metric-card metric-card-survival group !bg-white border-cyan-200/90 dark:!bg-[#1C1C1E] dark:border-cyan-500/15"
  >
    <div class="flex items-stretch">
      <div class="metric-rail text-cyan-600 dark:text-cyan-300">
        <div class="metric-rail-icon text-cyan-500 dark:text-cyan-300">
          <Activity class="h-3.5 w-3.5 shrink-0" />
        </div>
        <p class="writing-vertical metric-rail-title">{{ t(locale, 'survival.title') }}</p>
      </div>

      <div class="metric-body">
        <!-- ===== 真实配额模式（T1） ===== -->
        <template v-if="realQuota">
          <div class="flex items-center justify-between gap-2">
            <div class="flex min-w-0 items-center gap-1.5">
              <LobeIcon :slug="providerIcon" :size="16" class="text-cyan-600 dark:text-cyan-300" />
              <span class="text-[12px] font-semibold text-gray-900 dark:text-gray-50 truncate">{{ providerName }}</span>
            </div>
            <button
              @click="refreshReal"
              :disabled="refreshing"
              class="p-0.5 rounded hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors disabled:opacity-50"
              :title="t(locale, 'subscription.refresh')"
            >
              <svg class="w-3 h-3 text-gray-400" :class="{ 'animate-spin': refreshing }" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
              </svg>
            </button>
          </div>

          <div class="mt-1.5 grid grid-cols-2 gap-1.5">
            <div
              v-if="fiveHourTier"
              :class="['tier-chip', sevenDayTier ? '' : 'col-span-2']"
            >
              <div class="flex items-center justify-between gap-1.5">
                <span class="flex min-w-0 items-center gap-1">
                  <span class="h-1.5 w-1.5 rounded-full shrink-0" :class="tierDotClass(fiveHourTier.utilization)" />
                  <span class="text-[10px] text-gray-500 dark:text-gray-400 whitespace-nowrap">{{ t(locale, 'survival.window5h') }}</span>
                </span>
                <span class="flex shrink-0 items-center gap-1">
                  <span :class="['text-[10px] font-mono font-semibold', tierColorClass(fiveHourTier.utilization)]">{{ fiveHourTier.utilization.toFixed(0) }}%</span>
                  <span class="reset-chip"><Clock class="h-2.5 w-2.5 shrink-0" /><span class="font-mono">{{ formatResetFromIso(fiveHourTier.resetsAt) }}</span></span>
                </span>
              </div>
            </div>

            <div
              v-if="sevenDayTier"
              :class="['tier-chip', fiveHourTier ? '' : 'col-span-2']"
            >
              <div class="flex items-center justify-between gap-1.5">
                <span class="flex min-w-0 items-center gap-1">
                  <span class="h-1.5 w-1.5 rounded-full shrink-0" :class="tierDotClass(sevenDayTier.utilization)" />
                  <span class="text-[10px] text-gray-500 dark:text-gray-400 whitespace-nowrap">{{ t(locale, 'survival.weekly') }}</span>
                </span>
                <span class="flex shrink-0 items-center gap-1">
                  <span :class="['text-[10px] font-mono font-semibold', tierColorClass(sevenDayTier.utilization)]">{{ sevenDayTier.utilization.toFixed(0) }}%</span>
                  <span class="reset-chip"><Clock class="h-2.5 w-2.5 shrink-0" /><span class="font-mono">{{ formatResetFromIso(sevenDayTier.resetsAt) }}</span></span>
                </span>
              </div>
            </div>
          </div>

          <!-- burn 子行 -->
          <div v-if="showBurn || relativeText" class="mt-1.5 flex items-center gap-1.5 text-[10px] text-gray-500 dark:text-gray-400">
            <template v-if="showBurn">
              <Flame class="h-2.5 w-2.5 shrink-0 text-amber-500" />
              <span class="font-mono">{{ burnText }}</span>
            </template>
            <template v-if="relativeText">
              <span v-if="showBurn" class="text-gray-300 dark:text-gray-600">·</span>
              <span class="font-mono">{{ relativeText }}</span>
            </template>
          </div>
        </template>

        <!-- ===== 本地窗口（T3：有活跃 5h 块，无官方限额） ===== -->
        <template v-else-if="block">
          <div class="flex items-center justify-between gap-2">
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="h-1.5 w-1.5 rounded-full shrink-0 bg-cyan-400/85 dark:bg-cyan-300/70" />
              <span class="text-[11px] font-semibold text-gray-700 dark:text-gray-200 whitespace-nowrap">{{ t(locale, 'survival.localWindow5h') }}</span>
            </div>
            <span class="reset-chip"><Clock class="h-2.5 w-2.5 shrink-0" /><span class="font-mono">{{ blockResetText }}</span></span>
          </div>

          <!-- 时间进度条（代表 5h 窗口已消耗的时间，与 token 上限无关） -->
          <div class="mt-1.5 h-1 w-full overflow-hidden rounded-full bg-gray-100 dark:bg-neutral-800">
            <div
              class="h-full rounded-full transition-all"
              :class="timeBarColor(timeElapsedPct)"
              :style="{ width: timeElapsedPct + '%' }"
            />
          </div>

          <div class="mt-1.5 flex flex-wrap items-center justify-between gap-1.5 text-[10px] text-gray-500 dark:text-gray-400">
            <span>{{ t(locale, 'survival.used') }} <span class="font-mono font-semibold text-gray-700 dark:text-gray-200">{{ blockUsedText }}</span></span>
            <div class="flex items-center gap-1.5">
              <template v-if="showBurn">
                <Flame class="h-2.5 w-2.5 shrink-0 text-amber-500" />
                <span class="font-mono">{{ burnText }}</span>
              </template>
              <template v-if="relativeText">
                <span v-if="showBurn" class="text-gray-300 dark:text-gray-600">·</span>
                <span class="font-mono">{{ relativeText }}</span>
              </template>
            </div>
          </div>

          <!-- 无官方/中转额度时的底部提示（含内联刷新） -->
          <div
            v-if="!realQuota && !configuredSourceQuotas.length"
            class="mt-2 flex items-center justify-between gap-1"
          >
            <!-- 查询中：显示 loading 点动画 -->
            <div v-if="store.configuredSourceLoading" class="flex items-center gap-1 text-[9px] text-gray-400/60 dark:text-gray-600">
              <svg class="h-2.5 w-2.5 shrink-0 animate-spin" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
              </svg>
              <span>{{ t(locale, 'survival.quotaQuerying') }}</span>
            </div>
            <!-- 查询完毕无数据：显示提示 + 刷新按钮 -->
            <template v-else>
              <div class="flex items-center gap-1 text-[9px] text-gray-400/60 dark:text-gray-600">
                <svg class="h-2.5 w-2.5 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                </svg>
                <span>{{ t(locale, 'survival.noQuotaHint') }}</span>
              </div>
              <button
                @click="store.forceFetchConfiguredSourceQuotas()"
                class="p-0.5 rounded hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors"
                :title="t(locale, 'subscription.refresh')"
              >
                <svg class="w-3 h-3 text-gray-400/60" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                </svg>
              </button>
            </template>
          </div>
        </template>

        <!-- ===== 空闲态（有历史基线但当前 5h 无用量） ===== -->
        <template v-else-if="baseline">
          <div class="flex items-center justify-between gap-2">
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="h-1.5 w-1.5 rounded-full shrink-0 bg-gray-300/80 dark:bg-gray-600/70" />
              <span class="text-[11px] font-semibold text-gray-600 dark:text-gray-300 whitespace-nowrap">{{ t(locale, 'survival.localWindow5h') }}</span>
              <span class="text-[10px] text-gray-400">·</span>
              <span class="text-[10px] text-gray-400 dark:text-gray-500">{{ t(locale, 'survival.idle') }}</span>
            </div>
          </div>
          <div v-if="avgPaceText" class="mt-1.5 flex items-center gap-1.5 text-[10px] text-gray-400 dark:text-gray-500">
            <span class="font-mono">{{ avgPaceText }}</span>
          </div>
        </template>

        <!-- ===== 已配置来源额度（多工具中转，一行一来源，附加展示） ===== -->
        <div
          v-if="configuredSourceQuotas.length"
          :class="['source-section', (realQuota || block || baseline) ? 'mt-2 border-t border-gray-100 pt-2 dark:border-neutral-800' : '']"
        >
          <div class="flex items-center justify-between gap-2">
            <span class="text-[9px] font-semibold uppercase tracking-wide text-gray-400 dark:text-gray-500">{{ t(locale, 'survival.sourceSection') }}</span>
            <button
              @click="store.forceFetchConfiguredSourceQuotas()"
              :disabled="store.configuredSourceLoading"
              class="p-0.5 rounded hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors disabled:opacity-50"
              :title="t(locale, 'subscription.refresh')"
            >
              <svg class="w-3 h-3 text-gray-400" :class="{ 'animate-spin': store.configuredSourceLoading }" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
              </svg>
            </button>
          </div>
          <div class="mt-1 space-y-1">
            <div
              v-for="(q, i) in configuredSourceQuotas"
              :key="(q.sourceTool ?? '') + ':' + q.tool + ':' + i"
              class="flex items-center justify-between gap-2"
            >
              <div class="flex min-w-0 items-center gap-1.5">
                <span
                  class="h-1.5 w-1.5 rounded-full shrink-0"
                  :class="primaryWindowTierOf(q) ? tierDotClass(primaryWindowTierOf(q)!.utilization) : 'bg-emerald-400/85 dark:bg-emerald-300/70'"
                />
                <span class="text-[11px] font-semibold text-gray-700 dark:text-gray-200 truncate">{{ toolLabelOf(q) }}</span>
                <span class="text-[10px] text-gray-300 dark:text-gray-600 shrink-0">·</span>
                <span class="text-[10px] text-gray-500 dark:text-gray-400 truncate">{{ sourceCaptionOf(q) }}</span>
              </div>
              <span class="flex shrink-0 items-center gap-1">
                <template v-if="primaryWindowTierOf(q)">
                  <span :class="['text-[10px] font-mono font-semibold', tierColorClass(primaryWindowTierOf(q)!.utilization)]">{{ primaryWindowTierOf(q)!.utilization.toFixed(0) }}%</span>
                  <span v-if="primaryWindowTierOf(q)!.resetsAt" class="reset-chip"><Clock class="h-2.5 w-2.5 shrink-0" /><span class="font-mono">{{ formatResetFromIso(primaryWindowTierOf(q)!.resetsAt) }}</span></span>
                </template>
                <span
                  v-else-if="balanceTierOf(q)"
                  class="text-[11px] font-mono font-semibold text-emerald-600 dark:text-emerald-400"
                >{{ formatBalanceTier(balanceTierOf(q)!) }}</span>
              </span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.metric-card {
  --metric-separator-soft: color-mix(in srgb, var(--theme-chart-requests) 18%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-chart-requests) 34%, transparent);
  min-width: 0;
  overflow: hidden;
  border-radius: 1rem;
  border-width: 1px;
  background: var(--theme-surface-gradient);
  border-color: var(--theme-border-default);
  box-shadow: var(--theme-shadow-inline);
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
  top: 0.75rem;
  bottom: 0.75rem;
  right: 0;
  width: 1px;
  content: '';
  background: linear-gradient(to bottom, transparent, var(--metric-separator-strong) 12%, var(--metric-separator-strong) 88%, transparent);
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
  line-height: 1.08;
  transform: translateY(-0.1875rem);
  white-space: nowrap;
}

.metric-body {
  min-width: 0;
  flex: 1 1 0%;
  padding: 0.5rem 0.625rem;
  display: flex;
  flex-direction: column;
  justify-content: center;
}

.tier-chip {
  border-radius: 0.5rem;
  border: 1px solid color-mix(in srgb, var(--theme-text-primary) 8%, transparent);
  background: color-mix(in srgb, var(--theme-text-primary) 3%, transparent);
  padding: 0.3125rem 0.375rem;
}

.reset-chip {
  display: inline-flex;
  align-items: center;
  gap: 0.125rem;
  border-radius: 0.375rem;
  border: 1px solid color-mix(in srgb, var(--theme-text-primary) 8%, transparent);
  background: color-mix(in srgb, var(--theme-text-primary) 3%, transparent);
  padding: 0.0625rem 0.25rem;
  font-size: 9px;
  color: var(--theme-text-tertiary);
  white-space: nowrap;
}

.writing-vertical {
  writing-mode: vertical-rl;
  text-orientation: upright;
}
</style>
