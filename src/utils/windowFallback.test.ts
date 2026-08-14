import { describe, expect, it } from 'vitest'
import type { WindowUsage } from '../types'
import { FALLBACK_WINDOW_ORDER, hasAnyUsage, pickEffectiveWindow } from './windowFallback'

function makeWindow(window: string, usage: Partial<WindowUsage> = {}): WindowUsage {
  return {
    window,
    tokenUsed: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheCreateTokens: 0,
    cacheReadTokens: 0,
    requestUsed: 0,
    cost: 0,
    successRequests: 0,
    clientErrorRequests: 0,
    serverErrorRequests: 0,
    ...usage
  }
}

const withRequests = (requests: number): Partial<WindowUsage> => ({ requestUsed: requests })

describe('hasAnyUsage', () => {
  it('returns_false_for_null_or_empty', () => {
    expect(hasAnyUsage(null)).toBe(false)
    expect(hasAnyUsage(undefined)).toBe(false)
    expect(hasAnyUsage(makeWindow('5h'))).toBe(false)
  })

  it('returns_true_when_request_or_token_or_cost_nonzero', () => {
    expect(hasAnyUsage(makeWindow('5h', withRequests(1)))).toBe(true)
    expect(hasAnyUsage(makeWindow('5h', { tokenUsed: 10 }))).toBe(true)
    expect(hasAnyUsage(makeWindow('5h', { cost: 0.01 }))).toBe(true)
  })
})

describe('pickEffectiveWindow', () => {
  it('uses_preferred_window_when_it_has_data', () => {
    const windows = [
      makeWindow('5h', withRequests(3)),
      makeWindow('30d', withRequests(99))
    ]
    expect(pickEffectiveWindow(windows, '5h')).toEqual({ window: '5h', isFallback: false })
  })

  it('falls_back_to_30d_then_current_month_then_7d_priority', () => {
    // 5h 无数据：30d 与 current_month 都有 → 按优先级选 30d
    const windows = [
      makeWindow('5h'),
      makeWindow('current_month', withRequests(1)),
      makeWindow('30d', withRequests(2))
    ]
    expect(pickEffectiveWindow(windows, '5h')).toEqual({ window: '30d', isFallback: true })

    // 30d 无数据 → 下一个有数据的 current_month
    const windows2 = [
      makeWindow('5h'),
      makeWindow('30d'),
      makeWindow('current_month', withRequests(1)),
      makeWindow('7d', withRequests(2))
    ]
    expect(pickEffectiveWindow(windows2, '5h')).toEqual({ window: 'current_month', isFallback: true })

    // 30d 与 current_month 都无数据 → 7d
    const windows3 = [
      makeWindow('5h'),
      makeWindow('30d'),
      makeWindow('current_month'),
      makeWindow('7d', withRequests(2))
    ]
    expect(pickEffectiveWindow(windows3, '5h')).toEqual({ window: '7d', isFallback: true })
  })

  it('falls_back_to_shorter_windows_when_long_ones_empty', () => {
    const windows = [
      makeWindow('5h'),
      makeWindow('30d'),
      makeWindow('current_month'),
      makeWindow('7d'),
      makeWindow('24h'),
      makeWindow('today', withRequests(1))
    ]
    expect(pickEffectiveWindow(windows, '5h')).toEqual({ window: 'today', isFallback: true })
  })

  it('returns_preferred_window_with_isFallback_false_when_everything_empty', () => {
    const windows = [makeWindow('5h'), makeWindow('30d'), makeWindow('current_month')]
    expect(pickEffectiveWindow(windows, '5h')).toEqual({ window: '5h', isFallback: false })
  })

  it('returns_first_window_when_preferred_missing_and_all_empty', () => {
    const windows = [makeWindow('24h'), makeWindow('30d')]
    // 首选 '5h' 不在列表中 → 显示 windows[0]，且非回退（与组件 isWindowFallback 的 !!preferred 前置一致）
    expect(pickEffectiveWindow(windows, '5h')).toEqual({ window: '24h', isFallback: false })
  })

  it('falls_back_without_fallback_flag_when_preferred_missing', () => {
    // 首选 '5h' 不在列表中但 30d 有数据：窗口数据回退到 30d，
    // 但 isFallback=false（组件历史实现：!!preferred 前置，首选缺失不提示）
    const windows = [makeWindow('24h'), makeWindow('30d', withRequests(2))]
    expect(pickEffectiveWindow(windows, '5h')).toEqual({ window: '30d', isFallback: false })
  })

  it('returns_null_for_empty_window_list', () => {
    expect(pickEffectiveWindow([], '5h')).toEqual({ window: null, isFallback: false })
  })

  it('orders_fallback_without_duplicates_and_keeps_30d_first', () => {
    expect(FALLBACK_WINDOW_ORDER[0]).toBe('30d')
    expect(new Set(FALLBACK_WINDOW_ORDER).size).toBe(FALLBACK_WINDOW_ORDER.length)
  })
})
