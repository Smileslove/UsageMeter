import { describe, it, expect, beforeEach } from 'vitest'
import type { CurrencySettings } from '../types'
import {
  convertCost,
  formatCost,
  formatDurationMs,
  formatRate,
  formatRequestCount,
  formatTokenValue,
  formatUsedTotal,
  setNumberFormatMode,
} from './format'

// format.ts 持有模块级状态（numberFormatMode / unitLocale），
// 每个用例前复位到国际模式 + zh-CN locale，避免相互污染。
beforeEach(() => {
  setNumberFormatMode('international', 'zh-CN')
})

function currency(displayCurrency: string, rate: number): CurrencySettings {
  return {
    displayCurrency,
    exchangeRates: { [displayCurrency]: rate },
    trackedCurrencies: [displayCurrency],
    lastRateUpdate: null,
  }
}

describe('formatRequestCount / pickScale', () => {
  it('returns_plain_rounded_number_below_K_threshold', () => {
    expect(formatRequestCount(0)).toBe('0')
    expect(formatRequestCount(999)).toBe('999')
    expect(formatRequestCount(999.6)).toBe('1000')
  })

  it('promotes_to_K_M_B_at_thresholds', () => {
    expect(formatRequestCount(1000)).toBe('1.00K')
    expect(formatRequestCount(1500)).toBe('1.50K')
    expect(formatRequestCount(1000000)).toBe('1.00M')
    expect(formatRequestCount(1234567)).toBe('1.23M')
    expect(formatRequestCount(1000000000)).toBe('1.00B')
  })

  it('promotes_one_step_before_crossing_upper_bound_of_lower_unit', () => {
    // 999_999 若用 K 档会显示 "1000.00K"，必须晋级为 "1.00M"
    expect(formatRequestCount(999999)).toBe('1.00M')
    expect(formatRequestCount(999999999)).toBe('1.00B')
  })

  it('handles_negative_and_non_finite_inputs', () => {
    expect(formatRequestCount(-1)).toBe('-1')
    expect(formatRequestCount(-1000)).toBe('-1000')
    expect(formatRequestCount(NaN)).toBe('NaN')
    expect(formatRequestCount(Infinity)).toBe('InfinityB')
  })
})

describe('formatTokenValue', () => {
  it('uses_integer_when_below_threshold', () => {
    expect(formatTokenValue(0)).toBe('0')
    expect(formatTokenValue(1234)).toBe('1.23K')
    expect(formatTokenValue(-500)).toBe('-500')
  })

  it('promotes_units_and_keeps_sign_for_negative_input', () => {
    expect(formatTokenValue(999999)).toBe('1.00M')
    expect(formatTokenValue(-1500000)).toBe('-1.50M')
  })

  it('respects_shared_unit_base_from_pair', () => {
    expect(formatTokenValue(500, 1000)).toBe('0.50K')
    expect(formatTokenValue(1500, 2000)).toBe('1.50K')
  })

  it('handles_non_finite_inputs', () => {
    expect(formatTokenValue(NaN)).toBe('NaN')
    expect(formatTokenValue(Infinity)).toBe('InfinityB')
  })
})

describe('formatUsedTotal', () => {
  it('returns_plain_pair_below_threshold', () => {
    expect(formatUsedTotal(0, 0)).toBe('0 / 0')
    expect(formatUsedTotal(999, 999)).toBe('999 / 999')
    expect(formatUsedTotal(0.4, 0.6)).toBe('0 / 1')
  })

  it('shares_unit_between_used_and_total', () => {
    expect(formatUsedTotal(1000, 2000)).toBe('1.00K / 2K')
    expect(formatUsedTotal(1234, 5678)).toBe('1.23K / 5.68K')
    expect(formatUsedTotal(999999, 1000000)).toBe('1.00M / 1M')
  })

  it('clamps_negative_values_to_zero', () => {
    // total 部分经 Number() 去掉尾零（设计注释：「限额去掉多余的 0」）
    expect(formatUsedTotal(-100, 1500)).toBe('0.00K / 1.5K')
    expect(formatUsedTotal(-100, -50)).toBe('0 / 0')
  })

  it('exposes_nan_as_is_without_guard', () => {
    // 已知行为：NaN 未被 clamp 成 0（Math.max(0, NaN) === NaN）
    expect(formatUsedTotal(NaN, 100)).toBe('NaN / 100')
  })
})

