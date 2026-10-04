import { computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { formatCost as formatCostUtil } from '../utils/format'

export function useCurrency() {
  const store = useMonitorStore()
  const currency = computed(() => store.settings.currency)

  function formatCost(value: number): string {
    return formatCostUtil(value, currency.value)
  }

  function exchangeRateFor(code: string): number {
    const rate = currency.value.exchangeRates[code]
    return Number.isFinite(rate) && rate > 0 ? rate : 1.0
  }

  function convertToUSD(amount: number, fromCurrency: string): number {
    return amount / exchangeRateFor(fromCurrency)
  }

  function fromUSD(amount: number): number {
    return amount * exchangeRateFor(currency.value.displayCurrency)
  }

  return { currency, formatCost, convertToUSD, fromUSD }
}
