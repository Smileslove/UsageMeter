import type { StatisticsBucket, StatisticsQuery, WindowName } from '../types'

/** 时间窗口 → 默认粒度：5h/24h/today 小时，7d/30d/current_month 天（设计 6.5）。 */
export function bucketForWindow(window: WindowName): StatisticsBucket {
  if (window === '5h' || window === '24h' || window === 'today') return 'hour'
  return 'day'
}

export function startOfBusinessDay(date: Date, dayBoundaryHour: number): Date {
  const candidate = new Date(date.getFullYear(), date.getMonth(), date.getDate(), dayBoundaryHour, 0, 0, 0)
  if (date.getHours() < dayBoundaryHour) {
    candidate.setDate(candidate.getDate() - 1)
  }
  return candidate
}

export function addDays(date: Date, days: number): Date {
  const next = new Date(date)
  next.setDate(next.getDate() + days)
  return next
}

/**
 * 时间窗口 → 统计查询区间（半开区间 [start, end)，结束时刻为开区间，避免边界重复，设计 7.3）。
 * 与快速面板 Statistics.vue 的 presetRangeDates 语义保持一致。
 */
export function rangeQueryForWindow(
  window: WindowName,
  timezone: string,
  dayBoundaryHour: number,
  now: Date = new Date()
): StatisticsQuery {
  const bucket = bucketForWindow(window)
  let start: Date
  if (window === '5h') {
    start = new Date(now.getTime() - 5 * 60 * 60 * 1000)
  } else if (window === '24h') {
    start = new Date(now.getTime() - 24 * 60 * 60 * 1000)
  } else if (window === 'today') {
    start = startOfBusinessDay(now, dayBoundaryHour)
  } else if (window === '7d') {
    start = addDays(startOfBusinessDay(now, dayBoundaryHour), -6)
  } else if (window === '30d') {
    start = addDays(startOfBusinessDay(now, dayBoundaryHour), -29)
  } else {
    // current_month：本月 1 号（业务日界）起
    start = new Date(now.getFullYear(), now.getMonth(), 1, dayBoundaryHour, 0, 0, 0)
  }
  return {
    startEpoch: Math.floor(start.getTime() / 1000),
    endEpoch: Math.floor(now.getTime() / 1000),
    timezone,
    bucket
  }
}

/** 上一等长周期（与给定查询等长、紧邻其前，用于分析页"比较"）。 */
export function previousRangeQuery(query: StatisticsQuery): StatisticsQuery {
  const span = query.endEpoch - query.startEpoch
  return {
    ...query,
    startEpoch: query.startEpoch - span,
    endEpoch: query.startEpoch
  }
}
