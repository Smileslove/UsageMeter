import { describe, it, expect } from 'vitest'
import type {
  AppSettings,
  ConfiguredSourceQuotaQueryResult,
  OverviewDeferredBundle,
  SubscriptionQuota,
  UsageSnapshot,
  WindowUsage,
} from '../types'
import {
  CONFIGURED_SOURCE_FAILURE_BASE_MS,
  CONFIGURED_SOURCE_FAILURE_MAX_MS,
  CONFIGURED_SOURCE_SUCCESS_REFRESH_MS,
} from './monitorDefaults'
import {
  applyConfiguredSourceQuotaFailure,
  applyConfiguredSourceQuotaResult,
  createEmptyRateSummary,
  mergeDeferredOverviewSnapshot,
  normalizeSettings,
  type ConfiguredSourceQuotaState,
} from './monitorActionHelpers'

const BASE = CONFIGURED_SOURCE_FAILURE_BASE_MS // 2 分钟
const MAX = CONFIGURED_SOURCE_FAILURE_MAX_MS // 30 分钟
const SUCCESS = CONFIGURED_SOURCE_SUCCESS_REFRESH_MS // 5 分钟

function makeState(partial: Partial<ConfiguredSourceQuotaState> = {}): ConfiguredSourceQuotaState {
  return {
    configuredSourceQuotas: [],
    configuredSourceLastSuccessAt: null,
    configuredSourceLastAttemptAt: null,
    configuredSourceLastFailureAt: null,
    configuredSourceConsecutiveFailures: 0,
    configuredSourceNextEligibleAt: 0,
    configuredSourceLastError: null,
    ...partial,
  }
}

function makeResult(partial: Partial<ConfiguredSourceQuotaQueryResult> & { queriedAt: number }): ConfiguredSourceQuotaQueryResult {
  return {
    quotas: [],
    attemptedCount: 0,
    successCount: 0,
    failedCount: 0,
    errors: [],
    ...partial,
  }
}

function allFailed(queriedAt: number, errors: string[] = ['a', 'b']): ConfiguredSourceQuotaQueryResult {
  // attemptedCount 至少为 1，避免空 errors 误入 attemptedCount === 0 分支
  return makeResult({ queriedAt, attemptedCount: Math.max(1, errors.length), successCount: 0, failedCount: errors.length, errors })
}

