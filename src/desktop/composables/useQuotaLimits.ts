/**
 * 概览页"额度与生存"卡片（设计 6.6）纯计算逻辑。
 * 输出限流行列表 + 加载态（行内文案均为 i18n key，由组件层渲染）。数据源为 monitor store 的
 * limitSurvival / claudeQuota / subscriptionQuota / copilotQuota / geminiQuota / configuredSourceQuotas。
 */
import { computed } from 'vue'
import { useMonitorStore } from '../../stores/monitor'
import type { QuotaTier, SubscriptionQuota, SurvivalConfidence } from '../../types'

export type LimitState = 'safe' | 'attention' | 'danger' | 'unknown'

export interface LimitRow {
  key: string
  /** 可翻译标签的 i18n key（与 labelText 二选一）。 */
  labelKey: string | null
  /** 不可翻译的原始展示文本（产品名 / 动态账号名等，与 labelKey 二选一）。 */
  labelText: string | null
  windowLabelKey: string | null
  windowLabelText: string | null
  usedPct: number | null
  barPct: number
  /** 距离重置的原始秒数（null 表示无重置信息，由组件层格式化）。 */
  resetSeconds: number | null
  conclusionKey: string
  state: LimitState
  confidenceKey: string | null
  clickable: boolean
}

/** 使用百分比：优先 (max - remaining) / max，其次 utilization。 */
export function usedPercentOf(tier: QuotaTier | undefined): number | null {
  if (!tier) return null
  if (tier.maxValue != null && tier.remainingValue != null && tier.maxValue > 0) {
    return ((tier.maxValue - tier.remainingValue) / tier.maxValue) * 100
  }
  if (Number.isFinite(tier.utilization)) return tier.utilization
  return null
}

export function useQuotaLimits() {
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

  function stateForUsed(pct: number | null): LimitState {
    if (pct == null) return 'unknown'
    if (pct >= 90) return 'danger'
    if (pct >= 70) return 'attention'
    return 'safe'
  }

  function tierLabelParts(name: string): { key: string | null; text: string | null } {
    if (name === 'five_hour') return { key: 'subscription.fiveHour', text: null }
    if (name === 'seven_day') return { key: 'subscription.sevenDay', text: null }
    return { key: null, text: name || null }
  }

  function resetSecondsOf(resetsAt?: string): number | null {
    if (!resetsAt) return null
    const diffMs = new Date(resetsAt).getTime() - Date.now()
    return Math.floor(diffMs / 1000)
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
      labelKey: 'desktop.overview.limitLocalWindow',
      labelText: null,
      windowLabelKey: 'settings.window5h',
      windowLabelText: null,
      usedPct: elapsedPct,
      barPct: elapsedPct ?? 0,
      resetSeconds: block ? Math.max(0, block.remainingSeconds) : null,
      conclusionKey,
      state,
      confidenceKey: confidenceKeyOf(burn?.confidence),
      clickable: false
    }
  })

  /** 官方配额来源行（Claude / Codex / Copilot / Gemini / 中转来源）。 */
  function quotaRow(
    key: string,
    labelParts: { key: string | null; text: string | null },
    q: SubscriptionQuota
  ): LimitRow | null {
    const windowTier = primaryTierOf(q)
    const balanceTier = balanceTierOf(q)
    if (!windowTier && !balanceTier) return null
    const pct = balanceTier ? null : usedPercentOf(windowTier)
    let state: LimitState
    if (balanceTier) {
      state = (balanceTier.remainingValue ?? 0) <= 0 ? 'danger' : 'safe'
    } else {
      state = windowTier?.limitReached ? 'danger' : stateForUsed(pct)
    }
    let windowLabelKey: string | null = null
    let windowLabelText: string | null = null
    if (balanceTier) {
      windowLabelText = balanceTier.currency ?? ''
    } else {
      const wp = tierLabelParts(windowTier?.name ?? '')
      windowLabelKey = wp.key
      windowLabelText = wp.text
    }
    return {
      key,
      labelKey: labelParts.key,
      labelText: labelParts.text,
      windowLabelKey,
      windowLabelText,
      usedPct: pct,
      barPct: pct ?? (balanceTier ? 100 : 0),
      resetSeconds: resetSecondsOf(windowTier?.resetsAt),
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

  function sourceToolLabelParts(q: SubscriptionQuota): { key: string | null; text: string | null } {
    const key = q.sourceTool ? TOOL_LABEL_KEYS[q.sourceTool] : undefined
    if (key) return { key, text: null }
    if (q.sourceTool) return { key: null, text: q.sourceTool }
    if (q.provider === 'source-config') {
      if (q.accountLabel) return { key: null, text: q.accountLabel }
      if (q.credentialMessage) return { key: null, text: q.credentialMessage }
      return { key: 'survival.sourceSection', text: null }
    }
    return { key: null, text: q.tool }
  }

  const limitRows = computed<LimitRow[]>(() => {
    const rows: LimitRow[] = [localRow.value]
    if (claudeQuota.value) {
      const row = quotaRow('claude', { key: null, text: 'Claude' }, claudeQuota.value)
      if (row) rows.push(row)
    }
    if (codexQuota.value) {
      const row = quotaRow('codex', { key: 'subscription.codex', text: null }, codexQuota.value)
      if (row) rows.push(row)
    }
    if (copilotQuota.value) {
      const row = quotaRow('copilot', { key: 'copilot.label', text: null }, copilotQuota.value)
      if (row) rows.push(row)
    }
    if (geminiQuota.value) {
      const row = quotaRow('gemini', { key: 'subscription.gemini', text: null }, geminiQuota.value)
      if (row) rows.push(row)
    }
    for (const q of configuredQuotas.value) {
      const row = quotaRow(`source:${q.provider}:${q.tool}`, sourceToolLabelParts(q), q)
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
