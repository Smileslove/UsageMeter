import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import type { AppSettings, MonthActivity, OverviewBreakdown, SessionStats, StatisticsQuery, StatisticsSummary, YearActivity } from '../types'

const {
  queryStatisticsSummaryMock,
  queryOverviewBreakdownMock,
  querySessionsMock,
  queryMonthActivityMock,
  queryYearActivityMock,
} = vi.hoisted(() => ({
  queryStatisticsSummaryMock: vi.fn(),
  queryOverviewBreakdownMock: vi.fn(),
  querySessionsMock: vi.fn(),
  queryMonthActivityMock: vi.fn(),
  queryYearActivityMock: vi.fn(),
}))

// monitorDomains 各 action 直接消费查询层函数；statisticsQueries/sessionQueries 的
// 其他导出（月份/年度活动、会话详情等）在本文件不覆盖，统一桩化为空函数。
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('./statisticsQueries', () => ({
  queryStatisticsSummary: queryStatisticsSummaryMock,
  queryMonthActivity: queryMonthActivityMock,
  queryYearActivity: queryYearActivityMock,
  queryOverviewBreakdown: queryOverviewBreakdownMock,
}))
vi.mock('./sessionQueries', () => ({
  querySessions: querySessionsMock,
  querySessionDetail: vi.fn(),
  queryRecentRequestRecords: vi.fn(),
  queryProjectStats: vi.fn(),
}))
vi.mock('./subscriptionQueries', () => ({
  queryBoolean: vi.fn(),
  queryCopilotAuthStatus: vi.fn(),
  querySubscriptionQuota: vi.fn(),
}))

import type { MonitorDomainContext } from './monitorDomains'
import {
  errorMessage,
  fetchMonthActivityAction,
  fetchOverviewBreakdownAction,
  fetchSessionsAction,
  fetchStatisticsSummaryAction,
  fetchYearActivityAction,
} from './monitorDomains'

const QUERY_A: StatisticsQuery = { startEpoch: 1, endEpoch: 2, timezone: 'Asia/Shanghai', bucket: 'hour' }
const QUERY_B: StatisticsQuery = { startEpoch: 3, endEpoch: 4, timezone: 'Asia/Shanghai', bucket: 'day' }

function makeSummary(partial: Partial<StatisticsSummary> = {}): StatisticsSummary {
  return {
    generatedAtEpoch: 0,
    source: 'local-files',
    capability: { hasBasicUsage: true, hasPerformance: true, hasStatusCodes: true },
    range: { startEpoch: 0, endEpoch: 0, timezone: 'Asia/Shanghai', bucket: 'hour' },
    totals: {
      requestCount: 0, totalTokens: 0, inputTokens: 0, outputTokens: 0,
      cacheCreateTokens: 0, cacheReadTokens: 0, cost: 0, modelCount: 0,
      localRequestCount: 0, proxyRequestCount: 0,
    },
    trend: [],
    models: [],
    ...partial,
  } as StatisticsSummary
}

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

/** 最小化的 monitor store context：仅含被测 action 会读写的字段。 */
function makeContext(): MonitorDomainContext {
  return {
    settings: { summaryWindow: '24h' } as unknown as AppSettings,
    // statistics summary 字段
    statisticsLoading: false,
    statisticsRequestKey: '',
    statisticsRequestSeq: 0,
    statisticsError: '',
    statisticsSummary: null,
    // month/year activity 字段
    monthActivity: null as MonthActivity | null,
    monthActivityLoading: false,
    monthActivityRequestKey: '',
    monthActivityRequestSeq: 0,
    yearActivity: null as YearActivity | null,
    yearActivityLoading: false,
    yearActivityRequestKey: '',
    yearActivityRequestSeq: 0,
    // sessions 字段
    sessions: [] as SessionStats[],
    sessionsLoading: false,
    // overview breakdown 字段
    overviewBreakdownRequestSeq: 0,
    overviewBreakdownLoading: false,
    overviewBreakdownError: '',
    overviewBreakdown: null as OverviewBreakdown | null,
  } as unknown as MonitorDomainContext
}