describe('applyConfiguredSourceQuotaResult backoff', () => {
  it('backs_off_exponentially_on_consecutive_failures', () => {
    let state = makeState()
    const deltas: number[] = []
    for (let i = 1; i <= 5; i++) {
      const next = applyConfiguredSourceQuotaResult(state, allFailed(i * 1000))
      deltas.push(next.configuredSourceNextEligibleAt - i * 1000)
      expect(next.configuredSourceConsecutiveFailures).toBe(i)
      state = next
    }
    // 2 / 4 / 8 / 16 / 30 分钟（第 5 次起 32 分钟被封顶到 30）
    expect(deltas).toEqual([BASE, BASE * 2, BASE * 4, BASE * 8, MAX])
  })

  it('caps_backoff_at_30_minutes_for_further_failures', () => {
    const state = makeState({ configuredSourceConsecutiveFailures: 5 })
    const next = applyConfiguredSourceQuotaResult(state, allFailed(1000))
    expect(next.configuredSourceConsecutiveFailures).toBe(6)
    expect(next.configuredSourceNextEligibleAt - 1000).toBe(MAX)
  })

  it('records_failure_time_and_joined_errors', () => {
    const next = applyConfiguredSourceQuotaResult(makeState(), allFailed(1234, ['e1', 'e2', 'e3']))
    expect(next.configuredSourceLastFailureAt).toBe(1234)
    expect(next.configuredSourceLastError).toBe('e1 | e2 | e3')
    expect(next.configuredSourceNextEligibleAt).toBe(1234 + BASE)
  })

  it('uses_fallback_error_when_errors_list_is_empty', () => {
    const next = applyConfiguredSourceQuotaResult(makeState(), allFailed(1000, []))
    expect(next.configuredSourceLastError).toBe('ERR_CONFIGURED_SOURCE_QUOTA_FAILED')
  })

  it('resets_failure_state_after_partial_or_full_success', () => {
    const prev = makeState({
      configuredSourceConsecutiveFailures: 3,
      configuredSourceLastFailureAt: 999,
      configuredSourceLastError: 'old',
    })
    const success = makeResult({ queriedAt: 2000, attemptedCount: 2, successCount: 2, failedCount: 0 })
    const next = applyConfiguredSourceQuotaResult(prev, success)
    expect(next.configuredSourceConsecutiveFailures).toBe(0)
    expect(next.configuredSourceLastFailureAt).toBeNull()
    expect(next.configuredSourceLastError).toBeNull()
    expect(next.configuredSourceLastSuccessAt).toBe(2000)
    expect(next.configuredSourceNextEligibleAt).toBe(2000 + SUCCESS)
  })

  it('keeps_quotas_but_records_errors_on_partial_success', () => {
    const quotas = [{ source: 'claude' } as unknown as SubscriptionQuota]
    const result = makeResult({ queriedAt: 2000, quotas, attemptedCount: 3, successCount: 1, failedCount: 2, errors: ['x', 'y'] })
    const next = applyConfiguredSourceQuotaResult(makeState(), result)
    expect(next.configuredSourceQuotas).toBe(quotas)
    expect(next.configuredSourceLastError).toBe('x | y')
    expect(next.configuredSourceNextEligibleAt).toBe(2000 + SUCCESS)
  })

  it('clears_quotas_and_failure_state_when_attemptedCount_is_zero', () => {
    const prev = makeState({
      configuredSourceQuotas: [{ source: 'codex' } as unknown as SubscriptionQuota],
      configuredSourceConsecutiveFailures: 4,
      configuredSourceLastError: 'old',
      configuredSourceLastFailureAt: 1,
    })
    const next = applyConfiguredSourceQuotaResult(prev, makeResult({ queriedAt: 3000, attemptedCount: 0 }))
    expect(next.configuredSourceQuotas).toEqual([])
    expect(next.configuredSourceConsecutiveFailures).toBe(0)
    expect(next.configuredSourceLastError).toBeNull()
    expect(next.configuredSourceLastFailureAt).toBeNull()
    expect(next.configuredSourceLastSuccessAt).toBe(3000)
    expect(next.configuredSourceNextEligibleAt).toBe(3000 + SUCCESS)
  })

  it('does_not_mutate_input_state', () => {
    const state = makeState({ configuredSourceConsecutiveFailures: 2 })
    const snapshot = { ...state }
    applyConfiguredSourceQuotaResult(state, allFailed(1000))
    expect(state).toEqual(snapshot)
  })
})

describe('applyConfiguredSourceQuotaFailure', () => {
  it('increments_failures_and_backs_off_from_error_timestamp', () => {
    const next = applyConfiguredSourceQuotaFailure(makeState({ configuredSourceConsecutiveFailures: 1 }), 'boom', 5000)
    expect(next.configuredSourceConsecutiveFailures).toBe(2)
    expect(next.configuredSourceLastFailureAt).toBe(5000)
    expect(next.configuredSourceLastError).toBe('boom')
    expect(next.configuredSourceNextEligibleAt).toBe(5000 + BASE * 2)
  })

  it('stringifies_error_values', () => {
    expect(applyConfiguredSourceQuotaFailure(makeState(), new Error('boom'), 0).configuredSourceLastError).toBe('Error: boom')
    expect(applyConfiguredSourceQuotaFailure(makeState(), { code: 1 }, 0).configuredSourceLastError).toBe('[object Object]')
  })
})

