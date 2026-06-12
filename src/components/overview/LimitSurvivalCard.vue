<script setup lang="ts">
import { computed } from 'vue'
import { Activity } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import LobeIcon from '../LobeIcon.vue'
import { formatTokenValue } from '../../utils/format'
import type { QuotaTier, SubscriptionQuota } from '../../types'

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

function pickQuota(result: { success?: boolean; quota?: SubscriptionQuota } | null): SubscriptionQuota | null {
  if (result?.success && result.quota && (result.quota.tiers?.length ?? 0) > 0) {
    return result.quota
  }
  return null
}

const claudeQuota = computed(() => pickQuota(store.claudeQuota))
const codexQuota = computed(() => pickQuota(store.subscriptionQuota))
const copilotQuota = computed(() => pickQuota(store.copilotQuota))
const geminiQuota = computed(() => pickQuota(store.geminiQuota))

const configuredSourceQuotas = computed<SubscriptionQuota[]>(() =>
  store.configuredSourceQuotas.filter(q => (q.tiers?.length ?? 0) > 0)
)

const survival = computed(() => store.limitSurvival)
const block = computed(() => survival.value?.block ?? null)
const burn = computed(() => survival.value?.burn ?? null)
const baseline = computed(() => survival.value?.baseline ?? null)

const showBurn = computed(
  () => !!burn.value && burn.value.tokensPerHour > 0 && burn.value.confidence !== 'low'
)

const hasContent = computed(() =>
  !!block.value ||
  !!baseline.value ||
  !!copilotQuota.value ||
  !!claudeQuota.value ||
  !!codexQuota.value ||
  !!geminiQuota.value ||
  configuredSourceQuotas.value.length > 0
)

const BLOCK_SECONDS = 5 * 3600
const timeElapsedPct = computed<number>(() => {
  if (!block.value) return 0
  const elapsed = BLOCK_SECONDS - Math.max(0, block.value.remainingSeconds)
  return Math.min(100, Math.max(0, (elapsed / BLOCK_SECONDS) * 100))
})

const burnText = computed(() =>
  burn.value ? t(locale.value, 'survival.perHour', { value: formatTokenValue(burn.value.tokensPerHour) }) : ''
)

const relativeText = computed(() => {
  const value = baseline.value?.relativeToBaseline
  if (!value || value <= 0) return ''
  return t(locale.value, 'survival.relativeToAvg', { x: value.toFixed(1) })
})

const avgPaceText = computed(() => {
  const avg = baseline.value?.avgTokensPerHour
  if (!avg || avg <= 0) return ''
  return t(locale.value, 'survival.avgPace', { value: formatTokenValue(avg) })
})

const localStatusText = computed(() => {
  if (block.value) return formatDurationFromSeconds(block.value.remainingSeconds)
  if (avgPaceText.value) return avgPaceText.value
  return t(locale.value, 'survival.awaitingActivity')
})

const blockUsedText = computed(() =>
  block.value ? formatTokenValue(block.value.usedTokens) : '0'
)

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

function firstWindowTier(quota: SubscriptionQuota | null): QuotaTier | undefined {
  if (!quota) return undefined
  return quota.tiers.find(tier => tier.kind !== 'balance')
}

function primaryWindowTierOf(q: SubscriptionQuota): QuotaTier | undefined {
  return q.tiers.find(tt => tt.name === 'five_hour' && tt.kind !== 'balance')
    ?? q.tiers.find(tt => tt.kind !== 'balance')
}

function balanceTierOf(q: SubscriptionQuota): QuotaTier | undefined {
  return q.tiers.find(tt => tt.kind === 'balance')
}

function remainingPercent(tier?: QuotaTier): number {
  if (!tier) return 0
  return Math.max(0, Math.min(100, 100 - tier.utilization))
}

function remainingPercentText(tier?: QuotaTier): string {
  if (!tier) return '--'
  return t(locale.value, 'survival.remainingPercent', { value: Math.round(remainingPercent(tier)) })
}

function formatBalanceTier(tier: QuotaTier): string {
  if (tier.remainingValue == null) return '--'
  const cur = tier.currency ?? ''
  const sym = cur === 'USD' ? '$' : cur === 'CNY' ? '¥' : ''
  return `${sym}${tier.remainingValue.toFixed(2)}${sym ? '' : ` ${cur}`}`
}

