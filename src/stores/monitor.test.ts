import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import type { OverviewDeferredBundle, UsageRefreshBundle } from '../types'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))

// monitor store 直接经 @tauri-apps/api/core 的 invoke 调用后端；测试环境无
// Tauri 运行时，全部桩化。subscriptionQueries 也被桩化，避免 refreshUsage 内部
// fire-and-forget 的第三方额度查询触碰真实链路。
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('./subscriptionQueries', () => ({
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

import { useMonitorStore } from './monitor'

// refresh_usage_bundle 的合法响应
const BUNDLE: UsageRefreshBundle = {
  generatedAtEpoch: 123,
  snapshot: {
    generatedAtEpoch: 123,
    windows: [],
    source: 'local-files',
    summary: {
      totalTokens: 0, totalRequests: 0, totalInputTokens: 0, totalOutputTokens: 0,
      totalCacheCreateTokens: 0, totalCacheReadTokens: 0, totalCost: 0,
      totalSuccessRequests: 0, totalClientErrorRequests: 0, totalServerErrorRequests: 0,
    },
    modelDistribution: [],
  },
  limitSurvival: {
    generatedAtEpoch: 123,
    sourceKind: 'none',
    block: null,
    burn: {
      tokensPerHour: 0, requestsPerHour: 0, sampleSeconds: 0, sampleRequests: 0,
      confidence: 'low',
    },
    baseline: null,
  },
}

// get_overview_deferred_bundle 的合法响应（refreshUsage 会 fire-and-forget 发起）
const DEFERRED_BUNDLE: OverviewDeferredBundle = {
  window: '24h',
  generatedAtEpoch: 1,
  windowUsage: {
    window: '24h', tokenUsed: 0, inputTokens: 0, outputTokens: 0,
    cacheCreateTokens: 0, cacheReadTokens: 0, requestUsed: 0,
    localRequestCount: 0, proxyRequestCount: 0, cost: 0,
    successRequests: 0, clientErrorRequests: 0, serverErrorRequests: 0,
  },
  usageSummary: {
    totalTokens: 0, totalRequests: 0, totalInputTokens: 0, totalOutputTokens: 0,
    totalCacheCreateTokens: 0, totalCacheReadTokens: 0, totalCost: 0,
    totalSuccessRequests: 0, totalClientErrorRequests: 0, totalServerErrorRequests: 0,
  },
  modelDistribution: [],
  rateSummary: {
    window: '24h',
    overall: { requestCount: 0, totalOutputTokens: 0, totalDurationMs: 0, avgTokensPerSecond: 0 },
    byModel: [],
    ttft: { requestCount: 0, avgTtftMs: 0, minTtftMs: 0, maxTtftMs: 0 },
    ttftByModel: [],
  },
  overviewBreakdown: {
    window: '24h',
    generatedAtEpoch: 1,
    sourceRanking: [],
    toolRanking: [],
    modelRanking: [],
    capability: { hasSource: true, hasTool: true, hasCost: true, hasStatus: true, hasPerformance: true },
  },
}

/** 统计某个 command 被调用的次数（refreshUsage 内部还会调用其他 command）。 */
function callsFor(command: string): number {
  return invokeMock.mock.calls.filter(call => call[0] === command).length
}

function mockSuccessfulInvoke() {
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === 'refresh_usage_bundle') return Promise.resolve(BUNDLE)
    if (cmd === 'get_overview_deferred_bundle') return Promise.resolve(DEFERRED_BUNDLE)
    return Promise.resolve(undefined)
  })
}

describe('monitor store refreshUsage timeout & auto-refresh guard', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  afterEach(() => {
    invokeMock.mockReset()
    vi.useRealTimers()
  })

  it('blocks concurrent refreshUsage calls while a request is pending', async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'refresh_usage_bundle') return new Promise(() => {}) // 永不 settle（模拟后端挂起/超时前）
      return Promise.resolve(undefined)
    })
    const store = useMonitorStore()

    const first = store.refreshUsage()
    const second = store.refreshUsage()

    await second // loading guard 使其立即返回
    expect(store.loading).toBe(true)
    expect(callsFor('refresh_usage_bundle')).toBe(1)
    // first 永久挂起，无法 await；loading 在测试结束时仍未复位（符合预期）
    void first
  })

  it('recovers after a failed refresh so the next refresh succeeds', async () => {
    vi.useFakeTimers()
    let attempts = 0
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === 'refresh_usage_bundle') {
        attempts += 1
        if (attempts === 1) return Promise.reject(new Error('boom'))
        return Promise.resolve(BUNDLE)
      }
      if (cmd === 'get_overview_deferred_bundle') return Promise.resolve(DEFERRED_BUNDLE)
      return Promise.resolve(undefined)
    })
    const store = useMonitorStore()

    const first = store.refreshUsage()
    await vi.advanceTimersByTimeAsync(400) // 越过 finally 的最小加载时间 300ms
    await first
    expect(store.error).toBe('boom')
    expect(store.loading).toBe(false)

    const second = store.refreshUsage()
    await vi.advanceTimersByTimeAsync(400)
    await second
    // refreshUsage 落地 bundle.snapshot；随后 fire-and-forget 的 deferred bundle
    // 经 mergeDeferredOverviewSnapshot 合并出新的 snapshot 对象（引用不同，断言具体值）
    expect(store.snapshot?.generatedAtEpoch).toBe(123)
    expect(store.snapshot?.windows).toEqual([DEFERRED_BUNDLE.windowUsage])
    expect(store.lastUpdatedEpoch).toBe(123)
    expect(store.error).toBe('')
    expect(store.loading).toBe(false)
    expect(callsFor('refresh_usage_bundle')).toBe(2)
  })

  it('auto refresh stops after stopAutoRefresh and restarts with a fresh generation', async () => {
    vi.useFakeTimers()
    mockSuccessfulInvoke()
    const store = useMonitorStore()

    store.startAutoRefresh()
    // 第一个周期：interval(30s) 触发刷新，再越过 300ms 最小加载
    await vi.advanceTimersByTimeAsync(30000 + 400)
    expect(callsFor('refresh_usage_bundle')).toBe(1)
    expect(store.loading).toBe(false)

    // 第二个周期继续轮询
    await vi.advanceTimersByTimeAsync(30000 + 400)
    expect(callsFor('refresh_usage_bundle')).toBe(2)

    // stopAutoRefresh：generation 失效，后续时间不再触发
    store.stopAutoRefresh()
    await vi.advanceTimersByTimeAsync(120000)
    expect(callsFor('refresh_usage_bundle')).toBe(2)

    // 再次 startAutoRefresh 后，新 generation 恢复轮询
    store.startAutoRefresh()
    await vi.advanceTimersByTimeAsync(30000 + 400)
    expect(callsFor('refresh_usage_bundle')).toBe(3)
  })
})
