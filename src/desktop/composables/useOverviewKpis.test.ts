import { computed } from 'vue'
import { describe, expect, it, vi } from 'vitest'
import type { WindowRateSummary, WindowUsage } from '../../types'
import { useOverviewKpis } from './useOverviewKpis'

vi.mock('../../stores/monitor', () => ({
  useMonitorStore: () => ({ settings: { currency: undefined } })
}))

const usage: WindowUsage = {
  window: 'today', requestUsed: 30, localRequestCount: 20, proxyRequestCount: 10,
  tokenUsed: 1000, inputTokens: 100, outputTokens: 200,
  cacheCreateTokens: 300, cacheReadTokens: 400, cost: 12,
  successRequests: 8, clientErrorRequests: 1, serverErrorRequests: 1
}

describe('overview KPI details', () => {
  it('keeps source counts, cache facts and original USD cost in separate rows', () => {
    const { kpis } = useOverviewKpis(computed(() => usage), computed(() => null))
    const rows = (key: string) => kpis.value.find(item => item.key === key)!.details.map(row => row.value)
    expect(rows('requests')).toEqual(['20', '10'])
    expect(rows('cost')).toEqual(['12.0000', '30'])
    expect(rows('cache')).toEqual(['300', '400'])
    expect(rows('successRate')).toEqual(['8', '2'])
    expect(kpis.value.every(item => item.details.length === 2)).toBe(true)
  })

  it('shows missing performance as unavailable instead of zero', () => {
    const rates = { overall: { requestCount: 0, avgTokensPerSecond: 0 }, ttft: { avgTtftMs: 0 } } as WindowRateSummary
    const { kpis } = useOverviewKpis(computed(() => usage), computed(() => rates))
    const speed = kpis.value.find(item => item.key === 'avgSpeed')!
    expect(speed.details.map(row => row.value)).toEqual(['--', '--'])
    expect(speed.coverageTag).toBe(true)
  })
})
