<script setup lang="ts">
import { computed } from 'vue'
import { Activity, RefreshCw } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import LobeIcon from '../LobeIcon.vue'
import { resolveToolLobeIcon } from '../../iconConfig'
import { formatCountdownSeconds, formatRate, formatTokenValue, formatUsedTotal as formatUsedTotalPair } from '../../utils/format'
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
  const elapsed = block.value.elapsedSeconds
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
  return formatCountdownSeconds(
    seconds,
    t(locale.value, 'subscription.unitDayShort'),
    t(locale.value, 'subscription.unitHourShort'),
    t(locale.value, 'subscription.unitMinuteShort')
  )
}

function formatResetFromIso(resetsAt?: string): string {
  if (!resetsAt) return '--'
  const diffMs = new Date(resetsAt).getTime() - Date.now()
  if (!Number.isFinite(diffMs)) return '--'
  return diffMs <= 0 ? t(locale.value, 'subscription.resetNow') : formatDurationFromSeconds(Math.floor(diffMs / 1000))
}

function primaryWindowTierOf(q: SubscriptionQuota): QuotaTier | undefined {
  return q.tiers.find(tt => tt.name === 'five_hour' && tt.kind !== 'balance')
    ?? q.tiers.find(tt => tt.kind !== 'balance')
}

function balanceTierOf(q: SubscriptionQuota): QuotaTier | undefined {
  return q.tiers.find(tt => tt.kind === 'balance')
}

function remainingPercent(tier?: QuotaTier): number | null {
  if (!tier) return null
  if (tier.limitReached) return 0
  if (tier.utilizationAvailable === false) return null
  if (tier.maxValue != null && tier.maxValue > 0 && tier.remainingValue != null) {
    return Math.max(0, Math.min(100, tier.remainingValue / tier.maxValue * 100))
  }
  return Number.isFinite(tier.utilization) ? Math.max(0, Math.min(100, 100 - tier.utilization)) : null
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
  rows: CompactQuotaRow[]
  loading: boolean
  refresh: () => Promise<void>
}

type CompactQuotaRow = {
  key: string
  metric: string
  sublabel: string
  metricLabelKey: string
  detailText?: string
  resetText?: string
  barPercent: number | null
}

