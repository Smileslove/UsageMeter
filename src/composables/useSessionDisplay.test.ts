import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import type { ProjectStats, ProjectToolStats, SessionStats } from '../types'
import type { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { useSessionDisplay } from './useSessionDisplay'

// useMonitorStore 仅作类型使用（type-only import，不会加载 Pinia/Tauri 模块），
// 这里用一个最小 fake store 对象驱动纯展示函数。
function makeStore(locale = 'zh-CN'): ReturnType<typeof useMonitorStore> {
  return {
    settings: {
      locale,
      currency: {
        displayCurrency: 'USD',
        exchangeRates: { USD: 1.0 },
        trackedCurrencies: ['USD'],
        lastRateUpdate: null,
      },
      clientTools: { profiles: [] },
    },
  } as unknown as ReturnType<typeof useMonitorStore>
}

function session(overrides: Partial<SessionStats> = {}): SessionStats {
  return {
    sessionId: 's1',
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
    ...overrides,
  } as SessionStats
}

function project(overrides: Partial<ProjectStats>): ProjectStats {
  return {
    name: 'p',
    requestCount: 0,
    sessionCount: 0,
    totalInputTokens: 0,
    totalOutputTokens: 0,
    totalCacheCreateTokens: 0,
    totalCacheReadTokens: 0,
    totalCost: 0,
    lastActive: 0,
    toolBreakdown: [],
    ...overrides,
  } as ProjectStats
}

describe('formatTime', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2025-06-15T12:00:00Z'))
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('returns_dash_for_falsy_epoch', () => {
    const { formatTime } = useSessionDisplay(makeStore())
    expect(formatTime(0)).toBe('-')
    expect(formatTime(undefined as unknown as number)).toBe('-')
  })

  it('shows_just_now_under_one_minute', () => {
    const { formatTime } = useSessionDisplay(makeStore())
    expect(formatTime(Math.floor(Date.now() / 1000) - 30)).toBe(t('zh-CN', 'common.justNow'))
  })

  it('shows_minutes_ago_below_one_hour', () => {
    const { formatTime } = useSessionDisplay(makeStore())
    expect(formatTime(Math.floor(Date.now() / 1000) - 5 * 60)).toBe('5分钟前')
  })

  it('shows_hours_ago_below_24_hours', () => {
    const { formatTime } = useSessionDisplay(makeStore())
    expect(formatTime(Math.floor(Date.now() / 1000) - 2 * 3600)).toBe('2小时前')
    expect(formatTime(Math.floor(Date.now() / 1000) - 23 * 3600)).toBe('23小时前')
  })

  it('falls_back_to_absolute_date_after_24_hours', () => {
    const { formatTime } = useSessionDisplay(makeStore())
    const result = formatTime(Math.floor(Date.now() / 1000) - 2 * 86400)
    expect(result).not.toBe('-')
    expect(result).not.toMatch(/刚刚|分钟前|小时前/)
    expect(result).toMatch(/\d/)
  })

  it('includes_year_for_previous_calendar_year', () => {
    const { formatTime } = useSessionDisplay(makeStore())
    const result = formatTime(Math.floor(Date.now() / 1000) - 400 * 86400)
    expect(result).toContain('2024')
  })
})

describe('formatDuration', () => {
  it('returns_em_dash_for_falsy_input', () => {
    const { formatDuration } = useSessionDisplay(makeStore())
    expect(formatDuration(undefined)).toBe('—')
    expect(formatDuration(null)).toBe('—')
    expect(formatDuration(0)).toBe('—')
  })

  it('formats_milliseconds_seconds_and_minutes', () => {
    const { formatDuration } = useSessionDisplay(makeStore())
    expect(formatDuration(999)).toBe('999ms')
    expect(formatDuration(1000)).toBe('1.0s')
    expect(formatDuration(30000)).toBe('30.0s')
    expect(formatDuration(60000)).toBe('1m 0s')
    expect(formatDuration(61000)).toBe('1m 1s')
    expect(formatDuration(125000)).toBe('2m 5s')
    expect(formatDuration(3599000)).toBe('59m 59s')
  })
})

describe('sessionCacheHitRate (via displaySessionCacheHitRate)', () => {
  it('returns_em_dash_on_zero_total', () => {
    // sessionCacheHitRate 未从 useSessionDisplay 导出，经 displaySessionCacheHitRate 间接覆盖
    const { displaySessionCacheHitRate } = useSessionDisplay(makeStore())
    expect(displaySessionCacheHitRate(session())).toBe('—')
  })

  it('computes_cache_read_share_percent', () => {
    const { displaySessionCacheHitRate } = useSessionDisplay(makeStore())
    // 分母 = input + cacheCreate + cacheRead（含 cacheRead 本身）
    expect(displaySessionCacheHitRate(session({ totalInputTokens: 100, totalCacheReadTokens: 30 }))).toBe('23.1%')
    expect(displaySessionCacheHitRate(session({ totalInputTokens: 0, totalCacheCreateTokens: 10, totalCacheReadTokens: 10 }))).toBe('50.0%')
    expect(displaySessionCacheHitRate(session({ totalInputTokens: 100, totalCacheReadTokens: 0 }))).toBe('0.0%')
  })
})

