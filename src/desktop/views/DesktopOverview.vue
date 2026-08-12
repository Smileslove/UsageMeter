<script setup lang="ts">
/**
 * 桌面主窗口 - 概览页（设计文档第 6 章）。
 * 在一个屏幕内回答：用了多少 / 额度是否有风险 / 消耗来自哪里 / 是否有异常。
 * 数据口径与快速面板一致：monitor store 的 snapshot / limitSurvival / overviewBreakdown / rateSummary。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { LineChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import VChart from 'vue-echarts'
import {
  Activity,
  ArrowUpRight,
  CheckCircle2,
  CircleAlert,
  CircleDollarSign,
  CircleHelp,
  Clock,
  Database,
  Gauge,
  Layers3,
  LayoutGrid,
  MessageSquare,
  RefreshCw,
  ShieldCheck,
  Sigma,
  TriangleAlert,
  Zap
} from 'lucide-vue-next'
import type { Component } from 'vue'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import {
  formatCountdownSeconds,
  rangeQueryForWindow,
  resolveChartTheme,
  useDesktopAnalyticsStore,
  withAlpha
} from '../stores/desktopAnalytics'
import { t } from '../../i18n'
import {
  formatCost,
  formatDurationMs,
  formatRate,
  formatRequestCount,
  formatTokenPair,
  formatTokenValue
} from '../../utils/format'
import { formatToolDisplayName } from '../../utils/toolDisplay'
import type {
  OverviewBreakdownItem,
  QuotaTier,
  StatisticsMetric,
  StatisticsTrendPoint,
  SubscriptionQuota,
  SurvivalConfidence,
  WindowUsage
} from '../../types'

use([CanvasRenderer, LineChart, GridComponent, TooltipComponent])

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

// ============ 数据口径 ============

/** 当前概览窗口对应的窗口数据（缺失时回退到第一个可用窗口）。 */
const windowData = computed<WindowUsage | null>(() => {
  const windows = store.snapshot?.windows ?? []
  return windows.find(w => w.window === analytics.overviewWindow) ?? windows[0] ?? null
})

const rateSummary = computed(() => store.rateSummary)
/** 必须有实际请求才算有速率数据（避免错误时返回的空统计显示 0.00）。 */
const hasRateData = computed(() => {
  const rs = rateSummary.value
  return !!rs && rs.overall.requestCount > 0
})

/** 有状态码覆盖的请求数 = 成功 + 客户端错误 + 服务端错误。 */
const coveredStatusRequests = computed(() => {
  const d = windowData.value
  if (!d) return 0
  return d.successRequests + d.clientErrorRequests + d.serverErrorRequests
})

/** 成功率：成功 / 有状态码覆盖的请求（设计 6.4：明确"仅代理覆盖"标签）。 */
const successRate = computed(() => {
  const covered = coveredStatusRequests.value
  if (covered <= 0) return null
  return (windowData.value!.successRequests / covered) * 100
})

/** 缓存命中率：cacheRead / (cacheRead + cacheCreate)。 */
const cacheHitRate = computed(() => {
  const d = windowData.value
  if (!d) return null
  const total = d.cacheReadTokens + d.cacheCreateTokens
  if (total <= 0) return null
  return (d.cacheReadTokens / total) * 100
})

// ============ KPI 条带（设计 6.4） ============

interface KpiItem {
  key: string
  icon: Component
  labelKey: string
  primary: string
  secondary: string
  secondaryTitle?: string
  /** 点击后趋势切换到的主指标维度。 */
  metric: StatisticsMetric
  /** "仅代理覆盖"徽标。 */
  coverageTag: boolean
}

