/**
 * @deprecated 此文件已拆分为 chartSetup.ts + trendChartBuilders.ts。
 * 保留 re-export 以保持向后兼容，消费方应逐步迁移到直接导入新文件。
 */
export { registerChartComponents, useChartTheme, type TrendChartTheme } from './chartSetup'
export {
  metricValue, formatMetric, metricLabel, metricColor,
  trendLineSeries, buildTrendChartOption,
  type TrendLineSeriesInput, type TrendChartOptionInput,
} from './trendChartBuilders'
