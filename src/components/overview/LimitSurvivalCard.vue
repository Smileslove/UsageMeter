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

const localStateText = computed(() =>
  block.value ? t(locale.value, 'survival.active') : t(locale.value, 'survival.idle')
)

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

function translateCopilotPlanLabel(planLabel?: string | null): string | undefined {
  const plan = planLabel?.trim().toLowerCase()
  if (!plan) return undefined
  const key = `copilot.plan.${plan}`
  const translated = t(locale.value, key)
  return translated === key ? planLabel ?? undefined : translated
}

type CompactQuotaCard = {
  key: string
  label: string
  icon: string
  toneClass: string
  badge?: string
  summaryBadge?: string
  rows: CompactQuotaRow[]
  loading: boolean
  refresh: () => Promise<void>
}

type CompactQuotaRow = {
  key: string
  metric: string
  sublabel: string
  detailText?: string
  resetText?: string
  barPercent: number
}

function formatUsedTotal(tier?: QuotaTier): string {
  if (!tier || tier.maxValue == null) return '--'
  const total = Math.max(0, Math.round(tier.maxValue))
  const remaining = Math.max(0, Math.round(tier.remainingValue ?? 0))
  const used = Math.max(0, total - remaining)
  const peak = Math.max(used, total)
  let usedStr: string
  let totalStr: string
  if (peak >= 1_000_000) {
    usedStr = `${(used / 1_000_000).toFixed(2)}M`
    totalStr = `${(total / 1_000_000).toFixed(2)}M`
  } else if (peak >= 1_000) {
    usedStr = `${(used / 1_000).toFixed(2)}K`
    totalStr = `${Math.round(total / 1_000)}K`
  } else {
    usedStr = String(used)
    totalStr = String(total)
  }
  return `${usedStr} / ${totalStr}`
}

function usedPercentText(tier?: QuotaTier): string {
  if (!tier || tier.maxValue == null || tier.remainingValue == null || tier.maxValue <= 0) return ''
  const usedPercent = ((tier.maxValue - tier.remainingValue) / tier.maxValue) * 100
  return t(locale.value, 'survival.usedPercent', { value: usedPercent.toFixed(1) })
}

function tierLabel(name: string): string {
  switch (name) {
    case 'five_hour':
      return t(locale.value, 'subscription.fiveHour')
    case 'seven_day':
      return t(locale.value, 'subscription.sevenDay')
    case 'seven_day_sonnet':
      return `${t(locale.value, 'subscription.sevenDay')} Sonnet`
    case 'seven_day_opus':
      return `${t(locale.value, 'subscription.sevenDay')} Opus`
    case 'gemini_pro':
      return t(locale.value, 'subscription.geminiPro')
    case 'gemini_flash':
      return t(locale.value, 'subscription.geminiFlash')
    case 'gemini_flash_lite':
      return t(locale.value, 'subscription.geminiFlashLite')
    default:
      return name
    }
}

function officialTierRow(tier: QuotaTier): CompactQuotaRow {
  return {
    key: tier.name,
    metric: remainingPercentText(tier),
    sublabel: tierLabel(tier.name),
    resetText: t(locale.value, 'survival.resetIn', { time: formatResetFromIso(tier.resetsAt) }),
    barPercent: remainingPercent(tier),
  }
}