const kpis = computed<KpiItem[]>(() => {
  const d = windowData.value
  if (!d) return []
  const pair = formatTokenPair(d.inputTokens, d.outputTokens)
  const covered = coveredStatusRequests.value
  const speed = hasRateData.value ? rateSummary.value!.overall.avgTokensPerSecond : null
  const ttft = hasRateData.value ? rateSummary.value!.ttft.avgTtftMs : null
  const statusSecondary =
    covered > 0
      ? `${t(locale.value, 'desktop.overview.kpiSuccess')} ${formatRequestCount(d.successRequests)} · ${t(locale.value, 'desktop.overview.kpiFailed')} ${formatRequestCount(d.clientErrorRequests + d.serverErrorRequests)}`
      : '--'
  return [
    {
      key: 'requests',
      icon: MessageSquare,
      labelKey: 'desktop.overview.kpiRequests',
      primary: formatRequestCount(d.requestUsed),
      secondary: statusSecondary,
      metric: 'requests',
      coverageTag: false
    },
    {
      key: 'tokens',
      icon: Sigma,
      labelKey: 'desktop.overview.kpiTokens',
      primary: formatTokenValue(d.tokenUsed),
      secondary: `${t(locale.value, 'desktop.overview.kpiInput')} ${pair.input} · ${t(locale.value, 'desktop.overview.kpiOutput')} ${pair.output}`,
      metric: 'tokens',
      coverageTag: false
    },
    {
      key: 'cost',
      icon: CircleDollarSign,
      labelKey: 'desktop.overview.kpiCost',
      primary: formatCost(d.cost, store.settings.currency),
      secondary: `USD ${d.cost.toFixed(4)}`,
      secondaryTitle: t(locale.value, 'desktop.overview.kpiUsdHint'),
      metric: 'cost',
      coverageTag: false
    },
    {
      key: 'cache',
      icon: Database,
      labelKey: 'desktop.overview.kpiCache',
      primary: formatTokenValue(d.cacheReadTokens),
      secondary:
        cacheHitRate.value == null
          ? '--'
          : `${formatRate(cacheHitRate.value)}% ${t(locale.value, 'desktop.overview.kpiHitRate')}`,
      metric: 'tokens',
      coverageTag: false
    },
    {
      key: 'successRate',
      icon: ShieldCheck,
      labelKey: 'desktop.overview.kpiSuccessRate',
      primary: successRate.value == null ? '--' : `${formatRate(successRate.value)}%`,
      secondary:
        successRate.value == null
          ? t(locale.value, 'desktop.overview.kpiCoverageOnly')
          : t(locale.value, 'desktop.overview.kpiCovered', { count: formatRequestCount(coveredStatusRequests.value) }),
      metric: 'requests',
      coverageTag: successRate.value == null
    },
    {
      key: 'avgSpeed',
      icon: Gauge,
      labelKey: 'desktop.overview.kpiAvgSpeed',
      primary: speed == null ? '--' : `${formatRate(speed)} t/s`,
      secondary:
        ttft == null
          ? t(locale.value, 'desktop.overview.kpiCoverageOnly')
          : `${t(locale.value, 'desktop.overview.kpiTtft')} ${formatDurationMs(ttft)}`,
      metric: 'requests',
      coverageTag: !hasRateData.value
    }
  ]
})

// ============ 用量趋势（设计 6.5） ============

/** 当前概览窗口 → 统计查询（5h/24h/today 小时粒度，7d/30d/本月 天粒度）。 */
const trendQuery = computed(() =>
  rangeQueryForWindow(
    analytics.overviewWindow,
    store.settings.timezone,
    store.settings.dayBoundaryMode === 'night_owl' ? 4 : 0
  )
)

/** 趋势数据点（statisticsSummary 为共享状态，概览页挂载/切换窗口时主动拉取自己的范围）。 */
const trendPoints = computed(() => store.statisticsSummary?.trend ?? [])

/** 除主指标外额外叠加显示的序列（点击 legend chip 叠加，最多 3 条全部）。 */
const extraMetrics = ref<StatisticsMetric[]>([])
const visibleMetrics = computed(() => new Set<StatisticsMetric>([analytics.analyticsMetric, ...extraMetrics.value]))

function selectTrendMetric(metric: StatisticsMetric) {
  if (!extraMetrics.value.includes(metric)) {
    extraMetrics.value = [...extraMetrics.value, metric]
  }
  // 再次点击不取消，只维持一个当前主指标（设计 6.4）
  analytics.analyticsMetric = metric
}

function metricValue(point: StatisticsTrendPoint, metric: StatisticsMetric): number {
  if (metric === 'cost') return point.cost
  if (metric === 'tokens') return point.totalTokens
  return point.requestCount
}

function formatMetric(metric: StatisticsMetric, value: number): string {
  if (metric === 'cost') return formatCost(value, store.settings.currency)
  if (metric === 'tokens') return formatTokenValue(value)
  return formatRequestCount(value)
}

function metricLabel(metric: StatisticsMetric): string {
  if (metric === 'cost') return t(locale.value, 'statistics.metricCost')
  if (metric === 'tokens') return t(locale.value, 'statistics.metricTokens')
  return t(locale.value, 'statistics.metricRequests')
}

/** 主题颜色：touch theme 设置触发重算（同 StatisticsTrendChart 的做法）。 */
const chartTheme = computed(() => {
  void store.settings.theme.appearance
  void store.settings.theme.lightPalette
  void store.settings.theme.darkPalette
  return resolveChartTheme()
})

function metricColor(metric: StatisticsMetric): string {
  if (metric === 'cost') return chartTheme.value.cost
  if (metric === 'tokens') return chartTheme.value.tokens
  return chartTheme.value.requests
}

const trendMetrics: StatisticsMetric[] = ['requests', 'tokens', 'cost']

