/**
 * 概览页 KPI 条带（设计 6.4）纯计算逻辑。
 * 输入：当前概览窗口数据 + 速率统计；输出 6 个 KPI 卡片数据（文案为 i18n key，由组件层渲染）。
 * 不依赖任何页面局部状态，可在概览页 / 测试中复用。
 */
import { computed } from 'vue'
import type { Component, ComputedRef } from 'vue'
import {
  CircleDollarSign,
  Database,
  Gauge,
  MessageSquare,
  ShieldCheck,
  Sigma
} from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import {
  formatCost,
  formatDurationMs,
  formatRate,
  formatRequestCount,
  formatTokenPair,
  formatTokenValue
} from '../../utils/format'
import type { StatisticsMetric, WindowRateSummary, WindowUsage } from '../../types'

export interface SecondaryPart {
  /** 可翻译片段的 i18n key（与 text 二选一）。 */
  key?: string
  /** 不可翻译的原始数据片段（格式化数值 / 分隔符等）。 */
  text?: string
  /** i18n 插值参数（仅当 key 存在时有效）。 */
  params?: Record<string, string | number>
}

export interface KpiItem {
  key: string
  icon: Component
  labelKey: string
  primary: string
  /** 副标题结构化片段：组件层负责将 key 片段用 t() 渲染并与 text 片段以空格拼接。 */
  secondaryParts: SecondaryPart[]
  secondaryTitleKey?: string
  /** 点击后趋势切换到的主指标维度。 */
  metric: StatisticsMetric
  /** "仅代理覆盖"徽标。 */
  coverageTag: boolean
}

export function useOverviewKpis(
  windowData: ComputedRef<WindowUsage | null>,
  rateSummary: ComputedRef<WindowRateSummary | null>
) {
  const store = useMonitorStore()

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

  const kpis = computed<KpiItem[]>(() => {
    const d = windowData.value
    if (!d) return []
    const pair = formatTokenPair(d.inputTokens, d.outputTokens)
    const covered = coveredStatusRequests.value
    const speed = hasRateData.value ? rateSummary.value!.overall.avgTokensPerSecond : null
    const ttft = hasRateData.value ? rateSummary.value!.ttft.avgTtftMs : null
    const statusParts: SecondaryPart[] = covered > 0
      ? [
          { key: 'desktop.overview.kpiSuccess' },
          { text: formatRequestCount(d.successRequests) },
          { text: '·' },
          { key: 'desktop.overview.kpiFailed' },
          { text: formatRequestCount(d.clientErrorRequests + d.serverErrorRequests) }
        ]
      : [{ text: '--' }]
    return [
      {
        key: 'requests',
        icon: MessageSquare,
        labelKey: 'desktop.overview.kpiRequests',
        primary: formatRequestCount(d.requestUsed),
        secondaryParts: statusParts,
        metric: 'requests' as StatisticsMetric,
        coverageTag: false
      },
      {
        key: 'tokens',
        icon: Sigma,
        labelKey: 'desktop.overview.kpiTokens',
        primary: formatTokenValue(d.tokenUsed),
        secondaryParts: [
          { key: 'desktop.overview.kpiInput' },
          { text: pair.input },
          { text: '·' },
          { key: 'desktop.overview.kpiOutput' },
          { text: pair.output }
        ],
        metric: 'tokens' as StatisticsMetric,
        coverageTag: false
      },
      {
        key: 'cost',
        icon: CircleDollarSign,
        labelKey: 'desktop.overview.kpiCost',
        primary: formatCost(d.cost, store.settings.currency),
        secondaryParts: [
          { text: 'USD' },
          { text: d.cost.toFixed(4) }
        ],
        secondaryTitleKey: 'desktop.overview.kpiUsdHint',
        metric: 'cost' as StatisticsMetric,
        coverageTag: false
      },
      {
        key: 'cache',
        icon: Database,
        labelKey: 'desktop.overview.kpiCache',
        primary: formatTokenValue(d.cacheReadTokens),
        secondaryParts:
          cacheHitRate.value == null
            ? [{ text: '--' }]
            : [
                { text: `${formatRate(cacheHitRate.value)}%` },
                { key: 'desktop.overview.kpiHitRate' }
              ],
        metric: 'tokens' as StatisticsMetric,
        coverageTag: false
      },
      {
        key: 'successRate',
        icon: ShieldCheck,
        labelKey: 'desktop.overview.kpiSuccessRate',
        primary: successRate.value == null ? '--' : `${formatRate(successRate.value)}%`,
        secondaryParts: successRate.value == null
          ? [{ key: 'desktop.overview.kpiCoverageOnly' }]
          : [{ key: 'desktop.overview.kpiCovered', params: { count: formatRequestCount(coveredStatusRequests.value) } }],
        metric: 'requests' as StatisticsMetric,
        coverageTag: successRate.value == null
      },
      {
        key: 'avgSpeed',
        icon: Gauge,
        labelKey: 'desktop.overview.kpiAvgSpeed',
        primary: speed == null ? '--' : `${formatRate(speed)} t/s`,
        secondaryParts: ttft == null
          ? [{ key: 'desktop.overview.kpiCoverageOnly' }]
          : [
              { key: 'desktop.overview.kpiTtft' },
              { text: formatDurationMs(ttft) }
            ],
        metric: 'requests' as StatisticsMetric,
        coverageTag: !hasRateData.value
      }
    ]
  })

  return { kpis, hasRateData }
}