describe('reasonix coverage visibility rules', () => {
  it('session_usage_visible_true_for_non_reasonix_tools', () => {
    const { sessionUsageVisible } = useSessionDisplay(makeStore())
    expect(sessionUsageVisible(session({ tool: 'claude_code' }))).toBe(true)
  })

  it('session_usage_hidden_for_reasonix_without_coverage_data', () => {
    const { sessionUsageVisible } = useSessionDisplay(makeStore())
    expect(sessionUsageVisible(session({ tool: 'reasonix' }))).toBe(false)
    expect(sessionUsageVisible(session({ tool: 'reasonix', usageFullyCovered: true }))).toBe(false)
  })

  it('session_usage_visible_when_any_coverage_signal_present', () => {
    const { sessionUsageVisible } = useSessionDisplay(makeStore())
    expect(sessionUsageVisible(session({ tool: 'reasonix', coveredRequests: 1 }))).toBe(true)
    expect(sessionUsageVisible(session({ tool: 'reasonix', uncoveredRequests: 1 }))).toBe(true)
    expect(sessionUsageVisible(session({ tool: 'reasonix', usageFullyCovered: false }))).toBe(true)
  })

  it('project_usage_visibility_follows_reasonix_presence', () => {
    const { projectUsageVisible } = useSessionDisplay(makeStore())
    expect(projectUsageVisible(project({ toolBreakdown: [{ tool: 'codex' } as ProjectToolStats] }))).toBe(true)
    expect(projectUsageVisible(project({ toolBreakdown: [{ tool: 'reasonix' } as ProjectToolStats] }))).toBe(false)
    expect(projectUsageVisible(project({ toolBreakdown: [{ tool: 'reasonix' } as ProjectToolStats], coveredRequests: 2 }))).toBe(true)
    expect(projectUsageVisible(project({ toolBreakdown: [{ tool: 'reasonix' } as ProjectToolStats], usageFullyCovered: false }))).toBe(true)
  })

  it('tool_usage_visibility_by_tool_and_coverage', () => {
    const { projectToolUsageVisible } = useSessionDisplay(makeStore())
    expect(projectToolUsageVisible({ tool: 'claude_code' } as ProjectToolStats)).toBe(true)
    expect(projectToolUsageVisible({ tool: 'reasonix' } as ProjectToolStats)).toBe(false)
    expect(projectToolUsageVisible({ tool: 'reasonix', uncoveredRequests: 1 } as ProjectToolStats)).toBe(true)
  })

  it('partial_coverage_detected_only_for_reasonix_with_uncovered', () => {
    const { sessionHasPartialCoverage } = useSessionDisplay(makeStore())
    expect(sessionHasPartialCoverage(session({ tool: 'reasonix', uncoveredRequests: 3 }))).toBe(true)
    expect(sessionHasPartialCoverage(session({ tool: 'reasonix', uncoveredRequests: 0 }))).toBe(false)
    expect(sessionHasPartialCoverage(session({ tool: 'claude_code', uncoveredRequests: 3 }))).toBe(false)
  })

  it('session_cache_hit_rate_hidden_for_invisible_reasonix_session', () => {
    const { displaySessionCacheHitRate } = useSessionDisplay(makeStore())
    expect(displaySessionCacheHitRate(session({ tool: 'reasonix' }))).toBe('—')
    expect(displaySessionCacheHitRate(session({ tool: 'reasonix', coveredRequests: 1, totalInputTokens: 100, totalCacheReadTokens: 30 }))).toBe('23.1%')
  })
})

