// @vitest-environment happy-dom
import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import ShareUsageCard from './ShareUsageCard.vue'
import { createDefaultSettings } from '../../stores/monitorDefaults'
import type { StatisticsSummary } from '../../types'

function dailySummary(first: Date, last: Date): StatisticsSummary {
  const totals = {
    totalTokens: 1000000, inputTokens: 900000, outputTokens: 100000, requestCount: 10,
    cacheReadTokens: 0, cacheCreateTokens: 0, cost: 1, modelCount: 0, localRequestCount: 10, proxyRequestCount: 0
  }
  const trend = []
  for (const day = new Date(first); day <= last; day.setDate(day.getDate() + 1)) {
    trend.push({ ...totals, startEpoch: day.getTime() / 1000, label: '', totalTokens: trend.length < 3 ? 1000000 : 0 })
  }
  return {
    generatedAtEpoch: last.getTime() / 1000, source: 'local-files',
    capability: { hasBasicUsage: true, hasPerformance: false, hasStatusCodes: false },
    range: { startEpoch: first.getTime() / 1000, endEpoch: last.getTime() / 1000, timezone: 'Asia/Shanghai', bucket: 'day' },
    totals, trend, models: []
  }
}

function render(summary: StatisticsSummary, visual: 'calendar' | 'year', calendarBounds?: { start: number; end: number }) {
  return mount(ShareUsageCard, { props: {
    summary, visual, calendarBounds, locale: 'zh-CN', currency: createDefaultSettings().currency,
    rangeLabel: '', scopeLabel: '', generatedAtLabel: '', displayName: '', theme: 'lavender',
    includeStats: true, includeTrend: true, includeModels: true
  } })
}

describe('share poster calendars', () => {
  it('shows leap-month dates and daily values while leaving adjacent-month padding blank', () => {
    const first = new Date(2024, 1, 1, 4)
    const last = new Date(2024, 1, 29, 4)
    const view = render(dailySummary(first, last), 'calendar', { start: first.getTime() / 1000, end: last.getTime() / 1000 })
    expect(view.findAll('.share-card__calendar time')).toHaveLength(29)
    expect(view.get('[data-date="2024-02-01"] time').text()).toBe('1')
    expect(view.get('[data-date="2024-02-01"] strong').text()).toBe('1.00M')
    expect(view.find('[data-date="2024-03-01"]').exists()).toBe(false)
    expect(view.get('.share-card__heatmap-caption p').text()).toContain('峰值 1.00M')
    view.unmount()
  })

  it('keeps a rolling calendar with six weeks and both month labels', () => {
    const summary = dailySummary(new Date(2026, 7, 30, 4), new Date(2026, 8, 28, 12))
    const view = render(summary, 'calendar')
    expect(view.findAll('.share-card__calendar-row')).toHaveLength(6)
    expect(view.findAll('.share-card__calendar time')).toHaveLength(30)
    expect(view.get('[data-date="2026-09-01"] .share-card__calendar-date').text()).toContain('9月')
    expect(view.get('.share-card__heatmap-range').text()).toContain('8月30日')
    view.unmount()
  })

  it('splits a rolling year into four bands without losing or duplicating dates, even without trend data', async () => {
    const summary = dailySummary(new Date(2025, 9, 4, 4), new Date(2026, 9, 3, 12))
    // These consecutive days cross the first band's boundary.
    summary.trend = summary.trend.map((point, index) => ({ ...point, totalTokens: index >= 84 && index < 88 ? 1000000 : 0 }))
    const view = render(summary, 'year')
    expect(view.findAll('.share-card__year-band')).toHaveLength(4)
    const dates = view.findAll('.share-card__year-grid [data-date]').map(cell => cell.attributes('data-date'))
    expect(dates).toHaveLength(365)
    expect(new Set(dates).size).toBe(365)
    expect(dates[0]).toBe('2025-10-04')
    expect(dates.at(-1)).toBe('2026-10-03')
    expect(view.get('.share-card__heatmap-caption p').text()).toContain('4 最长连续天数')
    await view.setProps({ summary: { ...summary, trend: [] } })
    expect(view.findAll('.share-card__year-grid [data-date]')).toHaveLength(365)
    expect(view.get('.share-card__heatmap-caption p').text()).toContain('0 活跃天数')
    view.unmount()
  })
})