function formatUsedTotal(tier?: QuotaTier): string {
  if (!tier || tier.maxValue == null) return '--'
  const total = Math.max(0, Math.round(tier.maxValue))
  const remaining = Math.max(0, Math.round(tier.remainingValue ?? 0))
  const used = Math.max(0, total - remaining)
  return formatUsedTotalPair(used, total)
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
  const percent = remainingPercent(tier)
  return {
    key: tier.name,
    metric: tier.limitReached ? t(locale.value, 'desktop.overview.limitStatusExhausted') : percent == null ? '--' : `${Math.round(percent)}%`,
    metricLabelKey: 'survival.remaining',
    sublabel: tierLabel(tier.name),
    resetText: formatResetFromIso(tier.resetsAt),
    barPercent: percent,
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
      rows: [{
        key: 'copilot_premium',
        metric: formatUsedTotal(copilotTier),
        metricLabelKey: 'survival.used',
        sublabel: t(locale.value, 'copilot.quota.premium'),
        detailText: usedPercentText(copilotTier),
        resetText: formatResetFromIso(copilotTier.resetsAt),
        barPercent: copilotTier.maxValue && copilotTier.remainingValue != null
          ? Math.max(0, Math.min(100, ((copilotTier.maxValue - copilotTier.remainingValue) / copilotTier.maxValue) * 100))
          : null,
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
  hermes: 'survival.tool.hermes',
  opencode: 'survival.tool.opencode',
}

const sourceRows = computed(() =>
  configuredSourceQuotas.value.map((q, index) => {
    const windowTier = primaryWindowTierOf(q)
    const balanceTier = balanceTierOf(q)
    const windowRow = windowTier ? officialTierRow(windowTier) : null
    return {
      key: `${q.sourceTool ?? ''}:${q.tool}:${index}`,
      toneClass: 'tone-emerald',
      label: toolLabelOf(q),
      icon: resolveToolLobeIcon(q.sourceTool === 'claude-code' ? 'claude_code' : q.sourceTool),
      caption: sourceCaptionOf(q),
      isBalance: !!balanceTier,
      metric: balanceTier ? formatBalanceTier(balanceTier) : windowRow?.metric ?? '--',
      metricLabelKey: balanceTier ? 'survival.walletMetric' : 'survival.remaining',
      windowLabel: windowRow?.sublabel ?? '',
      footer: windowTier?.resetsAt
        ? formatResetFromIso(windowTier.resetsAt)
        : undefined,
      barPercent: windowRow?.barPercent ?? null,
      loading: store.configuredSourceLoading,
      refresh: async () => { await store.forceFetchConfiguredSourceQuotas() },
    }
  })
)

function toolLabelOf(q: SubscriptionQuota): string {
  const key = q.sourceTool ? TOOL_LABEL_KEYS[q.sourceTool] : undefined
  if (key) return t(locale.value, key)
  if (q.sourceTool) return q.sourceTool
  if (q.provider === 'source-config') {
    return q.accountLabel || q.credentialMessage || t(locale.value, 'survival.sourceSection')
  }
  return t(locale.value, 'survival.title')
}

function sourceCaptionOf(q: SubscriptionQuota): string {
  if (q.sourceTool && q.accountLabel) {
    return q.accountLabel
  }
  if (q.provider === 'source-config') {
    return q.planLabel || q.credentialMessage || q.tool
  }
  return q.tool
}

</script>

<template>
  <section v-if="hasContent" class="metric-card metric-card-survival" :aria-label="t(locale, 'survival.title')">
    <div class="flex items-stretch">
      <div class="metric-rail">
        <Activity :size="18" aria-hidden="true" />
        <p class="writing-vertical metric-rail-title">{{ t(locale, 'survival.title') }}</p>
      </div>

      <div class="metric-body">
        <div class="limit-stack">
          <div class="quota-strip quota-strip-local tone-emerald">
            <div class="quota-strip-head">
              <div class="quota-row-title">
                <span class="compact-dot quota-local-dot" aria-hidden="true" />
                <span class="compact-title">{{ t(locale, 'survival.localWindow5h') }}</span>
                <span class="compact-badge" :class="{ 'compact-badge-neutral': !block }">{{ localStateText }}</span>
              </div>
              <span v-if="block" class="quota-local-countdown" :title="t(locale, 'survival.remainingDuration')">{{ formatDurationFromSeconds(block.remainingSeconds) }}</span>
            </div>
            <div v-if="block" class="quota-local-overview">
              <div
                class="compact-progress quota-strip-progress"
                role="progressbar"
                :aria-label="t(locale, 'survival.timeProgress')"
                :aria-valuenow="timeElapsedPct"
                :aria-valuemin="0"
                :aria-valuemax="100"
              >
                <div class="compact-progress-fill" :style="{ width: `${timeElapsedPct}%` }" />
              </div>
              <dl class="quota-local-stats">
                <div :title="t(locale, 'survival.timeProgress')">
                  <dt>{{ t(locale, 'survival.timeProgress') }}</dt>
                  <dd>{{ formatRate(timeElapsedPct) }}%</dd>
                </div>
                <div :title="t(locale, 'survival.usedDuration')">
                  <dt>{{ t(locale, 'survival.usedDuration') }}</dt>
                  <dd>{{ formatDurationFromSeconds(block.elapsedSeconds) }} / {{ formatDurationFromSeconds(BLOCK_SECONDS) }}</dd>
                </div>
                <div class="sr-only" :title="t(locale, 'survival.remainingDuration')">
                  <dt>{{ t(locale, 'survival.remainingDuration') }}</dt>
                  <dd>{{ formatDurationFromSeconds(block.remainingSeconds) }}</dd>
                </div>
                <div class="sr-only">
                  <dt>{{ t(locale, 'survival.baselineRange') }}</dt>
                  <dd>{{ baseline ? t(locale, 'survival.last7Days') : '--' }}</dd>
                </div>
              </dl>
            </div>
            <p v-else class="quota-local-idle">{{ localStatusText }}</p>
            <div v-if="block || showBurn || relativeText" class="quota-local-context">
              <span v-if="block">{{ t(locale, 'survival.usedTokens', { value: blockUsedText }) }}</span>
              <span v-if="showBurn">{{ burnText }}</span>
              <span v-if="relativeText" :title="relativeText">{{ t(locale, 'survival.relativeToAvgCompact', { x: baseline?.relativeToBaseline?.toFixed(1) ?? '--' }) }}</span>
            </div>
          </div>

          <div v-for="row in officialRows" :key="row.key" :class="['quota-strip', row.toneClass, { 'quota-strip-codex': row.key === 'codex' }]">
            <div class="quota-strip-head quota-strip-head-official">
              <div class="quota-row-title quota-row-title-official">
                <span class="quota-provider-icon"><LobeIcon :slug="row.icon" :size="16" :class="{ 'dark:invert': row.icon === 'githubcopilot' || row.icon === 'cursor' }" /></span>
                <span class="compact-title quota-title-text" :title="row.label">{{ row.label }}</span>
                <span v-if="row.badge" class="compact-badge" :title="row.badge">{{ row.badge }}</span>
                <button
                  class="compact-refresh"
                  :disabled="row.loading"
                  :title="t(locale, 'subscription.refresh')"
                  :aria-label="t(locale, 'subscription.refresh')"
                  :aria-busy="row.loading"
                  @click="row.refresh"
                >
                  <RefreshCw :size="13" :class="{ 'animate-spin': row.loading }" aria-hidden="true" />
                </button>
              </div>
              <span class="quota-column-label">{{ t(locale, row.rows[0]?.metricLabelKey ?? 'survival.remaining') }}</span>
              <span class="quota-column-label">{{ t(locale, 'survival.resetTime') }}</span>
            </div>
            <div class="quota-strip-tier-list">
              <div
                v-for="tierRow in row.rows"
                :key="`${row.key}:${tierRow.key}`"
                class="quota-strip-tier-row"
                :class="{ 'tone-violet': tierRow.key.startsWith('seven_day'), 'quota-tier-wide-metric': tierRow.metric.length > 6 }"
              >
                <div class="quota-window-progress">
                  <span class="quota-window-label" :title="tierRow.sublabel">{{ tierRow.sublabel }}</span>
                  <div
                    v-if="tierRow.barPercent != null"
                    class="compact-progress"
                    role="progressbar"
                    :aria-label="`${row.label} · ${tierRow.sublabel} · ${t(locale, tierRow.metricLabelKey)}`"
                    :aria-valuenow="tierRow.barPercent"
                    :aria-valuemin="0"
                    :aria-valuemax="100"
                  >
                    <div class="compact-progress-fill" :style="{ width: `${tierRow.barPercent}%` }" />
                  </div>
                </div>
                <dl class="quota-stat quota-stat-emphasis" :title="tierRow.detailText || t(locale, tierRow.metricLabelKey)">
                  <dt class="sr-only">{{ t(locale, tierRow.metricLabelKey) }}</dt>
                  <dd>{{ tierRow.metric }}</dd>
                  <dd v-if="tierRow.detailText" class="sr-only quota-stat-detail">{{ tierRow.detailText }}</dd>
                </dl>
                <dl v-if="tierRow.resetText" class="quota-stat" :title="t(locale, 'survival.resetTime')">
                  <dt class="sr-only">{{ t(locale, 'survival.resetTime') }}</dt>
                  <dd>{{ tierRow.resetText }}</dd>
                </dl>
              </div>
            </div>
          </div>

          <div
            v-for="row in sourceRows"
            :key="row.key"
            :class="['quota-strip', 'quota-strip-source', row.toneClass, { 'quota-strip-balance': row.isBalance }]"
          >
            <div class="quota-strip-head">
              <div class="quota-row-title quota-row-title-official">
                <span v-if="row.icon" class="quota-provider-icon">
                  <LobeIcon :slug="row.icon" :size="16" :class="{ 'dark:invert': ['opencode', 'cursor', 'githubcopilot', 'hermesagent'].includes(row.icon) }" />
                </span>
                <span v-else class="compact-dot quota-row-dot" aria-hidden="true" />
                <span class="compact-title" :title="row.label">{{ row.label }}</span>
              </div>
              <button
                class="compact-refresh"
                :disabled="row.loading"
                :title="t(locale, 'subscription.refresh')"
                :aria-label="t(locale, 'subscription.refresh')"
                :aria-busy="row.loading"
                @click="row.refresh"
              >
                <RefreshCw :size="15" :class="{ 'animate-spin': row.loading }" aria-hidden="true" />
              </button>
            </div>
            <p v-if="row.caption" class="quota-source-caption" :title="row.caption">{{ row.caption }}</p>
            <div class="quota-strip-tier-row" :class="{ 'quota-tier-wide-metric': !row.isBalance && row.metric.length > 6 }">
              <div v-if="!row.isBalance" class="quota-window-progress">
                <span class="quota-window-label" :title="row.windowLabel">{{ row.windowLabel }}</span>
                <div
                  v-if="row.barPercent != null"
                  class="compact-progress"
                  role="progressbar"
                  :aria-label="`${row.label} · ${t(locale, 'survival.remaining')}`"
                  :aria-valuenow="row.barPercent"
                  :aria-valuemin="0"
                  :aria-valuemax="100"
                >
                  <div class="compact-progress-fill" :style="{ width: `${row.barPercent}%` }" />
                </div>
              </div>
              <dl class="quota-stat quota-stat-emphasis" :title="t(locale, row.metricLabelKey)">
                <dt class="sr-only">{{ t(locale, row.metricLabelKey) }}</dt>
                <dd>{{ row.metric }}</dd>
              </dl>
              <dl v-if="row.footer && !row.isBalance" class="quota-stat" :title="t(locale, 'survival.resetTime')">
                <dt class="sr-only">{{ t(locale, 'survival.resetTime') }}</dt>
                <dd>{{ row.footer }}</dd>
              </dl>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.metric-card {
  --quota-warm-accent: color-mix(in srgb, var(--theme-chart-series-1) 55%, var(--theme-chart-cost));
  min-width: 0;
  overflow: hidden;
  border-radius: 1rem;
  border: 1px solid var(--theme-border-default);
  background: var(--theme-surface-gradient);
  box-shadow: var(--theme-shadow-inline);
}

.metric-rail {
  position: relative;
  color: var(--quota-warm-accent);
  display: flex;
  width: 2rem;
  flex-shrink: 0;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.375rem;
  padding: 0.375rem 0.25rem;
}

.metric-rail::after {
  position: absolute;
  inset: 0.75rem 0 0.75rem auto;
  width: 1px;
  content: '';
  background: linear-gradient(to bottom, transparent, var(--theme-border-strong) 12%, var(--theme-border-strong) 88%, transparent);
}

.metric-rail-title {
  font-size: 12px;
  font-weight: 700;
  white-space: nowrap;
}

.writing-vertical {
  writing-mode: vertical-rl;
  text-orientation: upright;
}

.metric-body {
  min-width: 0;
  flex: 1 1 0%;
  padding: 0.25rem;
}

.limit-stack {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.quota-strip {
  --compact-accent: var(--theme-chart-tokens);
  --compact-accent-text: var(--theme-status-info-fg);
  min-width: 0;
  border-radius: 0.875rem;
  border: 1px solid color-mix(in srgb, var(--compact-accent) 18%, var(--theme-border-default));
  background: linear-gradient(115deg, var(--theme-bg-surface), color-mix(in srgb, var(--compact-accent) 7%, var(--theme-bg-surface)));
  box-shadow: inset 0 1px 0 color-mix(in srgb, var(--theme-text-inverse) 18%, transparent);
  padding: 0.1875rem 0.5rem;
}

.quota-strip-local {
  border-color: color-mix(in srgb, var(--quota-warm-accent) 20%, var(--theme-border-default));
  background: linear-gradient(115deg, color-mix(in srgb, var(--quota-warm-accent) 3%, var(--theme-bg-surface)), color-mix(in srgb, var(--theme-chart-series-3) 5%, var(--theme-bg-surface)));
}

.quota-local-dot {
  background: var(--quota-warm-accent);
}

.quota-strip-head,
.quota-row-title {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 0.375rem;
}

.quota-strip-head {
  min-height: 1.25rem;
  justify-content: space-between;
}

.quota-row-title-official {
  flex: 1 1 auto;
  flex-wrap: nowrap;
}

.compact-title {
  min-width: 0;
  font-size: 13px;
  font-weight: 700;
  line-height: 1.25;
  color: var(--theme-text-primary);
  overflow-wrap: anywhere;
}

.quota-title-text {
  flex: 1 1 auto;
}

.compact-dot {
  width: 0.5rem;
  height: 0.5rem;
  border-radius: 999px;
  flex-shrink: 0;
}

.quota-row-dot {
  background: var(--compact-accent);
}

.quota-provider-icon {
  display: grid;
  width: 1.25rem;
  height: 1.25rem;
  flex-shrink: 0;
  place-items: center;
  border-radius: 0.5rem;
  background: var(--theme-bg-elevated);
}

.compact-badge {
  flex-shrink: 0;
  border-radius: 999px;
  background: color-mix(in srgb, var(--compact-accent) 12%, transparent);
  padding: 0.125rem 0.375rem;
  font-size: 9px;
  line-height: 1.4;
  font-weight: 600;
  color: var(--compact-accent-text);
}

.compact-badge-neutral {
  background: color-mix(in srgb, var(--theme-text-primary) 6%, transparent);
  color: var(--theme-text-secondary);
}

.compact-progress {
  margin-top: 0;
  height: 0.375rem;
  width: 100%;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--compact-accent) 15%, var(--theme-bg-surface-muted));
}

.compact-progress-fill {
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, var(--compact-accent), color-mix(in srgb, var(--compact-accent) 75%, var(--theme-chart-series-5)));
}

.quota-local-countdown {
  flex-shrink: 0;
  font-size: 14px;
  line-height: 1.2;
  font-weight: 750;
  font-variant-numeric: tabular-nums;
  color: var(--theme-text-primary);
}

.quota-local-overview {
  margin-top: 0.25rem;
}

.quota-local-overview > .compact-progress {
  height: 0.375rem;
}

.quota-local-stats,
.quota-local-stats > div:not(.sr-only) {
  display: flex;
  align-items: baseline;
  gap: 0.25rem;
}

.quota-local-stats {
  justify-content: space-between;
  gap: 0.375rem;
  margin-top: 0.25rem;
}

.quota-local-stats > div:nth-child(2) {
  border-left: 1px solid var(--theme-border-default);
  padding-left: 0.5rem;
}

.quota-local-stats dt {
  font-size: 10px;
  color: var(--theme-text-secondary);
}

.quota-local-stats dd,
.quota-stat dd {
  font-size: 11px;
  font-weight: 650;
  line-height: 1.4;
  font-variant-numeric: tabular-nums;
  color: var(--theme-text-primary);
  white-space: nowrap;
}

.quota-local-stats dd {
  font-size: 12px;
  line-height: 1.25;
}

.quota-row-title-official .compact-badge {
  max-width: 35%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.quota-local-context {
  display: flex;
  flex-wrap: wrap;
  gap: 0.125rem 0.5rem;
  margin-top: 0.1875rem;
  font-size: 10px;
  line-height: 1.4;
  color: var(--theme-text-secondary);
}

.quota-local-idle {
  margin-top: 0.375rem;
  font-size: 11px;
  color: var(--theme-text-secondary);
}

.quota-strip-tier-list {
  margin-top: 0.125rem;
}

.quota-strip-tier-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 3.5rem 3.5rem;
  align-items: center;
  gap: 0.25rem;
  padding: 0.125rem 0;
}

.quota-strip-tier-row + .quota-strip-tier-row {
  border-top: 1px solid color-mix(in srgb, var(--compact-accent) 12%, var(--theme-border-default));
}

.quota-strip-tier-row:last-child {
  padding-bottom: 0.125rem;
}

.quota-window-progress,
.quota-strip-codex .quota-window-label {
  flex: 0 0 3rem;
}

.quota-stat {
  min-width: 0;
}

.quota-window-progress {
  display: flex;
  align-items: center;
  gap: 0.375rem;
}

.quota-window-progress > .compact-progress {
  min-width: 1rem;
  flex: 1 1 0%;
}

.quota-window-label {
  display: block;
  min-width: 0;
  max-width: 55%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.4;
  color: var(--theme-text-primary);
  overflow-wrap: anywhere;
}

.quota-stat {
  border-left: 1px solid var(--theme-border-default);
  padding-left: 0.375rem;
  text-align: center;
}

.quota-stat-emphasis dd {
  color: var(--compact-accent-text);
  font-size: 13px;
  line-height: 1.2;
  font-weight: 700;
}

.quota-tier-wide-metric {
  grid-template-columns: minmax(0, 1fr) minmax(3.5rem, max-content) 3.5rem;
}

.quota-tier-wide-metric .quota-stat-emphasis dd {
  font-size: 11px;
}

.quota-strip-head-official {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 3.5rem 3.5rem;
  gap: 0.25rem;
}

.quota-strip-head-official .compact-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 13px;
  line-height: 1.25;
}