function formatCopilotTierCounter(tier?: QuotaTier): string {
  if (!tier) return '--'
  if (tier.maxValue == null || tier.remainingValue == null) {
    return t(locale.value, 'copilot.quota.unlimited')
  }
  return t(locale.value, 'copilot.quota.remainingTotal', {
    remaining: Math.max(0, Math.round(tier.remainingValue)),
    total: Math.max(0, Math.round(tier.maxValue)),
  })
}

function translateCopilotPlanLabel(planLabel?: string | null): string | undefined {
  const plan = planLabel?.trim().toLowerCase()
  if (!plan) return undefined
  const key = `copilot.plan.${plan}`
  const translated = t(locale.value, key)
  return translated === key ? planLabel ?? undefined : translated
}

const officialRows = computed(() => {
  const rows: Array<{
    key: string
    label: string
    icon: string
    badge?: string
    account?: string
    metric: string
    sublabel: string
    footer?: string
    barPercent: number
    loading: boolean
    refresh: () => Promise<void>
  }> = []

  const copilotTier = copilotQuota.value?.tiers.find(tier => tier.name === 'copilot_premium')
  if (copilotQuota.value && copilotTier) {
    const badge = translateCopilotPlanLabel(copilotQuota.value.planLabel)
    const chatTier = copilotQuota.value.tiers.find(tier => tier.name === 'copilot_chat')
    const completionsTier = copilotQuota.value.tiers.find(tier => tier.name === 'copilot_completions')
    rows.push({
      key: 'copilot',
      label: t(locale.value, 'copilot.label'),
      icon: 'githubcopilot',
      badge,
      account: copilotQuota.value.accountLabel || undefined,
      metric: formatCopilotTierCounter(copilotTier),
      sublabel: t(locale.value, 'copilot.quota.premium'),
      footer: `${t(locale.value, 'copilot.quota.chat')} ${formatCopilotTierCounter(chatTier)} · ${t(locale.value, 'copilot.quota.completions')} ${formatCopilotTierCounter(completionsTier)}`,
      barPercent: copilotTier.maxValue && copilotTier.remainingValue != null
        ? Math.max(0, Math.min(100, (copilotTier.remainingValue / copilotTier.maxValue) * 100))
        : 100,
      loading: store.copilotQuotaLoading,
      refresh: async () => { await store.refreshCopilotQuota() },
    })
  }

  const claudeTier = firstWindowTier(claudeQuota.value)
  if (claudeQuota.value && claudeTier) {
    rows.push({
      key: 'claude',
      label: 'Claude',
      icon: 'claude',
      metric: remainingPercentText(claudeTier),
      sublabel: t(locale.value, 'survival.window5h'),
      footer: t(locale.value, 'survival.resetIn', { time: formatResetFromIso(claudeTier.resetsAt) }),
      barPercent: remainingPercent(claudeTier),
      loading: store.claudeLoading,
      refresh: async () => { await store.refreshClaudeQuota() },
    })
  }

  const codexTier = firstWindowTier(codexQuota.value)
  if (codexQuota.value && codexTier) {
    rows.push({
      key: 'codex',
      label: t(locale.value, 'subscription.codex'),
      icon: 'codex',
      metric: remainingPercentText(codexTier),
      sublabel: t(locale.value, 'survival.window5h'),
      footer: t(locale.value, 'survival.resetIn', { time: formatResetFromIso(codexTier.resetsAt) }),
      barPercent: remainingPercent(codexTier),
      loading: store.subscriptionLoading,
      refresh: async () => { await store.refreshSubscriptionQuota() },
    })
  }

  const geminiTier = firstWindowTier(geminiQuota.value)
  if (geminiQuota.value && geminiTier) {
    const summary = geminiQuota.value.tiers
      .slice(0, 3)
      .map((tier) => {
        const labelMap: Record<string, string> = {
          gemini_pro: t(locale.value, 'subscription.geminiPro'),
          gemini_flash: t(locale.value, 'subscription.geminiFlash'),
          gemini_flash_lite: t(locale.value, 'subscription.geminiFlashLite'),
        }
        return `${labelMap[tier.name] ?? tier.name} ${Math.round(remainingPercent(tier))}%`
      })
      .join(' · ')

    rows.push({
      key: 'gemini',
      label: t(locale.value, 'subscription.gemini'),
      icon: 'geminicli',
      badge: geminiQuota.value.planLabel || undefined,
      account: geminiQuota.value.accountLabel || undefined,
      metric: remainingPercentText(geminiTier),
      sublabel: t(locale.value, 'subscription.geminiQuota'),
      footer: summary,
      barPercent: remainingPercent(geminiTier),
      loading: store.geminiQuotaLoading,
      refresh: async () => { await store.refreshGeminiQuota() },
    })
  }

  return rows
})

