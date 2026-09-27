// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { shallowMount } from '@vue/test-utils'
import type {
  OverviewDeferredBundle,
  StatisticsSummary,
  UsageSnapshot,
  WindowUsage
} from '../../types'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))

// 概览页依赖 monitor store（经 invoke 调用后端）与 subscriptionQueries；
// 测试环境无 Tauri 运行时，全部桩化。
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('../../stores/subscriptionQueries', () => ({
  queryBoolean: vi.fn(),
  queryCopilotAuthStatus: vi.fn(),
  querySubscriptionQuota: vi.fn(),
  queryConfiguredSourceQuotas: vi.fn().mockResolvedValue({
    quotas: [], attemptedCount: 0, successCount: 0, failedCount: 0, errors: [], queriedAt: 0,
  }),
  querySourceQuotaProfiles: vi.fn(),
  querySourceQuotaBindingStates: vi.fn().mockResolvedValue([]),
  probeSourceQuotaBinding: vi.fn(),
  testSourceQuotaBinding: vi.fn(),
  queryCopilotAccounts: vi.fn(),
}))

import { useMonitorStore } from '../../stores/monitor'
import { useDesktopAnalyticsStore } from '../stores/desktopAnalytics'
import DesktopOverview from './DesktopOverview.vue'

/** 快照仅含 summaryWindow 一个窗口（refresh_usage_bundle 的真实行为）。 */
function windowUsage(window: string, requestUsed: number): WindowUsage {
  return {
    window,
    tokenUsed: requestUsed * 1000,
    inputTokens: requestUsed * 700,
    outputTokens: requestUsed * 300,
    cacheCreateTokens: 0,
    cacheReadTokens: 0,
    requestUsed,
    localRequestCount: requestUsed,
    proxyRequestCount: 0,
    cost: requestUsed * 0.01,
    successRequests: requestUsed,
    clientErrorRequests: 0,
    serverErrorRequests: 0
  }
}

function snapshot(windows: WindowUsage[]): UsageSnapshot {
  return {
    generatedAtEpoch: 1,
    windows,
    source: 'local-files',
    summary: {
      totalTokens: 0,
      totalRequests: 0,
      totalInputTokens: 0,
      totalOutputTokens: 0,
      totalCacheCreateTokens: 0,
      totalCacheReadTokens: 0,
      totalCost: 0,
      totalSuccessRequests: 0,
      totalClientErrorRequests: 0,
      totalServerErrorRequests: 0
    },
    modelDistribution: []
  }
}

function deferredBundle(window: string, requestUsed: number): OverviewDeferredBundle {
  const usage = windowUsage(window, requestUsed)
  return {
    window,
    generatedAtEpoch: 2,
    windowUsage: usage,
    usageSummary: {
      totalTokens: usage.tokenUsed,
      totalRequests: usage.requestUsed,
      totalInputTokens: usage.inputTokens,
      totalOutputTokens: usage.outputTokens,
      totalCacheCreateTokens: 0,
      totalCacheReadTokens: 0,
      totalCost: usage.cost,
      totalSuccessRequests: usage.successRequests,
      totalClientErrorRequests: 0,
      totalServerErrorRequests: 0
    },
    modelDistribution: [],
    rateSummary: {
      window,
      overall: {
        requestCount: usage.requestUsed,
        totalOutputTokens: usage.outputTokens,
        totalDurationMs: 0,
        avgTokensPerSecond: 0
      },
      byModel: [],
      ttft: { requestCount: 0, avgTtftMs: 0, minTtftMs: 0, maxTtftMs: 0 },
      ttftByModel: []
    },
    overviewBreakdown: {
      window,
      generatedAtEpoch: 2,
      sourceRanking: [],
      toolRanking: [],
      modelRanking: [],
      capability: {
        hasSource: false,
        hasTool: false,
        hasCost: false,
        hasStatus: false,
        hasPerformance: false
      }
    }
  }
}

const STAT_SUMMARY: StatisticsSummary = {
  generatedAtEpoch: 2,
  source: 'local-files',
  capability: { hasBasicUsage: true, hasPerformance: false, hasStatusCodes: false },
  range: {
    startEpoch: 0,
    endEpoch: 100,
    timezone: 'UTC',
    bucket: 'hour'
  },
  totals: {
    requestCount: 0,
    totalTokens: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheCreateTokens: 0,
    cacheReadTokens: 0,
    cost: 0,
    modelCount: 0,
    localRequestCount: 0,
    proxyRequestCount: 0,
    successRequests: 0,
    errorRequests: 0
  },
  trend: [],
  models: []
}

async function flushDebounce() {
  await new Promise(resolve => setTimeout(resolve, 80))
}

describe('DesktopOverview 窗口切换', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    invokeMock.mockReset()
    invokeMock.mockImplementation(async (cmd: string, args?: { window?: string }) => {
      if (cmd === 'get_overview_deferred_bundle') {
        const window = args?.window ?? '24h'
        return deferredBundle(window, window === '30d' ? 500 : 100)
      }
      if (cmd === 'get_statistics_summary') return STAT_SUMMARY
      if (cmd === 'get_sessions') return []
      throw new Error(`unexpected invoke: ${cmd}`)
    })
  })

  it('切换目标窗口不在快照列表时，按所选窗口显式拉取（回归：快照仅含 summaryWindow 导致切换不生效）', async () => {
    const store = useMonitorStore()
    const analytics = useDesktopAnalyticsStore()
    // 快照仅含 24h（后端 refresh_usage_bundle 只返回 summaryWindow 一个窗口）
    store.snapshot = snapshot([windowUsage('24h', 100)])
    store.settings.summaryWindow = '24h'
    analytics.overviewWindow = '24h'

    const wrapper = shallowMount(DesktopOverview, {
      global: {
        stubs: {
          VChart: true,
          'v-chart': true
        }
      }
    })
    await flushDebounce()

    // 挂载时按初始窗口（24h）拉取，不应有 30d 请求
    const before = invokeMock.mock.calls.filter(([cmd]) => cmd === 'get_overview_deferred_bundle')
    expect(before.some(([, args]) => args?.window === '30d')).toBe(false)

    // 用户点击 30d：目标窗口不在快照列表，必须按所选窗口拉取
    analytics.overviewWindow = '30d'
    await flushDebounce()

    const calls = invokeMock.mock.calls.filter(([cmd]) => cmd === 'get_overview_deferred_bundle')
    expect(calls.some(([, args]) => args?.window === '30d')).toBe(true)

    // 数据到达后 upsert 进快照，effectiveWindow 跟随切换
    await flushDebounce()
    expect(store.snapshot?.windows.some(w => w.window === '30d' && w.requestUsed === 500)).toBe(true)
    expect(analytics.overviewWindow).toBe('30d')
    wrapper.unmount()
  })
})
