import { beforeEach, describe, expect, it } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useMonitorStore } from '../stores/monitor'
import { useCurrency } from './useCurrency'

describe('useCurrency', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('converts USD and configured currencies using USD-based rates', () => {
    const store = useMonitorStore()
    store.settings.currency = {
      displayCurrency: 'CNY',
      exchangeRates: { USD: 1, CNY: 7.2 },
      trackedCurrencies: ['USD', 'CNY'],
      lastRateUpdate: null,
    }
    const { convertToUSD, fromUSD } = useCurrency()

    expect(convertToUSD(72, 'CNY')).toBe(10)
    expect(fromUSD(10)).toBe(72)
  })

  it('falls back to one for invalid or missing rates', () => {
    const store = useMonitorStore()
    store.settings.currency = {
      displayCurrency: 'CNY',
      exchangeRates: { USD: 1, CNY: 0, EUR: Number.NaN },
      trackedCurrencies: ['USD', 'CNY', 'EUR'],
      lastRateUpdate: null,
    }
    const { convertToUSD, fromUSD } = useCurrency()

    expect(convertToUSD(10, 'CNY')).toBe(10)
    expect(convertToUSD(10, 'EUR')).toBe(10)
    expect(fromUSD(10)).toBe(10)
  })
})