const TOOL_LABEL_KEYS: Record<string, string> = {
  'claude-code': 'survival.tool.claudeCode',
  codex: 'survival.tool.codex',
  opencode: 'survival.tool.opencode',
}

function toolLabelOf(q: SubscriptionQuota): string {
  if (q.provider === 'source-config') {
    return q.accountLabel || q.credentialMessage || t(locale.value, 'survival.sourceSection')
  }
  const key = q.sourceTool ? TOOL_LABEL_KEYS[q.sourceTool] : undefined
  return key ? t(locale.value, key) : (q.sourceTool ?? t(locale.value, 'survival.title'))
}

function sourceCaptionOf(q: SubscriptionQuota): string {
  if (q.provider === 'source-config') {
    return q.planLabel || q.credentialMessage || q.tool
  }
  return q.tool
}

function localBarClass(pct: number): string {
  if (pct >= 90) return 'bg-red-400'
  if (pct >= 70) return 'bg-amber-400'
  return 'bg-cyan-400'
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
        <div class="limit-stack">
          <div class="compact-row compact-row-local">
            <div class="compact-row-head">
              <div class="compact-row-main">
                <span class="compact-dot bg-cyan-400/85 dark:bg-cyan-300/70" />
                <span class="compact-title">{{ t(locale, 'survival.localWindow5h') }}</span>
                <span class="compact-subtle">{{ block ? t(locale, 'survival.active') : t(locale, 'survival.idle') }}</span>
              </div>
              <span class="compact-metric">{{ localStatusText }}</span>
            </div>

            <div v-if="block" class="compact-progress">
              <div
                class="compact-progress-fill"
                :class="localBarClass(timeElapsedPct)"
                :style="{ width: `${timeElapsedPct}%` }"
              />
            </div>

            <div class="compact-foot">
              <span v-if="block">{{ t(locale, 'survival.used') }} {{ blockUsedText }}</span>
              <span v-if="showBurn">{{ burnText }}</span>
              <span v-if="relativeText">{{ relativeText }}</span>
              <span v-if="!block && avgPaceText">{{ avgPaceText }}</span>
            </div>
          </div>

          <div v-if="officialRows.length" class="compact-group">
            <div
              v-for="row in officialRows"
              :key="row.key"
              class="compact-row"
            >
              <div class="compact-row-head">
                <div class="compact-row-main">
                  <LobeIcon :slug="row.icon" :size="14" />
                  <span class="compact-title">{{ row.label }}</span>
                  <span v-if="row.badge" class="compact-badge">{{ row.badge }}</span>
                </div>
                <div class="compact-row-actions">
                  <span class="compact-metric">{{ row.metric }}</span>
                  <button
                    class="compact-refresh"
                    :disabled="row.loading"
                    :title="t(locale, 'subscription.refresh')"
                    @click="row.refresh"
                  >
                    <svg class="w-3 h-3 text-gray-400" :class="{ 'animate-spin': row.loading }" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                    </svg>
                  </button>
                </div>
              </div>

              <div v-if="row.account" class="compact-caption">{{ row.account }}</div>
              <div class="compact-caption">{{ row.sublabel }}</div>

              <div class="compact-progress">
                <div class="compact-progress-fill bg-cyan-400" :style="{ width: `${row.barPercent}%` }" />
              </div>

              <div v-if="row.footer" class="compact-foot">
                <span>{{ row.footer }}</span>
              </div>
            </div>
          </div>

          <div v-if="configuredSourceQuotas.length" class="compact-group">
            <div class="compact-section-label">
              <span>{{ t(locale, 'survival.sourceSection') }}</span>
              <button
                class="compact-refresh"
                :disabled="store.configuredSourceLoading"
                :title="t(locale, 'subscription.refresh')"
                @click="store.forceFetchConfiguredSourceQuotas()"
              >
                <svg class="w-3 h-3 text-gray-400" :class="{ 'animate-spin': store.configuredSourceLoading }" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                </svg>
              </button>
            </div>

            <div
              v-for="(q, index) in configuredSourceQuotas"
              :key="`${q.sourceTool ?? ''}:${q.tool}:${index}`"
              class="compact-row"
            >
              <div class="compact-row-head">
                <div class="compact-row-main">
                  <span
                    class="compact-dot"
                    :class="primaryWindowTierOf(q) ? 'bg-emerald-400/85 dark:bg-emerald-300/70' : 'bg-emerald-400/85 dark:bg-emerald-300/70'"
                  />
                  <span class="compact-title">{{ toolLabelOf(q) }}</span>
                  <span class="compact-subtle">{{ sourceCaptionOf(q) }}</span>
                </div>
                <span
                  v-if="balanceTierOf(q)"
                  class="compact-metric compact-metric-balance"
                >{{ formatBalanceTier(balanceTierOf(q)!) }}</span>
                <span
                  v-else-if="primaryWindowTierOf(q)"
                  class="compact-metric"
                >{{ remainingPercentText(primaryWindowTierOf(q)) }}</span>
              </div>

              <div v-if="primaryWindowTierOf(q)" class="compact-progress">
                <div class="compact-progress-fill bg-emerald-400" :style="{ width: `${remainingPercent(primaryWindowTierOf(q))}%` }" />
              </div>

              <div v-if="primaryWindowTierOf(q)?.resetsAt" class="compact-foot">
                <span>{{ t(locale, 'survival.resetIn', { time: formatResetFromIso(primaryWindowTierOf(q)?.resetsAt) }) }}</span>
              </div>
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
  min-height: 0 !important;
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
  padding: 0.5rem 0.625rem 0.375rem;
}

