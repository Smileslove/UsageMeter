import { invoke } from '@tauri-apps/api/core'

export function getExchangeRates(currencies: string[]): Promise<Record<string, number>> {
  return invoke('get_exchange_rates', { currencies })
}
