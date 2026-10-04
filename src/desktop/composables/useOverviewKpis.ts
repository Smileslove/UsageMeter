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

export interface KpiDetail {
  labelKey?: string
  /** 币种代码等非自然语言标签。 */
  label?: string
  value: string
}

export interface KpiItem {
  key: string
  icon: Component
  labelKey: string
  primary: string
  details: KpiDetail[]
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

  const kpis = computed<KpiItem[]>(() => {
    const d = windowData.value
    if (!d) return []
    const pair = formatTokenPair(d.inputTokens, d.outputTokens)
    const covered = coveredStatusRequests.value
    const speed = hasRateData.value ? rateSummary.value!.overall.avgTokensPerSecond : null
    const ttft = hasRateData.value ? rateSummary.value!.ttft.avgTtftMs : null
    const statusDetails: KpiDetail[] = [
      { labelKey: 'desktop.overview.kpiSuccess', value: covered > 0 ? formatRequestCount(d.successRequests) : '--' },
      { labelKey: 'desktop.overview.kpiFailed', value: covered > 0 ? formatRequestCount(d.clientErrorRequests + d.serverErrorRequests) : '--' }
    ]
    return [
      {
        key: 'requests',
        icon: MessageSquare,
        labelKey: 'desktop.overview.kpiRequests',
        primary: formatRequestCount(d.requestUsed),
        details: [
          { labelKey: 'statistics.localRequests', value: formatRequestCount(d.localRequestCount) },
          { labelKey: 'statistics.proxyRequests', value: formatRequestCount(d.proxyRequestCount) }
        ],
        metric: 'requests' as StatisticsMetric,
        coverageTag: false
      },
      {
        key: 'tokens',
        icon: Sigma,
        labelKey: 'desktop.overview.kpiTokens',
        primary: formatTokenValue(d.tokenUsed),
        details: [
          { labelKey: 'desktop.overview.kpiInput', value: pair.input },
          { labelKey: 'desktop.overview.kpiOutput', value: pair.output }
        ],
        metric: 'tokens' as StatisticsMetric,
        coverageTag: false
      },
      {
        key: 'cost',
        icon: CircleDollarSign,
        labelKey: 'desktop.overview.kpiCost',
        primary: formatCost(d.cost, store.settings.currency),
        details: [
          { label: 'USD', value: d.cost.toFixed(4) },
          { labelKey: 'common.requests', value: formatRequestCount(d.requestUsed) }
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
        details: [
          { labelKey: 'statistics.cacheCreateShort', value: formatTokenValue(d.cacheCreateTokens) },
          { labelKey: 'statistics.cacheReadShort', value: formatTokenValue(d.cacheReadTokens) }
        ],
        metric: 'tokens' as StatisticsMetric,
        coverageTag: false
      },
      {
        key: 'successRate',
        icon: ShieldCheck,
        labelKey: 'desktop.overview.kpiSuccessRate',
        primary: successRate.value == null ? '--' : `${formatRate(successRate.value)}%`,
        details: statusDetails,
        metric: 'requests' as StatisticsMetric,
        coverageTag: successRate.value == null
      },
      {
        key: 'avgSpeed',
        icon: Gauge,
        labelKey: 'desktop.overview.kpiAvgSpeed',
        primary: speed == null ? '--' : `${formatRate(speed)} t/s`,
        details: [
          { labelKey: 'desktop.overview.kpiTtft', value: ttft == null ? '--' : formatDurationMs(ttft) },
          { labelKey: 'common.covered', value: hasRateData.value ? formatRequestCount(rateSummary.value!.overall.requestCount) : '--' }
        ],
        metric: 'requests' as StatisticsMetric,
        coverageTag: !hasRateData.value
      }
    ]
  })

  return { kpis, hasRateData }
}