describe('fetchStatisticsSummaryAction', () => {
  beforeEach(() => {
    queryStatisticsSummaryMock.mockReset()
  })

  it('sets statisticsError and resets loading on timeout while keeping the old summary', async () => {
    const ctx = makeContext()
    const oldSummary = makeSummary({ generatedAtEpoch: 11 })
    ctx.statisticsSummary = oldSummary

    queryStatisticsSummaryMock.mockRejectedValue(new Error('ERR_STATISTICS_TIMEOUT'))

    await fetchStatisticsSummaryAction(ctx, QUERY_A)

    expect(ctx.statisticsError).toBe('ERR_STATISTICS_TIMEOUT')
    expect(ctx.statisticsLoading).toBe(false)
    // 超时失败不清空已有结果
    expect(ctx.statisticsSummary).toBe(oldSummary)
    expect(queryStatisticsSummaryMock).toHaveBeenCalledWith(ctx.settings, QUERY_A)
  })

  it('lets a slow stale request lose the race against a newer summary', async () => {
    const ctx = makeContext()
    let resolveFirst!: (v: StatisticsSummary) => void
    let resolveSecond!: (v: StatisticsSummary) => void
    queryStatisticsSummaryMock
      .mockReturnValueOnce(new Promise(resolve => { resolveFirst = resolve }))
      .mockReturnValueOnce(new Promise(resolve => { resolveSecond = resolve }))

    const first = fetchStatisticsSummaryAction(ctx, QUERY_A)
    const second = fetchStatisticsSummaryAction(ctx, QUERY_B)
    expect(ctx.statisticsRequestSeq).toBe(2)

    // 新请求先返回
    resolveSecond(makeSummary({ generatedAtEpoch: 2 }))
    await second
    expect(ctx.statisticsSummary?.generatedAtEpoch).toBe(2)
    expect(ctx.statisticsLoading).toBe(false)

    // 慢请求（requestSeq=1）后到，不得覆盖 seq=2 的新结果
    resolveFirst(makeSummary({ generatedAtEpoch: 1 }))
    await first
    expect(ctx.statisticsSummary?.generatedAtEpoch).toBe(2)
    expect(ctx.statisticsLoading).toBe(false)
  })

  it('deduplicates concurrent requests with the same request key', async () => {
    const ctx = makeContext()
    queryStatisticsSummaryMock.mockReturnValue(new Promise(() => {}))
    ctx.statisticsLoading = true
    ctx.statisticsRequestKey = JSON.stringify(QUERY_A)

    await fetchStatisticsSummaryAction(ctx, QUERY_A)

    expect(queryStatisticsSummaryMock).not.toHaveBeenCalled()
    expect(ctx.statisticsRequestSeq).toBe(0)
  })
})

describe('fetchSessionsAction', () => {
  let consoleErrorSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    querySessionsMock.mockReset()
    // 失败路径会在 monitorDomains 内打 console.error，测试中静默以避免 stderr 噪音
    consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  afterEach(() => {
    consoleErrorSpy.mockRestore()
  })

  it('clears the session list and returns 0 when the first page times out', async () => {
    const ctx = makeContext()
    ctx.sessions = [makeSession({ sessionId: 'old-1' }), makeSession({ sessionId: 'old-2' })]

    querySessionsMock.mockRejectedValue(new Error('ERR_STATISTICS_TIMEOUT'))

    const count = await fetchSessionsAction(ctx)

    expect(count).toBe(0)
    expect(ctx.sessions).toEqual([])
    expect(ctx.sessionsLoading).toBe(false)
    expect(querySessionsMock).toHaveBeenCalledWith(ctx.settings, 50, 0, undefined)
  })

  it('keeps the existing list when an append (load-more) page times out', async () => {
    const ctx = makeContext()
    const existing = [makeSession({ sessionId: 'old-1' })]
    ctx.sessions = [...existing]

    querySessionsMock.mockRejectedValue(new Error('ERR_STATISTICS_TIMEOUT'))

    const count = await fetchSessionsAction(ctx, 30, 30, true)

    expect(count).toBe(0)
    // append 模式失败不清空已加载列表
    expect(ctx.sessions).toEqual(existing)
    expect(ctx.sessionsLoading).toBe(false)
  })
})

