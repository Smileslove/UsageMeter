import type {
  AppSettings,
  ConfiguredSourceQuotaQueryResult,
  OverviewDeferredBundle,
  ProxyStatus,
  SubscriptionQueryResult,
  UsageSnapshot,
  WindowRateSummary
} from '../types'
import {
  CONFIGURED_SOURCE_FAILURE_BASE_MS,
  CONFIGURED_SOURCE_FAILURE_MAX_MS,
  CONFIGURED_SOURCE_SUCCESS_REFRESH_MS
} from './monitorDefaults'
import { failedSubscriptionQuery } from './subscriptionErrors'

/** Normalize persisted values before they are consumed by settings UI or back-end calls. */
export function normalizeSettings(settings: AppSettings): AppSettings {
  return settings.numberFormat === 'chinese'
    ? settings
    : { ...settings, numberFormat: 'international' }
}

/** Merge the deferred overview response without mutating the current snapshot. */
export function mergeDeferredOverviewSnapshot(
  snapshot: UsageSnapshot | null,
  bundle: OverviewDeferredBundle
): UsageSnapshot | null {
  if (!snapshot) return snapshot
  const existingWindows = snapshot.windows.filter(item => item.window !== bundle.windowUsage.window)
  return {
    ...snapshot,
    windows: [...existingWindows, bundle.windowUsage],
    summary: bundle.usageSummary,
    modelDistribution: bundle.modelDistribution
  }
}

/** Stable empty value used when the deferred overview request fails. */
export function createEmptyRateSummary(window: string): WindowRateSummary {
  return {
    window,
    overall: { requestCount: 0, totalOutputTokens: 0, totalDurationMs: 0, avgTokensPerSecond: 0 },
    byModel: [],
    ttft: { requestCount: 0, avgTtftMs: 0, minTtftMs: 0, maxTtftMs: 0 },
    ttftByModel: []
  }
}

export function createUnavailableProxyStatus(): ProxyStatus {
  return {
    running: false,
    port: 0,
    uptimeSeconds: 0,
    totalRequests: 0,
    successRequests: 0,
    failedRequests: 0,
    activeConnections: 0,
    configTakenOver: false,
    recordCount: 0,
    status2xx: 0,
    status4xx: 0,
    status5xx: 0
  }
}

export interface ConfiguredSourceQuotaState {
  configuredSourceQuotas: ConfiguredSourceQuotaQueryResult['quotas']
  configuredSourceLastSuccessAt: number | null
  configuredSourceLastAttemptAt: number | null
  configuredSourceLastFailureAt: number | null
  configuredSourceConsecutiveFailures: number
  configuredSourceNextEligibleAt: number
  configuredSourceLastError: string | null
}

/** Apply one configured-source result and calculate the next retry window. */
export function applyConfiguredSourceQuotaResult(
  state: ConfiguredSourceQuotaState,
  result: ConfiguredSourceQuotaQueryResult
): ConfiguredSourceQuotaState {
  const next = { ...state }
  if (result.attemptedCount === 0) {
    next.configuredSourceQuotas = []
    next.configuredSourceLastSuccessAt = result.queriedAt
    next.configuredSourceLastError = null
    next.configuredSourceConsecutiveFailures = 0
    next.configuredSourceLastFailureAt = null
    next.configuredSourceNextEligibleAt = result.queriedAt + CONFIGURED_SOURCE_SUCCESS_REFRESH_MS
  } else if (result.successCount > 0) {
    next.configuredSourceQuotas = result.quotas
    next.configuredSourceLastSuccessAt = result.queriedAt
    next.configuredSourceConsecutiveFailures = 0
    next.configuredSourceLastFailureAt = null
    next.configuredSourceLastError = result.failedCount > 0 ? result.errors.join(' | ') : null
    next.configuredSourceNextEligibleAt = result.queriedAt + CONFIGURED_SOURCE_SUCCESS_REFRESH_MS
  } else {
    next.configuredSourceConsecutiveFailures += 1
    next.configuredSourceLastFailureAt = result.queriedAt
    next.configuredSourceLastError = result.errors.join(' | ') || 'ERR_CONFIGURED_SOURCE_QUOTA_FAILED'
    const backoffMs = Math.min(
      CONFIGURED_SOURCE_FAILURE_BASE_MS * (2 ** (next.configuredSourceConsecutiveFailures - 1)),
      CONFIGURED_SOURCE_FAILURE_MAX_MS
    )
    next.configuredSourceNextEligibleAt = result.queriedAt + backoffMs
  }
  return next
}

export function applyConfiguredSourceQuotaFailure(
  state: ConfiguredSourceQuotaState,
  error: unknown,
  now: number
): ConfiguredSourceQuotaState {
  const next = { ...state }
  next.configuredSourceConsecutiveFailures += 1
  next.configuredSourceLastFailureAt = now
  next.configuredSourceLastError = String(error)
  const backoffMs = Math.min(
    CONFIGURED_SOURCE_FAILURE_BASE_MS * (2 ** (next.configuredSourceConsecutiveFailures - 1)),
    CONFIGURED_SOURCE_FAILURE_MAX_MS
  )
  next.configuredSourceNextEligibleAt = now + backoffMs
  return next
}

/** Shared loading/error behavior for provider subscription quota actions. */
export async function runSubscriptionQuotaQuery(
  query: () => Promise<SubscriptionQueryResult>,
  setLoading: (loading: boolean) => void,
  setResult: (result: SubscriptionQueryResult) => void,
  onError?: (error: unknown) => void
): Promise<void> {
  setLoading(true)
  try {
    setResult(await query())
  } catch (error) {
    onError?.(error)
    setResult(failedSubscriptionQuery(error))
  } finally {
    setLoading(false)
  }
}