.writing-vertical {
  writing-mode: vertical-rl;
  text-orientation: upright;
}

.limit-stack {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.compact-group {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
}

.compact-row {
  border-radius: 0.875rem;
  border: 1px solid color-mix(in srgb, var(--theme-text-primary) 8%, transparent);
  background: color-mix(in srgb, var(--theme-text-primary) 3%, transparent);
  padding: 0.5rem 0.625rem;
}

.compact-row-local {
  border-style: dashed;
}

.compact-row-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}

.compact-row-main {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 0.375rem;
}

.compact-row-actions {
  display: flex;
  align-items: center;
  gap: 0.25rem;
  flex-shrink: 0;
}

.compact-dot {
  width: 0.45rem;
  height: 0.45rem;
  border-radius: 999px;
  flex-shrink: 0;
}

.compact-title {
  font-size: 12px;
  font-weight: 700;
  color: var(--theme-text-primary);
  white-space: nowrap;
}

.compact-subtle,
.compact-caption,
.compact-section-label,
.compact-foot {
  font-size: 10px;
  color: var(--theme-text-tertiary);
}

.compact-subtle {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.compact-caption,
.compact-foot {
  margin-top: 0.25rem;
  line-height: 1.35;
}

.compact-foot {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
}

.compact-metric {
  font-size: 11px;
  font-weight: 700;
  color: var(--theme-text-primary);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  text-align: right;
}

.compact-metric-balance {
  color: #34c99a;
}

.compact-badge {
  border-radius: 999px;
  border: 1px solid color-mix(in srgb, var(--theme-chart-requests) 22%, transparent);
  background: color-mix(in srgb, var(--theme-chart-requests) 10%, transparent);
  padding: 0.0625rem 0.375rem;
  font-size: 9px;
  font-weight: 600;
  color: var(--theme-chart-requests);
}

.compact-progress {
  margin-top: 0.375rem;
  height: 0.375rem;
  width: 100%;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--theme-text-primary) 10%, transparent);
}

.compact-progress-fill {
  height: 100%;
  border-radius: 999px;
  transition: width 180ms ease;
}

.compact-section-label {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  font-weight: 700;
}

.compact-refresh {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 0.375rem;
  padding: 0.125rem;
  transition: background-color 0.15s ease;
}

.compact-refresh:hover {
  background: color-mix(in srgb, var(--theme-text-primary) 8%, transparent);
}
</style>