describe('fetchOverviewBreakdownAction', () => {
  beforeEach(() => {
    queryOverviewBreakdownMock.mockReset()
  })

  it('sets error, clears breakdown and resets loading on timeout', async () => {
    const ctx = makeContext()
    ctx.overviewBreakdown = { window: '24h' } as OverviewBreakdown

    queryOverviewBreakdownMock.mockRejectedValue(new Error('ERR_STATISTICS_TIMEOUT'))

    await fetchOverviewBreakdownAction(ctx, '24h')

    expect(ctx.overviewBreakdownError).toBe('ERR_STATISTICS_TIMEOUT')
    expect(ctx.overviewBreakdown).toBeNull()
    expect(ctx.overviewBreakdownLoading).toBe(false)
    expect(queryOverviewBreakdownMock).toHaveBeenCalledWith(ctx.settings, '24h')
  })
})

describe('errorMessage', () => {
  it('maps Error instances and primitives to strings', () => {
    expect(errorMessage(new Error('ERR_STATISTICS_TIMEOUT'))).toBe('ERR_STATISTICS_TIMEOUT')
    expect(errorMessage('boom')).toBe('boom')
    expect(errorMessage(42)).toBe('42')
  })
})

describe('activity actions with silent refresh', () => {
  const MONTH_ACTIVITY: MonthActivity = {
    year: 2024,
    month: 3,
    timezone: 'Asia/Shanghai',
    metric: 'cost',
    days: [
      {
        date: '2024-03-01',
        requestCount: 10,
        totalTokens: 100,
        inputTokens: 40,
        outputTokens: 60,
        cacheCreateTokens: 0,
        cacheReadTokens: 0,
        cost: 0.01,
        modelCount: 1
      }
    ]
  }
  const YEAR_ACTIVITY: YearActivity = {
    year: 2024,
    timezone: 'Asia/Shanghai',
    metric: 'cost',
    days: MONTH_ACTIVITY.days
  }

  beforeEach(() => {
    queryMonthActivityMock.mockReset()
    queryYearActivityMock.mockReset()
  })

  it('silent refresh does not flip loading but still updates data on success', async () => {
    const ctx = makeContext()
    ctx.monthActivityLoading = false
    queryMonthActivityMock.mockResolvedValue(MONTH_ACTIVITY)

    await fetchMonthActivityAction(ctx, 2024, 3, 'cost', { silent: true })

    // silent 不翻转 loading（false 保持 false），其余逻辑与正常路径一致
    expect(ctx.monthActivityLoading).toBe(false)
    expect(ctx.statisticsError).toBe('')
    expect(ctx.monthActivity).toEqual(MONTH_ACTIVITY)
    expect(queryMonthActivityMock).toHaveBeenCalledWith(ctx.settings, 2024, 3, 'cost')
  })

  it('silent refresh does not flip loading, keeps old data and sets error on failure', async () => {
    const ctx = makeContext()
    const oldActivity = { ...YEAR_ACTIVITY }
    ctx.yearActivityLoading = false
    ctx.yearActivity = oldActivity
    queryYearActivityMock.mockRejectedValue(new Error('ERR_STATISTICS_TIMEOUT'))

    await fetchYearActivityAction(ctx, 2024, 'cost', { silent: true })

    // 失败时 loading 同样不被翻转；statisticsError 清空/设置逻辑与正常路径一致
    expect(ctx.yearActivityLoading).toBe(false)
    expect(ctx.yearActivity).toBe(oldActivity)
    expect(ctx.statisticsError).toBe('ERR_STATISTICS_TIMEOUT')
    expect(queryYearActivityMock).toHaveBeenCalledWith(ctx.settings, 2024, 'cost')
  })

  it('silent refresh settles a pre-existing loading state when it becomes the latest request', async () => {
    const ctx = makeContext()
    let resolveQuery!: (v: MonthActivity) => void
    queryMonthActivityMock.mockReturnValue(new Promise(resolve => { resolveQuery = resolve }))
    // 模拟用户正在发起请求（loading=true，key 与本次不同所以不触发防重）
    ctx.monthActivityLoading = true
    ctx.monthActivityRequestKey = JSON.stringify({ year: 2023, month: 1, metric: 'cost', settings: ctx.settings })

    const pending = fetchMonthActivityAction(ctx, 2024, 3, 'cost', { silent: true })
    // 请求期间 silent 不设置 loading，保持既有 true
    expect(ctx.monthActivityLoading).toBe(true)
    resolveQuery(MONTH_ACTIVITY)
    await pending
    // silent 请求成为最新请求后必须复位 loading（否则 loading 永久卡 true）
    expect(ctx.monthActivityLoading).toBe(false)
    expect(ctx.monthActivity).toEqual(MONTH_ACTIVITY)
  })
})