.quota-column-label {
  font-size: 9px;
  line-height: 1.25;
  text-align: center;
  color: var(--theme-text-secondary);
}

.quota-source-caption {
  margin-top: 0.1875rem;
  font-size: 11px;
  line-height: 1.4;
  color: var(--theme-text-secondary);
  overflow-wrap: anywhere;
}

.quota-strip-balance {
  border-radius: 0.5rem;
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 0.8fr) auto 1.25rem;
  align-items: center;
  gap: 0.375rem;
}

.quota-strip-balance .quota-strip-head {
  display: contents;
}

.quota-strip-balance .quota-row-title {
  grid-column: 1;
  grid-row: 1;
}

.quota-strip-balance .compact-title,
.quota-strip-balance .quota-source-caption {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.quota-strip-balance .quota-source-caption {
  grid-column: 2;
  grid-row: 1;
  margin-top: 0;
}

.quota-strip-balance .quota-strip-tier-row {
  display: block;
  grid-column: 3;
  grid-row: 1;
  padding: 0;
}

.quota-strip-balance .quota-stat {
  border: 0;
  padding-left: 0;
  text-align: right;
}

.quota-strip-balance .quota-provider-icon {
  border-radius: 0.25rem;
}

.quota-strip-balance .compact-refresh {
  grid-column: 4;
  grid-row: 1;
}

.compact-refresh {
  display: inline-flex;
  width: 1.25rem;
  height: 1.25rem;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  border-radius: 0.5rem;
  color: var(--theme-text-secondary);
  cursor: pointer;
  transition: background-color 150ms ease;
}

.compact-refresh:hover:not(:disabled) {
  background: var(--theme-bg-hover);
}

.compact-refresh:focus-visible {
  outline: 2px solid var(--theme-accent-primary);
  outline-offset: 2px;
}

.compact-refresh:disabled {
  cursor: default;
  opacity: 0.5;
}

.tone-violet {
  --compact-accent: color-mix(in srgb, var(--theme-chart-series-6) 70%, var(--theme-chart-series-3));
  --compact-accent-text: color-mix(in srgb, var(--compact-accent) 65%, var(--theme-text-primary));
}

.tone-amber {
  --compact-accent: var(--theme-chart-cost);
  --compact-accent-text: var(--theme-status-warning-fg);
}

.tone-cyan,
.tone-emerald {
  --compact-accent: var(--theme-chart-requests);
  --compact-accent-text: var(--theme-status-success-fg);
}

@media (prefers-reduced-motion: reduce) {
  .compact-refresh {
    transition: none;
  }

  .compact-refresh .animate-spin {
    animation: none;
  }
}
</style>
