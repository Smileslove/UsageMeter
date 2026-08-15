/**
 * 概览页"额度与生存"卡片（设计 6.6）纯计算逻辑。
 * 输入：locale；输出限流行列表 + 加载态。数据源为 monitor store 的
 * limitSurvival / claudeQuota / subscriptionQuota / copilotQuota / geminiQuota / configuredSourceQuotas。
 */
import { computed } from 'vue'
import type { ComputedRef } from 'vue'
import { useMonitorStore } from '../../stores/monitor'
import { formatCountdownSeconds } from '../stores/desktopAnalytics'
import { t } from '../../i18n'
import type { QuotaTier, SubscriptionQuota, SurvivalConfidence } from '../../types'

export type LimitState = 'safe' | 'attention' | 'danger' | 'unknown'

export interface LimitRow {
  key: string
  label: string
  windowLabel: string
  usedPct: number | null
  barPct: number
  resetText: string
  conclusionKey: string
  state: LimitState
  confidenceKey: string | null
  clickable: boolean
}

export function useQuotaLimits(locale: ComputedRef<string | undefined>) {
  const store = useMonitorStore()

  const BLOCK_SECONDS = 5 * 3600

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
  const configuredQuotas = computed(() => store.configuredSourceQuotas.filter(q => (q.tiers?.length ?? 0) > 0))

  function primaryTierOf(q: SubscriptionQuota): QuotaTier | undefined {
    return q.tiers.find(tt => tt.name === 'five_hour' && tt.kind !== 'balance') ?? q.tiers.find(tt => tt.kind !== 'balance')
  }

  function balanceTierOf(q: SubscriptionQuota): QuotaTier | undefined {
    return q.tiers.find(tt => tt.kind === 'balance')
  }

  /** 使用百分比：优先 (max - remaining) / max，其次 utilization。 */
  function usedPercentOf(tier: QuotaTier | undefined): number | null {
    if (!tier) return null
    if (tier.maxValue != null && tier.remainingValue != null && tier.maxValue > 0) {
      return ((tier.maxValue - tier.remainingValue) / tier.maxValue) * 100
    }
    if (tier.utilization > 0) return tier.utilization
    return null
  }

  function stateForUsed(pct: number | null): LimitState {
    if (pct == null) return 'unknown'
    if (pct >= 90) return 'danger'
    if (pct >= 70) return 'attention'
    return 'safe'
  }

  function tierLabel(name: string): string {
    if (name === 'five_hour') return t(locale.value, 'subscription.fiveHour')
    if (name === 'seven_day') return t(locale.value, 'subscription.sevenDay')
    return name
  }

  function resetCountdown(resetsAt?: string): string {
    if (!resetsAt) return '--'
    const diffMs = new Date(resetsAt).getTime() - Date.now()
    return formatCountdownSeconds(
      Math.floor(diffMs / 1000),
      t(locale.value, 'subscription.unitDayShort'),
      t(locale.value, 'subscription.unitHourShort'),
      t(locale.value, 'subscription.unitMinuteShort')
    )
  }

  function confidenceKeyOf(confidence: SurvivalConfidence | undefined): string | null {
    if (confidence === 'high') return 'desktop.overview.limitConfidenceHigh'
    if (confidence === 'medium') return 'desktop.overview.limitConfidenceMedium'
    if (confidence === 'low') return 'desktop.overview.limitConfidenceLow'
    return null
  }

  function conclusionKeyFor(state: LimitState): string {
    if (state === 'safe') return 'desktop.overview.limitStatusSafe'
    if (state === 'attention') return 'desktop.overview.limitStatusAttention'
    if (state === 'danger') return 'desktop.overview.limitStatusDanger'
    return 'desktop.overview.limitStatusUnknown'
  }

  /** 本地会话锚定 5h 块行。 */
  const localRow = computed<LimitRow>(() => {
    const block = store.limitSurvival?.block ?? null
    const burn = store.limitSurvival?.burn ?? null
    const elapsedPct = block
      ? Math.min(100, Math.max(0, ((BLOCK_SECONDS - Math.max(0, block.remainingSeconds)) / BLOCK_SECONDS) * 100))
      : null
    let state: LimitState = 'unknown'
    let conclusionKey = 'desktop.overview.limitStatusUnknown'
    if (block) {
      if (burn && burn.confidence !== 'low') {
        state = 'safe'
        conclusionKey = 'desktop.overview.limitStatusSafe'
      } else {
        // 有窗口活动但燃烧速率不可信（低置信度/缺基线）
        state = 'attention'
        conclusionKey = 'desktop.overview.limitStatusAttentionLow'
      }
    }
    return {
      key: 'local-5h',
      label: t(locale.value, 'desktop.overview.limitLocalWindow'),
      windowLabel: t(locale.value, 'settings.window5h'),
      usedPct: elapsedPct,
      barPct: elapsedPct ?? 0,
      resetText: block
        ? formatCountdownSeconds(
            Math.max(0, block.remainingSeconds),
            t(locale.value, 'subscription.unitDayShort'),
            t(locale.value, 'subscription.unitHourShort'),
            t(locale.value, 'subscription.unitMinuteShort')
          )
        : '--',
      conclusionKey,
      state,
      confidenceKey: confidenceKeyOf(burn?.confidence),
      clickable: false
    }
  })

  /** 官方配额来源行（Claude / Codex / Copilot / Gemini / 中转来源）。 */
  function quotaRow(key: string, label: string, q: SubscriptionQuota): LimitRow | null {
    const windowTier = primaryTierOf(q)
    const balanceTier = balanceTierOf(q)
    if (!windowTier && !balanceTier) return null
    const pct = balanceTier ? null : usedPercentOf(windowTier)
    let state: LimitState
    if (balanceTier) {
      state = (balanceTier.remainingValue ?? 0) <= 0 ? 'danger' : 'safe'
    } else {
      state = stateForUsed(pct)
    }
    return {
      key,
      label,
      windowLabel: balanceTier ? (balanceTier.currency ?? '') : tierLabel(windowTier?.name ?? ''),
      usedPct: pct,
      barPct: pct ?? (balanceTier ? 100 : 0),
      resetText: resetCountdown(windowTier?.resetsAt),
      conclusionKey:
        state === 'danger' && balanceTier ? 'desktop.overview.limitStatusExhausted' : conclusionKeyFor(state),
      state,
      confidenceKey: null,
      clickable: true
    }
  }

  const TOOL_LABEL_KEYS: Record<string, string> = {
    'claude-code': 'survival.tool.claudeCode',
    codex: 'survival.tool.codex',
    hermes: 'survival.tool.hermes',
    opencode: 'survival.tool.opencode'
  }

  function sourceToolLabel(q: SubscriptionQuota): string {
    const key = q.sourceTool ? TOOL_LABEL_KEYS[q.sourceTool] : undefined
    if (key) return t(locale.value, key)
    if (q.sourceTool) return q.sourceTool
    if (q.provider === 'source-config') {
      return q.accountLabel || q.credentialMessage || t(locale.value, 'survival.sourceSection')
    }
    return q.tool
  }

  const limitRows = computed<LimitRow[]>(() => {
    const rows: LimitRow[] = [localRow.value]
    if (claudeQuota.value) {
      const row = quotaRow('claude', 'Claude', claudeQuota.value)
      if (row) rows.push(row)
    }
    if (codexQuota.value) {
      const row = quotaRow('codex', t(locale.value, 'subscription.codex'), codexQuota.value)
      if (row) rows.push(row)
    }
    if (copilotQuota.value) {
      const row = quotaRow('copilot', t(locale.value, 'copilot.label'), copilotQuota.value)
      if (row) rows.push(row)
    }
    if (geminiQuota.value) {
      const row = quotaRow('gemini', t(locale.value, 'subscription.gemini'), geminiQuota.value)
      if (row) rows.push(row)
    }
    for (const q of configuredQuotas.value) {
      const row = quotaRow(`source:${q.provider}:${q.tool}`, sourceToolLabel(q), q)
      if (row) rows.push(row)
    }
    return rows
  })

  const limitLoading = computed(
    () =>
      !store.limitSurvival &&
      (store.loading || store.configuredSourceLoading || store.claudeLoading || store.subscriptionLoading)
  )

  return { limitRows, limitLoading }
}