describe('normalizeSettings', () => {
  it('returns_same_reference_for_chinese_format', () => {
    const settings = { numberFormat: 'chinese', cursor: { accountSyncEnabled: false, databasePath: null } } as AppSettings
    expect(normalizeSettings(settings)).toBe(settings)
  })

  it('normalizes_everything_else_to_international_without_mutation', () => {
    const settings = { numberFormat: 'international', refreshIntervalSeconds: 30 } as AppSettings
    const result = normalizeSettings(settings)
    expect(result).not.toBe(settings)
    expect(result.numberFormat).toBe('international')
    expect(result.refreshIntervalSeconds).toBe(30)
    expect(settings).toEqual({ numberFormat: 'international', refreshIntervalSeconds: 30 })
  })

  it('migrates_old_cursor_settings_without_enabling_account_network', () => {
    const old = { numberFormat: 'chinese' } as AppSettings
    expect(normalizeSettings(old).cursor).toEqual({ accountSyncEnabled: false, databasePath: null })
    expect(old.cursor).toBeUndefined()
  })

  it('normalizes_missing_numberFormat_to_international', () => {
    const result = normalizeSettings({ refreshIntervalSeconds: 30 } as AppSettings)
    expect(result.numberFormat).toBe('international')
  })
})

describe('mergeDeferredOverviewSnapshot', () => {
  const makeSnapshot = (windows: Array<{ window: string }>): UsageSnapshot =>
    ({
      generatedAtEpoch: 1,
      windows,
      source: 'local-files',
      summary: { marker: 'old-summary' },
      modelDistribution: [{ marker: 'old-model' }],
    }) as unknown as UsageSnapshot

  it('returns_null_when_snapshot_is_null', () => {
    expect(mergeDeferredOverviewSnapshot(null, {} as OverviewDeferredBundle)).toBeNull()
  })

  it('keeps_global_summary_and_model_distribution_when_merging_window', () => {
    const snapshot = makeSnapshot([{ window: '24h' }, { window: '7d' }])
    const windowUsage = { window: '24h', marker: 'new' } as unknown as WindowUsage
    const bundle = {
      windowUsage,
      usageSummary: { marker: 'new-summary' },
      modelDistribution: [{ marker: 'new-model' }],
    } as unknown as OverviewDeferredBundle

    const result = mergeDeferredOverviewSnapshot(snapshot, bundle)
    expect(result).not.toBeNull()
    const windows = result!.windows as unknown as Array<{ window: string; marker: string }>
    expect(windows).toHaveLength(2)
    expect(windows.map(w => w.window)).toEqual(['7d', '24h'])
    expect(windows.find(w => w.window === '24h')).toBe(windowUsage)
    // 全局口径不被单窗口覆盖：当前窗口无数据时概览不应被误判为空
    expect((result as unknown as { summary: unknown }).summary).toBe(snapshot.summary)
    expect((result as unknown as { modelDistribution: unknown }).modelDistribution).toBe(
      snapshot.modelDistribution
    )
  })

  it('does_not_mutate_original_snapshot', () => {
    const snapshot = makeSnapshot([{ window: '24h' }])
    const bundle = {
      windowUsage: { window: '24h' },
      usageSummary: {},
      modelDistribution: [],
    } as unknown as OverviewDeferredBundle

    const result = mergeDeferredOverviewSnapshot(snapshot, bundle)
    expect(result).not.toBe(snapshot)
    expect(snapshot.windows).toHaveLength(1)
    expect(snapshot.windows[0]).toEqual({ window: '24h' })
    expect(snapshot.summary).toEqual({ marker: 'old-summary' })
  })
})

describe('createEmptyRateSummary', () => {
  it('returns_zeroed_summary_for_window', () => {
    const summary = createEmptyRateSummary('5h')
    expect(summary.window).toBe('5h')
    expect(summary.overall).toEqual({ requestCount: 0, totalOutputTokens: 0, totalDurationMs: 0, avgTokensPerSecond: 0 })
    expect(summary.byModel).toEqual([])
    expect(summary.ttft).toEqual({ requestCount: 0, avgTtftMs: 0, minTtftMs: 0, maxTtftMs: 0 })
    expect(summary.ttftByModel).toEqual([])
  })
})