const chartOptions = computed(() => {
  const colors = chartTheme.value
  return {
    grid: { left: 48, right: 12, top: 14, bottom: 22 },
    tooltip: {
      trigger: 'axis',
      backgroundColor: colors.tooltipBg,
      borderColor: colors.tooltipBorder,
      borderRadius: 8,
      padding: [7, 9],
      textStyle: { color: colors.tooltipText, fontSize: 11 },
      // 极简 tooltip：时间 + 三指标精确值 + 平均速度
      formatter: (params: any) => {
        const point = trendPoints.value[params[0].dataIndex]
        if (!point) return ''
        const rows = trendMetrics
          .map(m => {
            const value = formatMetric(m, metricValue(point, m))
            const color = metricColor(m)
            return `<div style="display:flex;align-items:center;gap:6px;margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${color};"></span><span>${metricLabel(m)}: <b>${value}</b></span></div>`
          })
          .join('')
        const rate =
          point.avgTokensPerSecond == null ? '--' : `${formatRate(point.avgTokensPerSecond)} t/s`
        return `<div style="font-weight:600;margin-bottom:4px;">${point.label}</div>${rows}<div style="display:flex;align-items:center;gap:6px;margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${colors.series3};"></span><span>${t(locale.value, 'common.avgRate')}: <b>${rate}</b></span></div>`
      }
    },
    xAxis: {
      type: 'category',
      data: trendPoints.value.map(p => p.label),
      boundaryGap: false,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: { color: colors.axis, fontSize: 10, hideOverlap: true, margin: 8 }
    },
    yAxis: {
      type: 'value',
      min: 0,
      axisLine: { show: false },
      axisTick: { show: false },
      axisLabel: {
        color: colors.axis,
        fontSize: 10,
        formatter: (value: number) => formatMetric(analytics.analyticsMetric, value)
      },
      splitLine: { lineStyle: { type: 'dashed', color: colors.grid } }
    },
    series: trendMetrics
      .filter(m => visibleMetrics.value.has(m))
      .map(m => {
        const color = metricColor(m)
        const isPrimary = m === analytics.analyticsMetric
        return {
          name: m,
          type: 'line',
          data: trendPoints.value.map(p => metricValue(p, m)),
          smooth: true,
          showSymbol: false,
          lineStyle: { width: isPrimary ? 2.5 : 1.6, color },
          itemStyle: { color },
          emphasis: { focus: 'series' },
          areaStyle: isPrimary
            ? {
                color: {
                  type: 'linear',
                  x: 0,
                  y: 0,
                  x2: 0,
                  y2: 1,
                  colorStops: [
                    { offset: 0, color: withAlpha(color, '26') },
                    { offset: 1, color: withAlpha(color, '04') }
                  ]
                }
              }
            : undefined
        }
      })
  }
})

// ============ 额度与生存（设计 6.6） ============

type LimitState = 'safe' | 'attention' | 'danger' | 'unknown'

