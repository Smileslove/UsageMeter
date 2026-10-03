import { invoke } from '@tauri-apps/api/core'
import type { ProxyStatus, RequestQueryParams, RequestRecordsPage, StatisticsMetric, StatisticsQuery, SubscriptionQueryResult } from '../types'
import type { useMonitorStore } from './monitor'
import {
  queryMonthActivity,
  queryOverviewBreakdown,
  queryStatisticsSummary,
  queryYearActivity
} from './statisticsQueries'
import {
  queryProjectStats,
  queryRecentRequestRecords,
  queryRequestRecordsPage,
  querySessionDetail,
  querySessions
} from './sessionQueries'
import {
  queryBoolean,
  queryCopilotAuthStatus,
  querySubscriptionQuota
} from './subscriptionQueries'
import { createUnavailableProxyStatus, runSubscriptionQuotaQuery } from './monitorActionHelpers'

export type MonitorDomainContext = ReturnType<typeof useMonitorStore>

export async function fetchStatisticsSummaryAction(context: MonitorDomainContext, query: StatisticsQuery) {
  const requestKey = JSON.stringify(query)
  if (context.statisticsLoading && context.statisticsRequestKey === requestKey) return
  const requestSeq = ++context.statisticsRequestSeq
  context.statisticsRequestKey = requestKey
  context.statisticsLoading = true
  try {
    context.statisticsError = ''
    const summary = await queryStatisticsSummary(context.settings, query)
    if (requestSeq === context.statisticsRequestSeq) context.statisticsSummary = summary
  } catch (error) {
    if (requestSeq === context.statisticsRequestSeq) context.statisticsError = errorMessage(error)
  } finally {
    if (requestSeq === context.statisticsRequestSeq) context.statisticsLoading = false
  }
}

export async function fetchMonthActivityAction(
  context: MonitorDomainContext,
  year: number,
  month: number,
  metric: StatisticsMetric,
  options?: { silent?: boolean }
) {
  const requestKey = JSON.stringify({ year, month, metric, settings: context.settings })
  if (context.monthActivityLoading && context.monthActivityRequestKey === requestKey) return
  const requestSeq = ++context.monthActivityRequestSeq
  context.monthActivityRequestKey = requestKey
  if (!options?.silent) context.monthActivityLoading = true
  try {
    context.statisticsError = ''
    const activity = await queryMonthActivity(context.settings, year, month, metric)
    if (requestSeq === context.monthActivityRequestSeq) context.monthActivity = activity
  } catch (error) {
    if (requestSeq === context.monthActivityRequestSeq) context.statisticsError = errorMessage(error)
  } finally {
    // 无条件按“是否为最新请求”复位 loading：silent 请求成为最新请求时，
    // 所有更早在途请求都已过期，由它收尾复位是正确语义；
    // 若这里依赖 silent 跳过，用户请求在途 + silent 后来会导致 loading 永久卡 true。
    if (requestSeq === context.monthActivityRequestSeq) context.monthActivityLoading = false
  }
}

export async function fetchYearActivityAction(context: MonitorDomainContext, year: number, metric: StatisticsMetric, options?: { silent?: boolean }) {
  const requestKey = JSON.stringify({ year, metric, settings: context.settings })
  if (context.yearActivityLoading && context.yearActivityRequestKey === requestKey) return
  const requestSeq = ++context.yearActivityRequestSeq
  context.yearActivityRequestKey = requestKey
  if (!options?.silent) context.yearActivityLoading = true
  try {
    context.statisticsError = ''
    const activity = await queryYearActivity(context.settings, year, metric)
    if (requestSeq === context.yearActivityRequestSeq) context.yearActivity = activity
  } catch (error) {
    if (requestSeq === context.yearActivityRequestSeq) context.statisticsError = errorMessage(error)
  } finally {
    // 同 month：无条件按最新请求复位 loading，避免 silent 后来时 loading 永久卡 true。
    if (requestSeq === context.yearActivityRequestSeq) context.yearActivityLoading = false
  }
}

export async function fetchOverviewBreakdownAction(context: MonitorDomainContext, window: string) {
  const requestSeq = ++context.overviewBreakdownRequestSeq
  context.overviewBreakdownLoading = true
  try {
    context.overviewBreakdownError = ''
    const breakdown = await queryOverviewBreakdown(context.settings, window)
    if (requestSeq === context.overviewBreakdownRequestSeq) context.overviewBreakdown = breakdown
  } catch (error) {
    if (requestSeq === context.overviewBreakdownRequestSeq) {
      context.overviewBreakdownError = errorMessage(error)
      context.overviewBreakdown = null
    }
  } finally {
    if (requestSeq === context.overviewBreakdownRequestSeq) context.overviewBreakdownLoading = false
  }
}

export async function fetchSessionsAction(
  context: MonitorDomainContext,
  limit = 50,
  offset = 0,
  append = false,
  toolFilter?: string | null
) {
  if (offset === 0) context.sessionsLoading = true
  try {
    const newSessions = await querySessions(context.settings, limit, offset, toolFilter)
    context.sessions = append ? [...context.sessions, ...newSessions] : newSessions
    return newSessions.length
  } catch (error) {
    console.error('Failed to fetch sessions:', error)
    if (!append) context.sessions = []
    return 0
  } finally {
    context.sessionsLoading = false
  }
}