describe('displaySessionTitle priority chain', () => {
  it('prefers_topic_over_everything', () => {
    const { displaySessionTitle } = useSessionDisplay(makeStore())
    expect(displaySessionTitle(session({ topic: 'Fix login', sessionName: 'My Session', lastPrompt: 'hi' }))).toBe('Fix login')
  })

  it('falls_back_to_session_name_when_not_uuid_like', () => {
    const { displaySessionTitle } = useSessionDisplay(makeStore())
    expect(displaySessionTitle(session({ sessionName: 'My Session', lastPrompt: 'hi', projectName: 'p' }))).toBe('My Session')
    expect(displaySessionTitle(session({ sessionName: '  My Session  ' }))).toBe('My Session')
  })

  it('skips_uuid_like_session_names', () => {
    const { displaySessionTitle } = useSessionDisplay(makeStore())
    expect(
      displaySessionTitle(session({ sessionName: '550e8400-e29b-41d4-a716-446655440000', lastPrompt: 'what is the weather' }))
    ).toBe('what is the weather')
  })

  it('falls_back_to_last_prompt_then_project_name_then_untitled', () => {
    const { displaySessionTitle } = useSessionDisplay(makeStore())
    expect(displaySessionTitle(session({ lastPrompt: 'hello', projectName: 'proj' }))).toBe('hello')
    expect(displaySessionTitle(session({ projectName: 'proj' }))).toBe('proj')
    expect(displaySessionTitle(session({}))).toBe(t('zh-CN', 'sessions.untitled'))
  })
})

describe('displayProxyRateValue / coveredRequests helpers', () => {
  it('hides_rate_without_covered_requests_or_zero_avg', () => {
    const { displayProxyRateValue } = useSessionDisplay(makeStore())
    expect(displayProxyRateValue(session())).toBe('—')
    expect(displayProxyRateValue(session({ coveredRequests: 2, avgOutputTokensPerSecond: 0 }))).toBe('—')
    expect(displayProxyRateValue(session({ coveredRequests: 2, avgOutputTokensPerSecond: 1.234 }))).toBe('1.2t/s')
  })

  it('coerces_undefined_coverage_counts_to_zero', () => {
    const { coveredRequests, uncoveredRequests, localRequests } = useSessionDisplay(makeStore())
    expect(coveredRequests(undefined)).toBe(0)
    expect(coveredRequests(5)).toBe(5)
    expect(uncoveredRequests(undefined)).toBe(0)
    expect(localRequests(session({ coveredRequests: 3, uncoveredRequests: 2 }))).toBe(5)
  })
})

describe('displayTokens / displayCost visibility wrappers', () => {
  it('masks_values_when_not_visible', () => {
    const { displayTokens, displayCost } = useSessionDisplay(makeStore())
    expect(displayTokens(100, false)).toBe('—')
    expect(displayCost(1.2, false)).toBe('—')
  })

  it('formats_visible_values', () => {
    const { displayTokens, displayCost } = useSessionDisplay(makeStore())
    expect(displayTokens(100, true)).toBe('100')
    expect(displayCost(1.2, true)).toBe('$1.2000')
    expect(displayCost(undefined, true)).toBe('-')
  })
})

describe('displaySessionProjectBadge', () => {
  it('prefers_project_name_then_identity_labels', () => {
    const { displaySessionProjectBadge } = useSessionDisplay(makeStore())
    expect(displaySessionProjectBadge(session({ projectName: 'proj', projectIdentity: 'project' }))).toBe('proj')
    expect(displaySessionProjectBadge(session({ projectIdentity: 'global' }))).toBe(t('zh-CN', 'common.global'))
    expect(displaySessionProjectBadge(session({ projectIdentity: 'unknown' }))).toBe(t('zh-CN', 'common.unknownProject'))
    expect(displaySessionProjectBadge(session({}))).toBe('')
  })
})

describe('displaySessionPrimaryValue', () => {
  it('shows_covered_request_count_for_partially_covered_reasonix', () => {
    const { displaySessionPrimaryValue } = useSessionDisplay(makeStore())
    expect(displaySessionPrimaryValue(session({ tool: 'reasonix', coveredRequests: 3, uncoveredRequests: 1 }))).toBe('3')
  })

  it('shows_token_total_for_normal_sessions', () => {
    const { displaySessionPrimaryValue } = useSessionDisplay(makeStore())
    expect(displaySessionPrimaryValue(session({ totalInputTokens: 100, totalOutputTokens: 50 }))).toBe('150')
  })
})

describe('formatTokens', () => {
  it('returns_zero_for_falsy_and_formats_otherwise', () => {
    const { formatTokens } = useSessionDisplay(makeStore())
    expect(formatTokens(0)).toBe('0')
    expect(formatTokens(1200)).toBe('1.20K')
  })
})

describe('DeepSeek Harness account attribution', () => {
  it.each(['zh-CN', 'zh-TW', 'en-US'])('labels recorded provider evidence in %s', (locale) => {
    const request = {
      coverageOrigin: 'local_only',
      attributionMethod: 'provider_reported',
      sourceLabel: '__deepseek_harness_account__',
    } as import('../types').RequestRecord
    const display = useSessionDisplay(makeStore(locale))
    expect(display.requestAttributionLabel(request)).toBe(t(locale, 'sessions.requestAttributionProvider'))
    expect(display.requestSourceLabel(request)).toBe(t(locale, 'sources.deepseekHarnessAccount'))
  })
})
