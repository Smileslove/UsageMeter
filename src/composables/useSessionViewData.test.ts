import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { ref } from 'vue'
import type { SessionStats } from '../types'

const {
  querySessionsMock,
  queryRecentRequestRecordsMock,
} = vi.hoisted(() => ({
  querySessionsMock: vi.fn(),
  queryRecentRequestRecordsMock: vi.fn(),
}))

// useSessionViewData 挂载真实 monitor store（Pinia）。monitor 及其依赖链顶层
// import 了 Tauri API，测试环境（node）没有 Tauri 运行时，全部桩化。
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }))
// 会话/请求查询层桩化：让 fetchSessionsAction / fetchRecentRequestRecordsAction
// 走真实失败/竞态分支，而不触碰真实后端。
vi.mock('../stores/sessionQueries', () => ({
  querySessions: querySessionsMock,
  querySessionDetail: vi.fn(),
  queryRecentRequestRecords: queryRecentRequestRecordsMock,
  queryProjectStats: vi.fn(),
}))
vi.mock('../stores/subscriptionQueries', () => ({
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

import { useMonitorStore } from '../stores/monitor'
import { useSessionViewData } from './useSessionViewData'

const PAGE_SIZE = 30

function makeSession(partial: Partial<SessionStats> = {}): SessionStats {
  return {
    sessionId: 's',
    tool: 'claude_code',
    totalRequests: 0,
    totalInputTokens: 0,
    totalOutputTokens: 0,
    totalCacheCreateTokens: 0,
    totalCacheReadTokens: 0,
    totalDurationMs: 0,
    avgOutputTokensPerSecond: 0,
    firstRequestTime: 0,
    lastRequestTime: 0,
    models: [],
    ...partial,
  } as SessionStats
}

function makePage(count: number, prefix: string): SessionStats[] {
  return Array.from({ length: count }, (_, i) => makeSession({ sessionId: `${prefix}_${i}` }))
}

describe('useSessionViewData session reload/load-more timeout & race', () => {
  let consoleErrorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    setActivePinia(createPinia())
    querySessionsMock.mockReset()
    queryRecentRequestRecordsMock.mockReset()
    // 被测 store 会在失败路径打印 console.error，静音掉以保持输出干净
    consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  afterEach(() => {
    consoleErrorSpy.mockRestore()
  })

  it('reloadSessions on timeout leaves hasMore=false and clears the session list', async () => {
    const store = useMonitorStore()
    store.sessions = [makeSession({ sessionId: 'old-1' })]
    const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
    const view = useSessionViewData(store, activeTab)

    querySessionsMock.mockRejectedValue(new Error('ERR_STATISTICS_TIMEOUT'))

    await view.reloadSessions()

    // fetchSessionsAction 捕获超时后清空列表并返回 0，reloadSessions 用 count=0 < fetchLimit 判定无更多页
    expect(view.hasMore.value).toBe(false)
    expect(store.sessions).toEqual([])
    expect(store.sessionsLoading).toBe(false)
    expect(querySessionsMock).toHaveBeenCalledWith(store.settings, PAGE_SIZE, 0, null)
  })

  it('reloadSessions sets hasMore by comparing count against fetchLimit', async () => {
    const store = useMonitorStore()
    const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
    const view = useSessionViewData(store, activeTab)

    querySessionsMock.mockResolvedValue(makePage(30, 'full'))
    await view.reloadSessions()
    expect(store.sessions).toHaveLength(30)
    expect(view.hasMore.value).toBe(true)

    querySessionsMock.mockResolvedValue(makePage(1, 'tiny'))
    await view.reloadSessions(true)
    expect(store.sessions).toHaveLength(1)
    expect(view.hasMore.value).toBe(false)
  })

  it('reloadSessions without force serves from the view cache', async () => {
    const store = useMonitorStore()
    const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
    const view = useSessionViewData(store, activeTab)

    querySessionsMock.mockResolvedValueOnce(makePage(2, 'cached'))
    await view.reloadSessions()
    expect(store.sessions).toHaveLength(2)

    querySessionsMock.mockClear()
    await view.reloadSessions()
    expect(querySessionsMock).not.toHaveBeenCalled()
    expect(store.sessions).toHaveLength(2)
  })

  it('loadMore on timeout appends nothing and resets loadingMore', async () => {
    const store = useMonitorStore()
    const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
    const view = useSessionViewData(store, activeTab)

    querySessionsMock.mockResolvedValueOnce(makePage(30, 'page1'))
    await view.reloadSessions()
    expect(view.hasMore.value).toBe(true)
    expect(store.sessions).toHaveLength(30)

    querySessionsMock.mockRejectedValue(new Error('ERR_STATISTICS_TIMEOUT'))
    await view.loadMore()

    expect(store.sessions).toHaveLength(30) // 超时页不追加
    expect(view.loadingMore.value).toBe(false)
    // 当前实现：失败返回 count=0，会被当作「没有更多数据」处理
    expect(view.hasMore.value).toBe(false)
    expect(querySessionsMock).toHaveBeenCalledWith(store.settings, PAGE_SIZE, PAGE_SIZE, null)
  })

  it('loadMore appends the next page on success', async () => {
    const store = useMonitorStore()
    const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
    const view = useSessionViewData(store, activeTab)

    querySessionsMock.mockResolvedValueOnce(makePage(30, 'page1'))
    await view.reloadSessions()

    querySessionsMock.mockResolvedValueOnce(makePage(5, 'page2'))
    await view.loadMore()

    expect(store.sessions).toHaveLength(35)
    expect(view.loadingMore.value).toBe(false)
    expect(view.hasMore.value).toBe(false) // 追加 5 条 < pageSize
  })

  it('concurrent loadMore + reloadSessions does not duplicate items', async () => {
    const store = useMonitorStore()
    const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
    const view = useSessionViewData(store, activeTab)

    querySessionsMock.mockResolvedValueOnce(makePage(30, 'page1'))
    await view.reloadSessions()

    let resolveLoadMore!: (v: SessionStats[]) => void
    let resolveReload!: (v: SessionStats[]) => void
    querySessionsMock
      .mockReturnValueOnce(new Promise(resolve => { resolveLoadMore = resolve }))
      .mockReturnValueOnce(new Promise(resolve => { resolveReload = resolve }))

    const loadMorePromise = view.loadMore()
    const reloadPromise = view.reloadSessions(true)

    // reload（append=false 覆盖）先完成
    resolveReload(makePage(2, 'reloaded'))
    await reloadPromise
    expect(store.sessions).toHaveLength(2)

    // loadMore（append=true）后到，基于 reload 后的列表追加，不产生重复
    resolveLoadMore(makePage(1, 'appended'))
    await loadMorePromise

    const ids = store.sessions.map(s => s.sessionId)
    expect(ids).toEqual(['reloaded_0', 'reloaded_1', 'appended_0'])
    expect(new Set(ids).size).toBe(ids.length) // 无重复条目
    expect(view.loadingMore.value).toBe(false)
  })

  it('loadMoreRequests on timeout appends nothing and resets loadingMoreRequests', async () => {
    const store = useMonitorStore()
    store.requestRecords = [{
      requestKey: 'k1',
      sessionId: 's1',
      tool: 'claude_code',
      timestampSec: 0,
      timestampMs: 0,
      model: 'm',
      inputTokens: 0,
      outputTokens: 0,
      cacheCreateTokens: 0,
      cacheReadTokens: 0,
      totalTokens: 0,
      estimatedCost: 0,
      coverageOrigin: 'local_only',
      outputTokensPerSecond: null,
      ttftMs: null,
    }]
    const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
    const view = useSessionViewData(store, activeTab)

    queryRecentRequestRecordsMock.mockRejectedValue(new Error('ERR_STATISTICS_TIMEOUT'))
    await view.loadMoreRequests()

    expect(view.loadingMoreRequests.value).toBe(false)
    expect(store.requestRecords).toHaveLength(1) // 超时页不追加
    expect(view.requestHasMore.value).toBe(false) // 失败按 count=0 处理
    expect(queryRecentRequestRecordsMock).toHaveBeenCalledWith(store.settings, null, 30, 30)
  })
})