interface LimitRow {
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

const BLOCK_SECONDS = 5 * 3600

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

function stateToneClass(state: LimitState): string {
  if (state === 'safe') return 'text-emerald-600 dark:text-emerald-400'
  if (state === 'attention') return 'text-amber-600 dark:text-amber-400'
  if (state === 'danger') return 'text-red-600 dark:text-red-400'
  return 'text-[var(--theme-text-tertiary)]'
}

function barClass(state: LimitState): string {
  if (state === 'safe') return 'bg-emerald-500 dark:bg-emerald-400'
  if (state === 'attention') return 'bg-amber-500 dark:bg-amber-400'
  if (state === 'danger') return 'bg-red-500 dark:bg-red-400'
  return 'bg-[var(--theme-text-quaternary)]'
}

function stateIcon(state: LimitState): Component {
  if (state === 'safe') return CheckCircle2
  if (state === 'attention') return TriangleAlert
  if (state === 'danger') return CircleAlert
  return CircleHelp
}

// ============ 三维贡献（设计 6.7） ============

const breakdown = computed(() => store.overviewBreakdown)

function primaryValue(item: OverviewBreakdownItem): number {
  if (analytics.analyticsMetric === 'cost') return item.cost
  if (analytics.analyticsMetric === 'tokens') return item.totalTokens
  return item.requestCount
}

function displayLabel(item: OverviewBreakdownItem): string {
  if (item.kind === 'tool') {
    return formatToolDisplayName(item.id, locale.value, store.settings.clientTools.profiles)
  }
  if (item.label === '__unknown__') return t(locale.value, 'sources.unknown')
  if (item.label === '__official_api__') return t(locale.value, 'sources.officialAnthropic')
  return item.label
}

interface ContributionRow {
  item: OverviewBreakdownItem
  percent: number
}

/** 前 5 + "其他"（其余汇总为一行），占比按当前主指标计算。 */
function topRows(items: OverviewBreakdownItem[]): ContributionRow[] {
  const sorted = [...items].sort((a, b) => primaryValue(b) - primaryValue(a))
  const top = sorted.slice(0, 5)
  const rest = sorted.slice(5)
  const total = Math.max(sorted.reduce((sum, i) => sum + primaryValue(i), 0), 1)
  const rows: ContributionRow[] = top.map(item => ({ item, percent: (primaryValue(item) / total) * 100 }))
  if (rest.length > 0) {
    const restValue = rest.reduce((sum, i) => sum + primaryValue(i), 0)
    const other: OverviewBreakdownItem = {
      id: '__other__',
      label: t(locale.value, 'desktop.overview.contributionOther'),
      kind: 'model',
      color: null,
      icon: null,
      requestCount: rest.reduce((sum, i) => sum + i.requestCount, 0),
      totalTokens: rest.reduce((sum, i) => sum + i.totalTokens, 0),
      inputTokens: 0,
      outputTokens: 0,
      cacheCreateTokens: 0,
      cacheReadTokens: 0,
      cost: restValue,
      percent: 0
    }
    rows.push({ item: other, percent: (restValue / total) * 100 })
  }
  return rows
}

const contributionSections = computed(() => {
  const b = breakdown.value
  if (!b) return []
  return [
    { key: 'source' as const, titleKey: 'desktop.overview.contributionSources', rows: topRows(b.sourceRanking) },
    { key: 'tool' as const, titleKey: 'desktop.overview.contributionTools', rows: topRows(b.toolRanking) },
    { key: 'model' as const, titleKey: 'desktop.overview.contributionModels', rows: topRows(b.modelRanking) }
  ]
})

function sectionIcon(key: string): Component {
  if (key === 'source') return Layers3
  if (key === 'tool') return LayoutGrid
  return Zap
}

function formatContributionPrimary(item: OverviewBreakdownItem): string {
  if (analytics.analyticsMetric === 'cost') return formatCost(item.cost, store.settings.currency)
  if (analytics.analyticsMetric === 'tokens') return formatTokenValue(item.totalTokens)
  return formatRequestCount(item.requestCount)
}

/** 点击行：设置过滤并进入分析页（设计 6.7），经 desktopNavigation 深链通道传递筛选上下文。 */
function onContributionClick(sectionKey: 'source' | 'tool' | 'model', item: OverviewBreakdownItem) {
  if (item.id === '__other__') {
    nav.navigate('analytics')
    return
  }
  if (sectionKey === 'source') {
    nav.applyNavigationTarget({ page: 'analytics', sourceId: item.id })
  } else if (sectionKey === 'tool') {
    nav.applyNavigationTarget({ page: 'analytics', tool: item.id })
  } else {
    nav.navigate('analytics')
  }
}

// ============ 最近活跃会话（设计 6.8） ============

const recentSessions = computed(() => store.sessions.slice(0, 5))

function sessionTitle(session: { sessionName?: string; topic?: string }): string {
  return session.sessionName || session.topic || t(locale.value, 'sessions.untitled')
}

function formatRelativeTime(epoch: number): string {
  if (!epoch) return '--'
  const diffMs = Date.now() - epoch * 1000
  if (diffMs < 60_000) return t(locale.value, 'common.justNow')
  const minutes = Math.floor(diffMs / 60_000)
  if (minutes < 60) return `${minutes}${t(locale.value, 'common.minutesAgo')}`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}${t(locale.value, 'common.hoursAgo')}`
  return `${Math.floor(hours / 24)}${t(locale.value, 'common.daysAgo')}`
}

// ============ 数据覆盖与异常（设计 6.8） ============

interface HealthItem {
  key: string
  icon: Component
  tone: 'ok' | 'warning' | 'danger'
  text: string
  actionKey?: string
  action?: () => void
}

function retry() {
  void store.refreshUsage()
  refreshWindowData()
}

const healthItems = computed<HealthItem[]>(() => {
  const items: HealthItem[] = []
  if (store.error) {
    items.push({
      key: 'scanFailed',
      icon: CircleAlert,
      tone: 'danger',
      text: t(locale.value, 'desktop.overview.healthScanFailed'),
      actionKey: 'desktop.overview.healthRetry',
      action: retry
    })
  }
  if (store.snapshot && !hasRateData.value) {
    items.push({
      key: 'proxyCoverage',
      icon: TriangleAlert,
      tone: 'warning',
      text: t(locale.value, 'desktop.overview.healthProxyCoverage'),
      actionKey: 'desktop.manageDataSources',
      action: () => nav.navigate('settings')
    })
  }
  if (store.proxyStatus && !store.proxyStatus.running) {
    items.push({
      key: 'gatewayDown',
      icon: CircleAlert,
      tone: 'danger',
      text: t(locale.value, 'desktop.overview.healthGatewayDown')
    })
  }
  if (store.configuredSourceLastFailureAt) {
    items.push({
      key: 'quotaQuery',
      icon: TriangleAlert,
      tone: 'warning',
      text: t(locale.value, 'desktop.overview.healthQuotaQueryFailed')
    })
  }
  const stale =
    !!store.lastUpdatedEpoch && Date.now() - store.lastUpdatedEpoch * 1000 > 10 * 60_000
  if (stale) {
    items.push({
      key: 'stale',
      icon: Clock,
      tone: 'warning',
      text: t(locale.value, 'desktop.overview.healthStaleData')
    })
  }
  if (items.length === 0) {
    items.push({ key: 'ok', icon: CheckCircle2, tone: 'ok', text: t(locale.value, 'desktop.overview.healthOk') })
  }
  return items
})

// ============ 状态处理（设计 6.9） ============

const isEmpty = computed(() => {
  if (!store.snapshot) return false
  const total = store.snapshot.summary.totalRequests + store.snapshot.summary.totalTokens
  return total <= 0 && store.sessions.length === 0
})

const fatalError = computed(() => !!store.error && !store.snapshot)

// ============ 数据加载 ============

/** 窗口数据 + 趋势数据（范围变化或挂载时调用）。 */
function refreshWindowData() {
  void store.fetchOverviewDeferredBundle(analytics.overviewWindow)
  void store.fetchStatisticsSummary(trendQuery.value)
}

/** 深链：消费 pendingFilters 中的 sourceId/tool 并应用到全局筛选（window/metric/view 暂无通道，见遗留说明）。 */
function applyPendingFilters() {
  const filters = nav.consumePendingFilters()
  if (!filters) return
  if (filters.sourceId && filters.sourceId !== store.settings.sourceAware.activeSourceFilter) {
    void store.setActiveSourceFilter(filters.sourceId)
  }
  if (filters.tool && filters.tool !== store.settings.clientTools.activeToolFilter) {
    void store.setActiveToolFilter(filters.tool)
  }
}

watch(
  () => analytics.overviewWindow,
  () => {
    if (store.snapshot) refreshWindowData()
  }
)

// snapshot 到达（说明 settings 已加载完成）后，继承一次快速面板 summaryWindow 并补齐窗口数据
watch(
  () => store.snapshot,
  snapshot => {
    if (snapshot) {
      analytics.inheritSummaryWindowOnce()
      if (!store.overviewBreakdown) refreshWindowData()
    }
  },
  { immediate: true }
)

onMounted(() => {
  applyPendingFilters()
  void store.fetchSessions(30, 0, false)
  if (store.snapshot) refreshWindowData()
})
</script>

<template>
  <section class="flex flex-col gap-5">
    <!-- 加载失败：页面内错误 + 重试（设计 6.9） -->
    <div
      v-if="fatalError"
      class="flex flex-col items-center gap-3 rounded-lg border border-[var(--theme-status-danger-border)] bg-[var(--theme-status-danger-bg)] px-6 py-10 text-center"
      role="alert"
    >
      <CircleAlert :size="22" class="text-[var(--theme-status-danger-fg)]" aria-hidden="true" />
      <div>
        <p class="text-sm font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.overview.errorTitle') }}</p>
        <p class="mt-1 max-w-md text-xs leading-5 text-[var(--theme-text-tertiary)]">{{ store.error }}</p>
      </div>
      <button
        type="button"
        class="flex items-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3 py-1.5 text-xs font-semibold text-[var(--theme-accent-contrast)] transition-opacity duration-150 hover:opacity-90"
        @click="retry"
      >
        <RefreshCw :size="13" aria-hidden="true" />
        {{ t(locale, 'desktop.overview.errorRetry') }}
      </button>
    </div>

    <!-- 完全空数据：中央说明 + 主操作"管理数据源"（设计 6.9） -->
    <div
      v-else-if="isEmpty"
      class="flex flex-col items-center gap-3 rounded-lg border border-dashed border-[var(--theme-border-strong)] px-6 py-16 text-center"
    >
      <Database :size="22" class="text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <div>
        <p class="text-sm font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.overview.emptyTitle') }}</p>
        <p class="mt-1 max-w-md text-xs leading-5 text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.overview.emptyDesc') }}</p>
      </div>
      <button
        type="button"
        class="rounded-lg bg-[var(--theme-accent-primary)] px-3 py-1.5 text-xs font-semibold text-[var(--theme-accent-contrast)] transition-opacity duration-150 hover:opacity-90"
        @click="nav.navigate('settings')"
      >
        {{ t(locale, 'desktop.manageDataSources') }}
      </button>
    </div>

    <template v-else>
      <!-- KPI 条带：一条连续表面 6 个指标（设计 6.4），固定高度骨架保证布局不跳动 -->
      <div
        class="overflow-hidden rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-border-default)]"
        role="group"
        :aria-label="t(locale, 'desktop.overview.kpiBand')"
      >
        <div v-if="kpis.length" class="grid grid-cols-2 gap-px md:grid-cols-3 2xl:grid-cols-6">
          <button
            v-for="kpi in kpis"
            :key="kpi.key"
            type="button"
            class="flex h-[112px] flex-col items-start justify-between bg-[var(--theme-bg-elevated)] p-4 text-left transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-ring-focus)]"
            :aria-pressed="analytics.analyticsMetric === kpi.metric"
            :title="t(locale, 'desktop.overview.kpiSwitchHint')"
            @click="analytics.analyticsMetric = kpi.metric"
          >
            <span class="flex w-full items-center justify-between gap-1.5">
              <span class="flex min-w-0 items-center gap-1.5 text-xs font-medium text-[var(--theme-text-tertiary)]">
                <component :is="kpi.icon" :size="14" class="shrink-0" aria-hidden="true" />
                <span class="truncate">{{ t(locale, kpi.labelKey) }}</span>
              </span>
              <span
                v-if="kpi.coverageTag"
                class="shrink-0 rounded-full border border-[var(--theme-status-warning-border)] bg-[var(--theme-status-warning-bg)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--theme-status-warning-fg)]"
              >
                {{ t(locale, 'desktop.overview.kpiCoverageOnly') }}
              </span>
            </span>
            <span class="w-full truncate font-mono text-[20px] font-bold leading-7 text-[var(--theme-text-primary)]">
              {{ kpi.primary }}
            </span>
            <span
              class="w-full truncate text-xs text-[var(--theme-text-tertiary)]"
              :title="kpi.secondaryTitle"
            >
              {{ kpi.secondary }}
            </span>
          </button>
        </div>
        <div v-else class="grid grid-cols-2 gap-px md:grid-cols-3 2xl:grid-cols-6">
          <div v-for="i in 6" :key="i" class="h-[112px] animate-pulse bg-[var(--theme-bg-elevated)] p-4">
            <div class="h-3 w-16 rounded bg-[var(--theme-text-primary)]/10"></div>
            <div class="mt-4 h-5 w-20 rounded bg-[var(--theme-text-primary)]/10"></div>
            <div class="mt-2 h-3 w-24 rounded bg-[var(--theme-text-primary)]/10"></div>
          </div>
        </div>
      </div>

      <!-- 用量趋势 + 额度与生存 -->
      <div class="grid grid-cols-1 gap-5 xl:grid-cols-3">
        <!-- 用量趋势（约 2/3 宽） -->
        <div class="rounded-lg border border-[var(--theme-border-default)] p-4 xl:col-span-2" style="background: var(--theme-surface-gradient)">
          <div class="mb-3 flex items-center justify-between gap-2">
            <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
              <Activity :size="14" class="shrink-0" aria-hidden="true" />
              {{ t(locale, 'desktop.overview.trendTitle') }}
            </h3>
            <div
              class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
              role="group"
              :aria-label="t(locale, 'desktop.overview.trendMetric')"
            >
              <button
                v-for="m in trendMetrics"
                :key="m"
                type="button"
                class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
                :class="
                  analytics.analyticsMetric === m
                    ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
                    : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
                "
                :aria-pressed="analytics.analyticsMetric === m"
                @click="selectTrendMetric(m)"
              >
                <span class="h-1.5 w-1.5 rounded-full" :style="{ backgroundColor: metricColor(m) }"></span>
                {{ metricLabel(m) }}
              </button>
            </div>
          </div>

          <div v-if="trendPoints.length" class="h-[240px]">
            <v-chart class="h-full w-full" :option="chartOptions" autoresize />
          </div>
          <div
            v-else-if="store.statisticsLoading"
            class="h-[240px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"
            :aria-label="t(locale, 'common.syncing')"
          ></div>
          <div
            v-else
            class="grid h-[240px] place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] text-xs text-[var(--theme-text-tertiary)]"
          >
            {{ t(locale, 'desktop.overview.trendNoData') }}
          </div>
        </div>

        <!-- 额度与生存（约 1/3 宽）：按来源纵向列表（设计 6.6） -->
        <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
          <h3 class="mb-3 flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
            <ShieldCheck :size="14" class="shrink-0" aria-hidden="true" />
            {{ t(locale, 'desktop.overview.limitTitle') }}
          </h3>

          <div v-if="limitLoading" class="flex flex-col gap-2">
            <div v-for="i in 3" :key="i" class="h-[84px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"></div>
          </div>
          <div
            v-else-if="limitRows.length === 0"
            class="grid place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center text-xs leading-5 text-[var(--theme-text-tertiary)]"
          >
            {{ t(locale, 'desktop.overview.limitEmpty') }}
          </div>
          <ul v-else class="flex flex-col gap-2">
            <li
              v-for="row in limitRows"
              :key="row.key"
              :class="[
                'rounded-lg border border-[var(--theme-border-default)] p-3',
                row.clickable ? 'cursor-pointer transition-colors duration-150 hover:bg-[var(--theme-bg-hover)]' : ''
              ]"
              :title="row.clickable ? t(locale, 'desktop.overview.limitOpenHint') : undefined"
              @click="row.clickable && nav.navigate('analytics')"
            >
              <div class="flex items-center justify-between gap-2">
                <div class="flex min-w-0 items-center gap-1.5">
                  <component :is="stateIcon(row.state)" :size="14" class="shrink-0" :class="stateToneClass(row.state)" aria-hidden="true" />
                  <span class="truncate text-xs font-semibold text-[var(--theme-text-primary)]">{{ row.label }}</span>
                  <span
                    class="shrink-0 rounded-full border border-[var(--theme-border-default)] px-1.5 py-px text-[10px] text-[var(--theme-text-tertiary)]"
                  >
                    {{ row.windowLabel }}
                  </span>
                </div>
                <span class="shrink-0 text-[11px] font-medium" :class="stateToneClass(row.state)">
                  {{ t(locale, row.conclusionKey) }}
                </span>
              </div>
              <div class="mt-2 flex items-center gap-2">
                <div class="h-1.5 flex-1 overflow-hidden rounded-full bg-[var(--theme-text-primary)]/10">
                  <div
                    class="h-full rounded-full transition-[width] duration-300"
                    :class="barClass(row.state)"
                    :style="{ width: `${row.barPct}%` }"
                  ></div>
                </div>
                <span class="w-10 shrink-0 text-right font-mono text-[11px] font-semibold text-[var(--theme-text-secondary)]">
                  {{ row.usedPct == null ? '--' : `${formatRate(row.usedPct)}%` }}
                </span>
              </div>
              <div class="mt-1.5 flex items-center justify-between gap-2 text-[10px] text-[var(--theme-text-quaternary)]">
                <span>{{ t(locale, 'desktop.overview.limitResetIn', { time: row.resetText }) }}</span>
                <span v-if="row.confidenceKey">
                  {{ t(locale, 'desktop.overview.limitConfidence') }} {{ t(locale, row.confidenceKey) }}
                </span>
              </div>
            </li>
          </ul>
        </div>
      </div>

      <!-- 三维贡献：来源 / 工具 / 模型排行（设计 6.7） -->
      <div class="grid grid-cols-1 gap-5 md:grid-cols-3">
        <section
          v-for="section in contributionSections"
          :key="section.key"
          class="rounded-lg border border-[var(--theme-border-default)] p-4"
          style="background: var(--theme-surface-gradient)"
        >
          <div class="mb-2 flex items-center justify-between gap-2">
            <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
              <component :is="sectionIcon(section.key)" :size="14" class="shrink-0" aria-hidden="true" />
              {{ t(locale, section.titleKey) }}
            </h3>
            <span class="shrink-0 text-[10px] text-[var(--theme-text-quaternary)]">{{ metricLabel(analytics.analyticsMetric) }}</span>
          </div>

          <div v-if="store.overviewBreakdownLoading && !breakdown" class="flex flex-col gap-2">
            <div v-for="i in 3" :key="i" class="h-[56px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"></div>
          </div>
          <div v-else-if="section.rows.length === 0" class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-3 py-6 text-center text-xs text-[var(--theme-text-tertiary)]">
            {{ t(locale, 'desktop.overview.contributionNoData') }}
          </div>
          <div v-else class="flex flex-col">
            <button
              v-for="row in section.rows"
              :key="row.item.id"
              type="button"
              class="flex flex-col gap-1 rounded-md px-1.5 py-2 text-left transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
              :title="t(locale, 'desktop.overview.contributionOpenHint')"
              @click="onContributionClick(section.key, row.item)"
            >
              <span class="flex items-center justify-between gap-2">
                <span class="truncate text-xs font-medium text-[var(--theme-text-primary)]">{{ displayLabel(row.item) }}</span>
                <span class="shrink-0 font-mono text-[11px] font-semibold text-[var(--theme-text-secondary)]">
                  {{ formatContributionPrimary(row.item) }}
                </span>
              </span>
              <span class="flex items-center gap-2">
                <span class="h-1 flex-1 overflow-hidden rounded-full bg-[var(--theme-text-primary)]/10">
                  <span
                    class="block h-full rounded-full transition-[width] duration-300"
                    :style="{ width: `${row.percent}%`, backgroundColor: row.item.color || 'var(--theme-accent-primary)' }"
                  ></span>
                </span>
                <span class="w-9 shrink-0 text-right text-[10px] text-[var(--theme-text-quaternary)]">{{ formatRate(row.percent) }}%</span>
              </span>
              <span class="text-[10px] text-[var(--theme-text-quaternary)]">
                {{ t(locale, 'desktop.overview.contributionRequests', { count: formatRequestCount(row.item.requestCount) }) }}
              </span>
            </button>
          </div>
        </section>
      </div>

      <!-- 最近活跃会话 + 数据覆盖与异常 -->
      <div class="grid grid-cols-1 gap-5 xl:grid-cols-3">
        <div class="rounded-lg border border-[var(--theme-border-default)] p-4 xl:col-span-2" style="background: var(--theme-surface-gradient)">
          <div class="mb-2 flex items-center justify-between gap-2">
            <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
              <Clock :size="14" class="shrink-0" aria-hidden="true" />
              {{ t(locale, 'desktop.overview.sessionsTitle') }}
            </h3>
            <button
              v-if="recentSessions.length > 5 || store.sessions.length > 5"
              type="button"
              class="flex items-center gap-1 text-xs font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:underline"
              @click="nav.navigate('sessions')"
            >
              {{ t(locale, 'desktop.overview.sessionsViewAll') }}
              <ArrowUpRight :size="13" aria-hidden="true" />
            </button>
          </div>

          <div v-if="store.sessionsLoading && recentSessions.length === 0" class="flex flex-col gap-2">
            <div v-for="i in 3" :key="i" class="h-[52px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"></div>
          </div>
          <div
            v-else-if="recentSessions.length === 0"
            class="grid place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center text-xs text-[var(--theme-text-tertiary)]"
          >
            {{ t(locale, 'desktop.overview.sessionsEmpty') }}
          </div>
          <ul v-else class="flex flex-col divide-y divide-[var(--theme-border-subtle)]">
            <li v-for="s in recentSessions" :key="s.sessionId">
              <button
                type="button"
                class="flex w-full items-center gap-3 rounded-md px-2 py-2 text-left transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
                :title="t(locale, 'desktop.overview.sessionsOpenHint')"
                @click="nav.openSession(s.sessionId)"
              >
                <span class="min-w-0 flex-1">
                  <span class="block truncate text-xs font-semibold text-[var(--theme-text-primary)]">{{ sessionTitle(s) }}</span>
                  <span class="mt-0.5 block truncate text-[11px] text-[var(--theme-text-tertiary)]">
                    {{ s.projectName || t(locale, 'common.unknownProject') }}
                    · {{ formatToolDisplayName(s.tool, locale, store.settings.clientTools.profiles) }}
                    · {{ formatRelativeTime(s.lastRequestTime) }}
                  </span>
                </span>
                <span class="flex shrink-0 items-center gap-3 font-mono text-[11px] text-[var(--theme-text-secondary)]">
                  <span>{{ formatRequestCount(s.totalRequests) }}</span>
                  <span>{{ formatCost(s.estimatedCost ?? 0, store.settings.currency) }}</span>
                  <ArrowUpRight :size="14" class="text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                </span>
              </button>
            </li>
          </ul>
        </div>

        <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
          <h3 class="mb-2 flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
            <CircleHelp :size="14" class="shrink-0" aria-hidden="true" />
            {{ t(locale, 'desktop.overview.healthTitle') }}
          </h3>
          <ul class="flex flex-col gap-1.5">
            <li
              v-for="item in healthItems"
              :key="item.key"
              class="flex items-start justify-between gap-2 rounded-lg border border-[var(--theme-border-subtle)] px-3 py-2"
            >
              <span class="flex min-w-0 items-start gap-2 text-xs leading-5">
                <component
                  :is="item.icon"
                  :size="14"
                  class="mt-0.5 shrink-0"
                  :class="
                    item.tone === 'ok'
                      ? 'text-emerald-500 dark:text-emerald-400'
                      : item.tone === 'danger'
                        ? 'text-red-500 dark:text-red-400'
                        : 'text-amber-500 dark:text-amber-400'
                  "
                  aria-hidden="true"
                />
                <span class="min-w-0 text-[var(--theme-text-secondary)]">{{ item.text }}</span>
              </span>
              <button
                v-if="item.actionKey && item.action"
                type="button"
                class="shrink-0 text-xs font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:underline"
                @click="item.action()"
              >
                {{ t(locale, item.actionKey) }}
              </button>
            </li>
          </ul>
        </div>
      </div>
    </template>
  </section>
</template>
