import type { ProjectStats, ProjectToolStats, RequestRecord, SessionStats } from '../types'
import { sourceLabel, t } from '../i18n'
import { TOOL_LOBE_ICONS } from '../iconConfig'
import { getFamilyHead } from '../toolFamilies'
import { formatCost as formatCostUtil, formatTokenValue, formatRequestCount } from '../utils/format'
import { formatModelDisplayName } from '../utils/modelDisplay'
import { formatToolDisplayName, getToolProfileByTool } from '../utils/toolDisplay'
import type { useMonitorStore } from '../stores/monitor'

const uuidLikePattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** Shared, locale-aware presentation helpers for the session and request views. */
export function useSessionDisplay(store: ReturnType<typeof useMonitorStore>) {
  const formatTime = (epoch: number) => {
    if (!epoch) return '-'
    const date = new Date(epoch * 1000)
    const now = new Date()
    const diffMs = now.getTime() - date.getTime()
    const diffMins = Math.floor(diffMs / 60000)
    const diffHours = Math.floor(diffMs / 3600000)
    const isSameYear = date.getFullYear() === now.getFullYear()
    const locale = store.settings.locale.replace('_', '-')

    if (diffMins < 1) return t(store.settings.locale, 'common.justNow')
    if (diffMins < 60) return t(store.settings.locale, 'sessions.timeMinutesAgo', { count: diffMins })
    if (diffHours < 24) return t(store.settings.locale, 'sessions.timeHoursAgo', { count: diffHours })

    return date.toLocaleString(locale, {
      year: isSameYear ? undefined : 'numeric',
      month: 'numeric',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    })
  }

  const formatTokens = (tokens: number) => (!tokens ? '0' : formatTokenValue(tokens))

  const formatCost = (cost: number | undefined) => (
    cost === undefined || cost === null ? '-' : formatCostUtil(cost, store.settings.currency, 4)
  )

  const formatDuration = (ms?: number | null) => {
    if (!ms) return '—'
    if (ms < 1000) return `${ms}ms`
    if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`
    const minutes = Math.floor(ms / 60000)
    const seconds = Math.round((ms % 60000) / 1000)
    return `${minutes}m ${seconds}s`
  }

  const requestModelLabel = (request: RequestRecord) => (
    formatModelDisplayName(request.model, request.tool, store.settings.locale, store.settings.clientTools.profiles)
  )

  const sessionModelLabel = (session: SessionStats) => (
    formatModelDisplayName(session.models[0], session.tool, store.settings.locale, store.settings.clientTools.profiles)
  )

  const requestStatusLabel = (request: RequestRecord) => {
    if (request.coverageOrigin === 'local_only') return t(store.settings.locale, 'sessions.requestLocalOnly')
    const statusCode = request.statusCode
    if (!statusCode) return t(store.settings.locale, 'common.unknown')
    if (statusCode < 400) return t(store.settings.locale, 'common.success')
    return t(store.settings.locale, 'common.error')
  }

  const requestStatusClasses = (request: RequestRecord) => {
    if (request.coverageOrigin === 'local_only') {
      return 'bg-slate-50 text-slate-500 border-slate-100 dark:bg-white/[0.04] dark:text-slate-300 dark:border-white/8'
    }
    const statusCode = request.statusCode || 0
    if (statusCode >= 400) {
      return 'bg-rose-50 text-rose-600 border-rose-100 dark:bg-rose-500/15 dark:text-rose-300 dark:border-rose-400/20'
    }
    return 'bg-emerald-50 text-emerald-600 border-emerald-100 dark:bg-emerald-500/15 dark:text-emerald-300 dark:border-emerald-400/20'
  }

  const requestCoverageLabel = (origin: RequestRecord['coverageOrigin']) => {
    if (origin === 'proxy_only') return t(store.settings.locale, 'sessions.requestCoverageProxy')
    if (origin === 'merged_proxy_preferred' || origin === 'merged_fuzzy_matched') {
      return t(store.settings.locale, 'sessions.requestCoverageMerged')
    }
    return t(store.settings.locale, 'sessions.requestCoverageLocal')
  }

  const requestProjectLabel = (request: RequestRecord) => {
    const pathParts = request.projectPath?.split('/').filter(Boolean) || []
    return request.projectName?.trim()
      || pathParts[pathParts.length - 1]
      || t(store.settings.locale, 'common.unknownProject')
  }

  const requestSourceLabel = (request: RequestRecord) => (
    request.sourceLabel?.trim() ? sourceLabel(store.settings.locale, request.sourceLabel)
      || request.requestBaseUrl?.trim()
      || t(store.settings.locale, 'source.unknown')
      : request.requestBaseUrl?.trim()
        || t(store.settings.locale, 'source.unknown')
  )

  const requestAttributionLabel = (request: RequestRecord): string | null => {
    if (request.attributionMethod === 'config_inferred') {
      return t(store.settings.locale, 'sessions.requestAttributionInferred')
    }
    if (request.attributionMethod === 'manual') {
      return t(store.settings.locale, 'sessions.requestAttributionManual')
    }
    if (request.coverageOrigin === 'local_only') {
      return t(store.settings.locale, 'sessions.requestAttributionUnattributed')
    }
    return null
  }

  const requestToolLabel = (tool: string) => (
    formatToolDisplayName(tool, store.settings.locale, store.settings.clientTools.profiles)
  )

  const requestCacheTokens = (request: RequestRecord) => (
    (request.cacheCreateTokens || 0) + (request.cacheReadTokens || 0)
  )

  const requestHasProxyPerformance = (request: RequestRecord) => (
    request.durationMs !== null && request.durationMs !== undefined
  )

  const sessionCacheHitRate = (session: SessionStats): string => {
    const total = (session.totalInputTokens || 0) + (session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)
    if (total === 0) return '—'
    return `${((session.totalCacheReadTokens || 0) / total * 100).toFixed(1)}%`
  }

  const coveredRequests = (value?: number) => value || 0
  const uncoveredRequests = (value?: number) => value || 0
  const localRequests = (session: SessionStats) => coveredRequests(session.coveredRequests) + uncoveredRequests(session.uncoveredRequests)
  const hasReasonixCoverageData = (covered?: number, uncovered?: number, usageFullyCovered?: boolean) => (
    (covered || 0) > 0 || (uncovered || 0) > 0 || usageFullyCovered === false
  )
  const sessionUsageVisible = (session: SessionStats) => (
    session.tool !== 'reasonix' || hasReasonixCoverageData(session.coveredRequests, session.uncoveredRequests, session.usageFullyCovered)
  )
  const projectUsageVisible = (project: ProjectStats) => (
    !project.toolBreakdown?.some(tool => tool.tool === 'reasonix')
    || hasReasonixCoverageData(project.coveredRequests, project.uncoveredRequests, project.usageFullyCovered)
  )
  const projectToolUsageVisible = (tool: ProjectToolStats) => (
    tool.tool !== 'reasonix'
    || hasReasonixCoverageData(tool.coveredRequests, tool.uncoveredRequests, tool.usageFullyCovered)
  )
  const sessionHasPartialCoverage = (session: SessionStats) => (
    session.tool === 'reasonix' && uncoveredRequests(session.uncoveredRequests) > 0
  )
  const displayTokens = (value: number, visible: boolean) => (visible ? formatTokens(value) : '—')
  const displayCost = (value: number | undefined, visible: boolean) => (visible ? formatCost(value) : '—')
  const displayRequestValue = (value: number) => formatRequestCount(value)
  const displaySessionCacheHitRate = (session: SessionStats) => (
    sessionUsageVisible(session) ? sessionCacheHitRate(session) : '—'
  )
  const displaySessionPrimaryLabel = (session: SessionStats) => (
    sessionHasPartialCoverage(session)
      ? t(store.settings.locale, 'common.covered')
      : t(store.settings.locale, 'common.totalTokens')
  )
  const displaySessionPrimaryValue = (session: SessionStats) => (
    sessionHasPartialCoverage(session)
      ? displayRequestValue(coveredRequests(session.coveredRequests))
      : displayTokens(
        (session.totalInputTokens || 0)
          + (session.totalOutputTokens || 0)
          + (session.totalCacheCreateTokens || 0)
          + (session.totalCacheReadTokens || 0),
        sessionUsageVisible(session)
      )
  )
  const displayProjectCoverageHint = (project: ProjectStats) => (
    uncoveredRequests(project.uncoveredRequests) > 0
      ? t(store.settings.locale, 'sessions.uncoveredRequests', { count: uncoveredRequests(project.uncoveredRequests) })
      : ''
  )
  const displayToolCoverageHint = (tool: ProjectToolStats) => (
    uncoveredRequests(tool.uncoveredRequests) > 0
      ? t(store.settings.locale, 'sessions.uncoveredRequests', { count: uncoveredRequests(tool.uncoveredRequests) })
      : ''
  )

  const displaySessionTitle = (session: SessionStats) => {
    const sessionName = session.sessionName?.trim()
    if (session.topic?.trim()) return session.topic
    if (sessionName && !uuidLikePattern.test(sessionName)) return sessionName
    if (session.lastPrompt?.trim()) return session.lastPrompt
    if (session.projectName?.trim()) return session.projectName
    return t(store.settings.locale, 'sessions.untitled')
  }

  const displaySessionProjectBadge = (session: SessionStats) => {
    if (session.projectName?.trim()) return session.projectName
    if (session.projectIdentity === 'global') return t(store.settings.locale, 'common.global')
    if (session.projectIdentity === 'unknown') return t(store.settings.locale, 'common.unknownProject')
    return ''
  }

  const projectBadgeClasses = (identity?: string) => {
    if (identity === 'global') {
      return 'bg-slate-50 text-slate-500 dark:bg-slate-500/15 dark:text-slate-300 border border-slate-100 dark:border-slate-400/20'
    }
    if (identity === 'unknown') {
      return 'bg-amber-50 text-amber-600 dark:bg-amber-500/15 dark:text-amber-300 border border-amber-100 dark:border-amber-400/20'
    }
    return 'bg-indigo-50 text-indigo-500 dark:bg-indigo-500/20 dark:text-indigo-300 border border-indigo-100 dark:border-indigo-500/30'
  }

  const displayProjectName = (project: ProjectStats) => {
    if (project.projectIdentity === 'global') return t(store.settings.locale, 'common.global')
    if (project.projectIdentity === 'unknown') return t(store.settings.locale, 'common.unknownProject')
    return project.name
  }

  const displayProjectHint = (project: ProjectStats) => {
    if (project.projectIdentity === 'global') return t(store.settings.locale, 'sessions.globalSessionHint')
    if (project.projectIdentity === 'unknown') return t(store.settings.locale, 'sessions.unknownProjectHint')
    return ''
  }

  const projectWslDistros = (project: ProjectStats) => {
    const distros = project.wslDistros?.filter(distro => distro.trim()) || []
    if (distros.length > 0) return distros
    return project.wslDistro ? [project.wslDistro] : []
  }

  const displayProjectWslBadge = (project: ProjectStats) => {
    const distros = projectWslDistros(project)
    if (distros.length === 0) return ''
    if (distros.length === 1) return distros[0]
    return `${distros[0]} +${distros.length - 1}`
  }

  const displayProjectWslTitle = (project: ProjectStats) => {
    const distros = projectWslDistros(project)
    if (distros.length === 0) return ''
    return t(store.settings.locale, 'sessions.wslBadgeTitle', { distro: distros.join(', ') })
  }

  const displayProxyTokenValue = (session: SessionStats) => (
    coveredRequests(session.coveredRequests) > 0
      ? formatTokens(
        (session.totalInputTokens || 0)
          + (session.totalOutputTokens || 0)
          + (session.totalCacheCreateTokens || 0)
          + (session.totalCacheReadTokens || 0)
      )
      : '—'
  )

  const displayProxyRateValue = (session: SessionStats) => (
    coveredRequests(session.coveredRequests) > 0 && (session.avgOutputTokensPerSecond || 0) > 0
      ? `${session.avgOutputTokensPerSecond.toFixed(1)}t/s`
      : '—'
  )

  const getToolProfile = (tool: string) => getToolProfileByTool(store.settings.clientTools.profiles, tool)

  const getToolIcon = (tool: string) => {
    const profile = getToolProfile(tool)
    return profile?.icon || TOOL_LOBE_ICONS[tool] || TOOL_LOBE_ICONS[getFamilyHead(tool)] || null
  }

  const projectToolRows = (project: ProjectStats) => {
    const rows = project.toolBreakdown?.length
      ? project.toolBreakdown
      : [{
          tool: 'unknown',
          requestCount: project.requestCount,
          sessionCount: project.sessionCount,
          totalInputTokens: project.totalInputTokens,
          totalOutputTokens: project.totalOutputTokens,
          totalCacheCreateTokens: project.totalCacheCreateTokens,
          totalCacheReadTokens: project.totalCacheReadTokens,
          totalCost: project.totalCost,
          lastActive: project.lastActive
        }]

    return [...rows].sort((a, b) => b.lastActive - a.lastActive)
  }

  const projectTotalTokens = (project: ProjectStats) => (
    project.totalInputTokens
    + project.totalOutputTokens
    + project.totalCacheCreateTokens
    + project.totalCacheReadTokens
  )

  const shouldShowProjectTotalRow = (project: ProjectStats) => projectToolRows(project).length > 1

  return {
    formatTime,
    formatTokens,
    formatCost,
    formatDuration,
    requestModelLabel,
    sessionModelLabel,
    requestStatusLabel,
    requestStatusClasses,
    requestCoverageLabel,
    requestProjectLabel,
    requestSourceLabel,
    requestAttributionLabel,
    requestToolLabel,
    requestCacheTokens,
    requestHasProxyPerformance,
    localRequests,
    coveredRequests,
    uncoveredRequests,
    sessionUsageVisible,
    projectUsageVisible,
    projectToolUsageVisible,
    sessionHasPartialCoverage,
    displayTokens,
    displayCost,
    displayRequestValue,
    displaySessionCacheHitRate,
    displaySessionPrimaryLabel,
    displaySessionPrimaryValue,
    displayProjectCoverageHint,
    displayToolCoverageHint,
    displaySessionTitle,
    displaySessionProjectBadge,
    projectBadgeClasses,
    displayProjectName,
    displayProjectHint,
    displayProjectWslBadge,
    displayProjectWslTitle,
    displayProxyTokenValue,
    displayProxyRateValue,
    getToolIcon,
    projectToolRows,
    projectTotalTokens,
    shouldShowProjectTotalRow,
  }
}
