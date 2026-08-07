import type {
  AppSettings,
  MonthActivity,
  OverviewBreakdown,
  OverviewDeferredBundle,
  StatisticsMetric,
  StatisticsQuery,
  StatisticsSummary,
  YearActivity
} from '../types'
import { invokeWithTimeout } from './invokeWithTimeout'

const OVERVIEW_TIMEOUT_MS = 60_000

export function queryStatisticsSummary(
  settings: AppSettings,
  query: StatisticsQuery
): Promise<StatisticsSummary> {
  return invokeWithTimeout('get_statistics_summary', { query, settings })
}

export function queryMonthActivity(
  settings: AppSettings,
  year: number,
  month: number,
  metric: StatisticsMetric
): Promise<MonthActivity> {
  return invokeWithTimeout('get_month_activity', { year, month, metric, settings })
}

export function queryYearActivity(
  settings: AppSettings,
  year: number,
  metric: StatisticsMetric
): Promise<YearActivity> {
  return invokeWithTimeout('get_year_activity', { year, metric, settings })
}

export function queryOverviewBreakdown(
  settings: AppSettings,
  window: string
): Promise<OverviewBreakdown> {
  return invokeWithTimeout('get_overview_breakdown', { window, settings }, OVERVIEW_TIMEOUT_MS)
}

export function queryOverviewDeferredBundle(
  settings: AppSettings,
  window: string
): Promise<OverviewDeferredBundle> {
  return invokeWithTimeout('get_overview_deferred_bundle', { window, settings }, OVERVIEW_TIMEOUT_MS)
}
