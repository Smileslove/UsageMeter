import { computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { formatCost as formatCostUtil } from '../utils/format'

export function useCurrency() {
  const store = useMonitorStore()
  const currency = computed(() => store.settings.currency)

  function formatCost(value: number): string {
    return formatCostUtil(value, currency.value)
  }

  function convertToUSD(amount: number, fromCurrency: string): number {
    const rate = currency.value.exchangeRates[fromCurrency] || 1.0
    return amount / rate
  }

  function fromUSD(amount: number): number {
    const rate = currency.value.exchangeRates[currency.value.displayCurrency] || 1.0
    return amount * rate
  }

  return { currency, formatCost, convertToUSD, fromUSD }
}