describe('formatDurationMs', () => {
  it('returns_dash_for_zero_negative_and_nan', () => {
    expect(formatDurationMs(0)).toBe('-')
    expect(formatDurationMs(-1)).toBe('-')
    expect(formatDurationMs(NaN)).toBe('-')
  })

  it('formats_milliseconds_below_one_second', () => {
    expect(formatDurationMs(999)).toBe('999ms')
    expect(formatDurationMs(0.4)).toBe('0ms')
  })

  it('formats_seconds_above_one_second', () => {
    expect(formatDurationMs(1000)).toBe('1.00s')
    expect(formatDurationMs(55000)).toBe('55.00s')
    expect(formatDurationMs(61000)).toBe('61.00s')
    expect(formatDurationMs(Infinity)).toBe('Infinitys')
  })
})

describe('formatRate', () => {
  it('returns_zero_for_zero_negative_and_nan', () => {
    expect(formatRate(0)).toBe('0')
    expect(formatRate(-0.5)).toBe('0')
    expect(formatRate(NaN)).toBe('0')
  })

  it('formats_one_decimal_below_100_and_rounded_integer_at_or_above', () => {
    expect(formatRate(12.34)).toBe('12.3')
    expect(formatRate(99.9)).toBe('99.9')
    expect(formatRate(100)).toBe('100')
    expect(formatRate(123.45)).toBe('123')
    expect(formatRate(Infinity)).toBe('Infinity')
  })
})

describe('formatCost', () => {
  it('formats_four_decimals_with_dollar_by_default', () => {
    expect(formatCost(1.23456)).toBe('$1.2346')
    expect(formatCost(0)).toBe('$0.0000')
    expect(formatCost(-2.5)).toBe('$-2.5000')
  })

  it('converts_with_currency_rate_and_symbol', () => {
    expect(formatCost(1.2, currency('USD', 1.0))).toBe('$1.2000')
    expect(formatCost(1.2, currency('CNY', 7.1))).toBe('¥8.5200')
    expect(formatCost(1.2, currency('JPY', 145))).toBe('¥174.0000')
  })

  it('honors_custom_precision', () => {
    expect(formatCost(1, undefined, 2)).toBe('$1.00')
    expect(formatCost(3.14159, currency('USD', 1.0), 2)).toBe('$3.14')
  })

  it('returns_zero_padding_for_non_finite_values', () => {
    expect(formatCost(NaN)).toBe('$0.0000')
    expect(formatCost(Infinity)).toBe('$0.0000')
  })
})

describe('convertCost', () => {
  it('multiplies_by_display_currency_rate', () => {
    expect(convertCost(10, currency('CNY', 7.1))).toBe(71)
    expect(convertCost(10, currency('USD', 1.0))).toBe(10)
    expect(convertCost(2.5, currency('CNY', 1.1))).toBe(2.75)
  })

  it('treats_zero_rate_as_default_1_0', () => {
    // 真实 bug：`exchangeRates[c] || 1.0` 把合法汇率 0 吞掉变成 1.0
    expect(convertCost(10, currency('CNY', 0))).toBe(10)
  })
})

describe('中文 万/亿 模式', () => {
  it('uses_wan_yi_units_after_chinese_mode', () => {
    setNumberFormatMode('chinese', 'zh-CN')
    expect(formatRequestCount(10000)).toBe('1.00万')
    expect(formatRequestCount(100000000)).toBe('1.00亿')
    expect(formatRequestCount(9999)).toBe('9999')
  })

  it('promotes_wan_to_yi_before_crossing_upper_bound', () => {
    setNumberFormatMode('chinese', 'zh-CN')
    expect(formatRequestCount(99999999)).toBe('1.00亿')
    expect(formatTokenValue(15000)).toBe('1.50万')
    expect(formatUsedTotal(10000, 50000)).toBe('1.00万 / 5万')
  })

  it('switches_suffix_with_zh_TW_locale', () => {
    setNumberFormatMode('chinese', 'zh-TW')
    expect(formatRequestCount(10000)).toBe('1.00萬')
  })

  it('resets_back_to_international_mode', () => {
    setNumberFormatMode('chinese', 'zh-CN')
    setNumberFormatMode('international')
    expect(formatRequestCount(10000)).toBe('10.00K')
  })
})