export async function fetchSessionDetailAction(context: MonitorDomainContext, sessionId: string) {
  try {
    context.selectedSession = await querySessionDetail(context.settings, sessionId)
  } catch (error) {
    console.error('Failed to fetch session detail:', error)
    context.selectedSession = null
  }
}

export async function fetchRecentRequestRecordsAction(
  context: MonitorDomainContext,
  toolFilter: string | null,
  limit = 30,
  offset = 0,
  append = false
) {
  if (offset === 0) context.requestRecordsLoading = true
  try {
    const records = await queryRecentRequestRecords(context.settings, toolFilter, limit, offset)
    context.requestRecords = append ? [...context.requestRecords, ...records] : records
    return records.length
  } catch (error) {
    console.error('Failed to fetch request records:', error)
    if (!append) context.requestRecords = []
    return 0
  } finally {
    context.requestRecordsLoading = false
  }
}

export async function fetchRequestRecordsPageAction(
  context: MonitorDomainContext,
  toolFilter: string | null,
  params: RequestQueryParams
): Promise<RequestRecordsPage | null> {
  const requestSeq = ++context.requestRecordsPageRequestSeq
  context.requestRecordsLoading = true
  try {
    const page = await queryRequestRecordsPage(context.settings, toolFilter, params)
    if (requestSeq === context.requestRecordsPageRequestSeq) {
      context.requestRecords = page.items
      context.requestTotal = page.total
      context.requestHasMore = page.hasMore
    }
    return page
  } catch (error) {
    console.error('Failed to fetch request records page:', error)
    if (requestSeq === context.requestRecordsPageRequestSeq) {
      context.requestRecords = []
      context.requestTotal = 0
      context.requestHasMore = false
    }
    return null
  } finally {
    if (requestSeq === context.requestRecordsPageRequestSeq) {
      context.requestRecordsLoading = false
    }
  }
}

export async function fetchProjectStatsAction(context: MonitorDomainContext, toolFilter?: string | null) {
  context.projectStatsLoading = true
  try {
    context.projectStats = await queryProjectStats(context.settings, toolFilter)
  } catch (error) {
    console.error('Failed to fetch project stats:', error)
    context.projectStats = []
  } finally {
    context.projectStatsLoading = false
  }
}

export async function startProxyOnlyAction(context: MonitorDomainContext, port: number) {
  context.proxyLoading = true
  try {
    context.error = ''
    await invoke('start_proxy', { port })
    await getProxyStatusAction(context)
  } catch (error) {
    context.error = errorMessage(error)
    throw error
  } finally {
    context.proxyLoading = false
  }
}

export async function getProxyStatusAction(context: MonitorDomainContext) {
  try {
    context.proxyStatus = await invoke<ProxyStatus>('get_proxy_status')
  } catch (error) {
    console.error('Failed to get proxy status:', error)
    context.proxyStatus = createUnavailableProxyStatus()
  }
}

export async function prepareExitAction(context: MonitorDomainContext) {
  if (!context.isProxyRunning) return
  try {
    await invoke('stop_proxy_runtime_only')
  } catch (error) {
    console.error('Failed to stop proxy on exit:', error)
  }
}

export async function runProviderQuotaAction(
  context: MonitorDomainContext,
  provider: 'gpt' | 'claude' | 'gemini' | 'copilot' | 'cursor',
  force: boolean
) {
  const setLoading = (loading: boolean) => {
    if (provider === 'gpt') context.subscriptionLoading = loading
    else if (provider === 'claude') context.claudeLoading = loading
    else if (provider === 'gemini') context.geminiQuotaLoading = loading
    else if (provider === 'cursor') context.cursorQuotaLoading = loading
    else context.copilotQuotaLoading = loading
  }
  const setResult = (result: SubscriptionQueryResult) => {
    if (provider === 'gpt') context.subscriptionQuota = result
    else if (provider === 'claude') context.claudeQuota = result
    else if (provider === 'gemini') context.geminiQuota = result
    else if (provider === 'cursor') context.cursorQuota = result
    else context.copilotQuota = result
  }
  await runSubscriptionQuotaQuery(
    () => querySubscriptionQuota(provider, force),
    setLoading,
    setResult,
    error => console.error(`Failed to ${force ? 'refresh' : 'fetch'} ${provider} quota:`, error)
  )
}

export async function checkOAuthAction(context: MonitorDomainContext, command: string, key: 'hasChatGptOAuth' | 'hasClaudeOAuth' | 'hasGeminiOAuth' | 'hasCopilotAuth') {
  try {
    context[key] = await queryBoolean(command)
  } catch (error) {
    console.error(`Failed to check ${key}:`, error)
    context[key] = false
  }
}

export async function refreshCopilotAuthStatusAction(context: MonitorDomainContext) {
  try {
    context.copilotAuthStatus = await queryCopilotAuthStatus()
    context.hasCopilotAuth = !!context.copilotAuthStatus?.authenticated
  } catch (error) {
    console.error('Failed to fetch Copilot auth status:', error)
    context.copilotAuthStatus = null
    context.hasCopilotAuth = false
  }
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}
