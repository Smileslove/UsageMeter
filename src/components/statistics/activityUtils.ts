import type { DayActivity, StatisticsMetric } from '../../types'

export interface CalendarCell {
  key: string
  day: DayActivity | null
  dayNumber: string
}

export interface AnnualCell {
  key: string
  day: DayActivity | null
}

export const METRICS: Array<{ value: StatisticsMetric; key: string }> = [
  { value: 'cost', key: 'statistics.metricCost' },
  { value: 'requests', key: 'statistics.metricRequests' },
  { value: 'tokens', key: 'statistics.metricTokens' }
]

export function formatLocalDate(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

export function getMonthDayCount(year: number, month: number): number {
  return new Date(year, month, 0).getDate()
}

export function makeEmptyDay(date: string): DayActivity {
  return {
    date,
    requestCount: 0,
    totalTokens: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheCreateTokens: 0,
    cacheReadTokens: 0,
    cost: 0,
    modelCount: 0,
    successRequests: null,
    errorRequests: null
  }
}

export function intensityClass(ratio: number): string {
  if (ratio <= 0) return 'bg-gray-100 dark:bg-neutral-800'
  if (ratio < 0.25) return 'bg-emerald-200 dark:bg-emerald-900/70'
  if (ratio < 0.5) return 'bg-emerald-300 dark:bg-emerald-700'
  if (ratio < 0.75) return 'bg-emerald-400 dark:bg-emerald-500'
  return 'bg-emerald-500 dark:bg-emerald-300'
}

export function valueOf(day: DayActivity, metric: StatisticsMetric): number {
  if (metric === 'cost') return day.cost
  if (metric === 'tokens') return day.totalTokens
  return day.requestCount
}

export function getTodayKey(): string {
  return formatLocalDate(new Date())
}