const officialRows = computed(() => {
  const rows: CompactQuotaCard[] = []

  const copilotTier = copilotQuota.value?.tiers.find(tier => tier.name === 'copilot_premium')
  if (copilotQuota.value && copilotTier) {
    const badge = translateCopilotPlanLabel(copilotQuota.value.planLabel)
    rows.push({
      key: 'copilot',
      label: t(locale.value, 'copilot.label'),
      icon: 'githubcopilot',
      toneClass: 'tone-cyan',
      badge,
      summaryBadge: t(locale.value, 'copilot.quota.premium'),
      rows: [{
        key: 'copilot_premium',
        metric: formatUsedTotal(copilotTier),
        sublabel: t(locale.value, 'copilot.quota.premium'),
        detailText: usedPercentText(copilotTier),
        resetText: t(locale.value, 'survival.resetIn', { time: formatResetFromIso(copilotTier.resetsAt) }),
        barPercent: copilotTier.maxValue && copilotTier.remainingValue != null
          ? Math.max(0, Math.min(100, ((copilotTier.maxValue - copilotTier.remainingValue) / copilotTier.maxValue) * 100))
          : 100,
      }],
      loading: store.copilotQuotaLoading,
      refresh: async () => { await store.refreshCopilotQuota() },
    })
  }

  const claudeTiers = claudeQuota.value?.tiers.filter(tier => tier.kind !== 'balance') ?? []
  if (claudeQuota.value && claudeTiers.length > 0) {
    rows.push({
      key: 'claude',
      label: 'Claude',
      icon: 'claude',
      toneClass: 'tone-amber',
      rows: claudeTiers.map(officialTierRow),
      loading: store.claudeLoading,
      refresh: async () => { await store.refreshClaudeQuota() },
    })
  }

  const codexTiers = codexQuota.value?.tiers.filter(tier => tier.kind !== 'balance') ?? []
  if (codexQuota.value && codexTiers.length > 0) {
    rows.push({
      key: 'codex',
      label: t(locale.value, 'subscription.codex'),
      icon: 'codex',
      toneClass: 'tone-sky',
      rows: codexTiers.map(officialTierRow),
      loading: store.subscriptionLoading,
      refresh: async () => { await store.refreshSubscriptionQuota() },
    })
  }

  const geminiTiers = geminiQuota.value?.tiers.filter(tier => tier.kind !== 'balance') ?? []
  if (geminiQuota.value && geminiTiers.length > 0) {
    rows.push({
      key: 'gemini',
      label: t(locale.value, 'subscription.gemini'),
      icon: 'geminicli',
      toneClass: 'tone-violet',
      badge: geminiQuota.value.planLabel || undefined,
      rows: geminiTiers.map(officialTierRow),
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

const sourceRows = computed(() =>
  configuredSourceQuotas.value.map((q, index) => {
    const windowTier = primaryWindowTierOf(q)
    const balanceTier = balanceTierOf(q)
    return {
      key: `${q.sourceTool ?? ''}:${q.tool}:${index}`,
      toneClass: 'tone-emerald',
      label: toolLabelOf(q),
      caption: sourceCaptionOf(q),
      isBalance: !!balanceTier,
      metric: balanceTier ? formatBalanceTier(balanceTier) : remainingPercentText(windowTier),
      footer: windowTier?.resetsAt
        ? t(locale.value, 'survival.resetIn', { time: formatResetFromIso(windowTier.resetsAt) })
        : undefined,
      barPercent: windowTier ? remainingPercent(windowTier) : null,
      loading: store.configuredSourceLoading,
      refresh: async () => { await store.forceFetchConfiguredSourceQuotas() },
    }
  })
)

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
          <div class="quota-strip quota-strip-local tone-cyan">
            <div class="quota-strip-head">
              <div class="quota-row-title">
                <span class="compact-dot bg-cyan-400/85 dark:bg-cyan-300/70" />
                <span class="compact-title">{{ t(locale, 'survival.localWindow5h') }}</span>
                <span class="compact-subtle">{{ localStateText }}</span>
              </div>
              <span class="quota-strip-metric quota-strip-metric-inline">{{ localStatusText }}</span>
            </div>

            <div class="quota-strip-official-body">
              <div v-if="block" class="compact-progress quota-strip-progress quota-strip-progress-official">
                <div
                  class="compact-progress-fill"
                  :class="localBarClass(timeElapsedPct)"
                  :style="{ width: `${timeElapsedPct}%` }"
                />
              </div>
              <div class="quota-strip-official-meta quota-strip-local-meta">
                <span v-if="block" class="quota-strip-caption quota-strip-caption-strong">{{ t(locale, 'survival.used') }} {{ blockUsedText }}</span>
                <span v-if="showBurn" class="quota-strip-caption quota-strip-caption-muted">{{ burnText }}</span>
                <span v-if="relativeText" class="quota-strip-caption quota-strip-caption-muted">{{ relativeText }}</span>
                <span v-if="!block && avgPaceText" class="quota-strip-caption quota-strip-caption-muted">{{ avgPaceText }}</span>
              </div>
            </div>
          </div>

          <div
            v-for="row in officialRows"
            :key="row.key"
            :class="['quota-strip', row.toneClass, { 'quota-strip-multi': row.rows.length > 1 }]"
          >
            <div class="quota-strip-head">
              <div class="quota-row-title quota-row-title-official">
                <LobeIcon :slug="row.icon" :size="14" />
                <span class="compact-title quota-title-text">{{ row.label }}</span>
                <span v-if="row.summaryBadge" class="compact-badge compact-badge-neutral">{{ row.summaryBadge }}</span>
                <span v-if="row.badge" class="compact-badge">{{ row.badge }}</span>
              </div>
              <button
                class="compact-refresh quota-strip-refresh"
                :disabled="row.loading"
                :title="t(locale, 'subscription.refresh')"
                :aria-label="t(locale, 'subscription.refresh')"
                @click="row.refresh"
              >
                <svg class="w-3 h-3 text-gray-400" :class="{ 'animate-spin': row.loading }" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                </svg>
              </button>
            </div>

            <div class="quota-strip-tier-list">
              <div
                v-for="tierRow in row.rows"
                :key="`${row.key}:${tierRow.key}`"
                :class="[
                  'quota-strip-official-body',
                  {
                    'quota-strip-official-body-tiered': row.rows.length > 1,
                    'quota-strip-tier-row': row.rows.length > 1,
                  }
                ]"
              >
                <div class="compact-progress quota-strip-progress quota-strip-progress-official">
                  <div class="compact-progress-fill" :style="{ width: `${tierRow.barPercent}%` }" />
                </div>
                <div
                  :class="[
                    'quota-strip-official-meta',
                    {
                      'quota-strip-official-meta-copilot': row.key === 'copilot',
                      'quota-strip-official-meta-tiered': row.rows.length > 1,
                    }
                  ]"
                >
                  <span
                    v-if="tierRow.detailText"
                    :class="['quota-strip-caption', row.key === 'copilot' ? 'quota-strip-caption-emphasis' : 'quota-strip-caption-muted']"
                  >{{ tierRow.detailText }}</span>
                  <span
                    v-if="row.rows.length > 1 || !row.summaryBadge"
                    class="quota-strip-caption quota-strip-caption-strong quota-strip-caption-label quota-strip-caption-tier-label"
                  >{{ tierRow.sublabel }}</span>
                  <span :class="['quota-strip-metric', 'quota-strip-metric-inline', { 'quota-strip-metric-tiered': row.rows.length > 1 }]">{{ tierRow.metric }}</span>
                  <span v-if="tierRow.resetText" class="quota-strip-caption quota-strip-caption-muted">{{ tierRow.resetText }}</span>
                </div>
              </div>
            </div>
          </div>

          <div
            v-for="row in sourceRows"
            :key="row.key"
            :class="['quota-strip', 'quota-strip-source', row.toneClass, { 'quota-strip-balance': row.isBalance }]"
          >
            <div class="quota-strip-head">
              <div class="quota-row-title quota-row-title-source">
                <span class="compact-dot quota-row-dot" />
                <span class="compact-title quota-title-text">{{ row.label }}</span>
                <span class="quota-inline-caption quota-inline-caption-source">{{ row.caption }}</span>
              </div>
              <div class="quota-strip-trailing">
                <span
                  class="quota-strip-metric quota-strip-metric-source"
                  :class="{ 'compact-metric-balance': row.isBalance }"
                >{{ row.metric }}</span>
                <button
                  class="compact-refresh quota-strip-refresh"
                  :disabled="row.loading"
                  :title="t(locale, 'subscription.refresh')"
                  :aria-label="t(locale, 'subscription.refresh')"
                  @click="row.refresh"
                >
                  <svg class="w-3 h-3 text-gray-400" :class="{ 'animate-spin': row.loading }" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                  </svg>
                </button>
              </div>
            </div>

            <div v-if="!row.isBalance && row.barPercent != null" class="quota-strip-official-body">
              <div class="compact-progress quota-strip-progress quota-strip-progress-official">
                <div class="compact-progress-fill" :style="{ width: `${row.barPercent}%` }" />
              </div>
              <div v-if="row.footer" class="quota-strip-official-meta">
                <span class="quota-strip-caption quota-strip-caption-muted">{{ row.footer }}</span>
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
  gap: 0.4375rem;
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
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.compact-subtle {
  font-size: 10px;
  font-weight: 500;
  line-height: 1.2;
  color: var(--theme-text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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

.compact-badge-neutral {
  border-color: color-mix(in srgb, var(--theme-text-primary) 14%, transparent);
  background: color-mix(in srgb, var(--theme-text-primary) 6%, transparent);
  color: var(--theme-text-secondary);
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

.quota-strip {
  --compact-accent: var(--theme-chart-requests);
  --compact-accent-soft: color-mix(in srgb, var(--compact-accent) 12%, transparent);
  --compact-accent-border: color-mix(in srgb, var(--compact-accent) 22%, transparent);
  display: flex;
  min-width: 0;
  flex-direction: column;
  border-radius: 0.875rem;
  border: 1px solid color-mix(in srgb, var(--theme-text-primary) 8%, transparent);
  background: color-mix(in srgb, var(--theme-text-primary) 3%, transparent);
  padding: 0.5rem 0.625rem;
}

.quota-strip-multi {
  padding: 0.46875rem 0.5625rem;
}

.quota-strip-head {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}

.quota-strip-trailing {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 0.25rem;
}

.quota-row-title {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 0.375rem;
}

.quota-row-title-source {
  gap: 0.3125rem;
}

.quota-row-title-official {
  flex: 1 1 auto;
}

.quota-title-text {
  flex: 1 1 auto;
}

.quota-inline-caption {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 10px;
  font-weight: 500;
  line-height: 1.2;
  color: var(--theme-text-tertiary);
}

.quota-inline-caption-source {
  font-size: 10.5px;
  font-weight: 600;
  color: color-mix(in srgb, var(--compact-accent) 82%, var(--theme-text-secondary) 18%);
}

.quota-strip-refresh {
  flex-shrink: 0;
}

.quota-strip-metric {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 14px;
  font-weight: 800;
  line-height: 1.1;
  color: var(--theme-text-primary);
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
}

.quota-strip-metric-inline {
  font-size: 12px;
  font-weight: 750;
  letter-spacing: -0.01em;
}

.quota-strip-metric-source {
  font-size: 12px;
  font-weight: 750;
}

.quota-strip-balance .quota-strip-metric-source {
  font-size: 15px;
}

.quota-strip-official-body {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 0.5rem;
  margin-top: 0.3125rem;
}

.quota-strip-official-body-tiered {
  margin-top: 0;
}

.quota-strip-tier-list {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 0.1875rem;
  margin-top: 0.25rem;
}

.quota-strip-local-body {
  margin-top: 0.375rem;
}

.quota-strip-official-meta {
  display: flex;
  min-width: 0;
  flex: 0 0 auto;
  align-items: center;
  gap: 0.375rem;
  white-space: nowrap;
}

.quota-strip-official-meta-copilot {
  flex: 0 0 auto;
  min-width: 0;
  justify-content: flex-end;
  gap: 0.3125rem;
}

.quota-strip-official-meta-tiered {
  gap: 0.25rem;
}

.quota-strip-local-meta {
  flex: 0 1 auto;
}

.quota-strip-progress-official {
  flex: 1 1 0;
  min-width: 0;
  width: 100%;
  margin-top: 0;
}

.quota-strip-caption {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 10px;
  font-weight: 500;
  line-height: 1.2;
  color: var(--theme-text-secondary);
}

.quota-strip-caption-strong {
  color: var(--theme-text-primary);
  font-weight: 700;
}

.quota-strip-caption-label {
  color: var(--theme-text-secondary);
  font-weight: 600;
}

.quota-strip-caption-tier-label {
  font-size: 9.5px;
}

.quota-strip-caption-emphasis {
  color: color-mix(in srgb, var(--compact-accent) 86%, var(--theme-text-secondary) 14%);
  font-weight: 600;
  letter-spacing: -0.01em;
}

.quota-strip-caption-muted {
  color: var(--theme-text-tertiary);
}

.quota-strip-progress {
  margin-top: 0.3125rem;
}

.quota-strip-tier-row {
  padding-top: 0.25rem;
  border-top: 1px solid color-mix(in srgb, var(--theme-text-primary) 6%, transparent);
}

.quota-strip-tier-row:first-child {
  padding-top: 0;
  border-top: 0;
}

.quota-strip-tier-row .quota-strip-progress-official {
  flex-basis: 46%;
}

.quota-strip-tier-row .compact-progress {
  height: 0.3125rem;
}

.quota-strip-metric-tiered {
  font-size: 11.5px;
}

.quota-row-dot {
  background: color-mix(in srgb, var(--compact-accent) 72%, white 28%);
}

.quota-strip .compact-progress-fill {
  background: var(--compact-accent);
}

.quota-strip .compact-badge {
  border-color: var(--compact-accent-border);
  background: var(--compact-accent-soft);
  color: var(--compact-accent);
}

.tone-cyan {
  --compact-accent: var(--theme-chart-requests);
}

.tone-sky {
  --compact-accent: var(--theme-chart-tokens);
}

.tone-violet {
  --compact-accent: var(--theme-chart-series-3);
}

.tone-amber {
  --compact-accent: var(--theme-chart-cost);
}

.tone-emerald {
  --compact-accent: #34c99a;
}

@media (max-width: 420px) {
  .quota-strip-metric {
    font-size: 13px;
  }

  .quota-strip-metric-inline {
    font-size: 11.5px;
  }
}
</style>
