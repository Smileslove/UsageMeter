import type { OverviewBreakdownItem, StatisticsMetric } from '../types'

/**
 * 按主指标（费用 / 请求 / Token）取 breakdown item 的原始数值。
 * 用于概览页贡献排行与分析页构成视图的占比计算（设计 6.7、7.5）。
 */
export function metricValueOfBreakdownItem(item: OverviewBreakdownItem, metric: StatisticsMetric): number {
  if (metric === 'cost') return item.cost
  if (metric === 'tokens') return item.totalTokens
  return item.requestCount
}
