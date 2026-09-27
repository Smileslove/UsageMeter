import { describe, expect, it } from 'vitest'
import { usedPercentOf, useQuotaLimits } from './useQuotaLimits'
import { createPinia, setActivePinia } from 'pinia'
import { useMonitorStore } from '../../stores/monitor'

describe('usedPercentOf', () => {
  it('treats zero utilization as a valid safe value', () => {
    expect(usedPercentOf({ name: 'five_hour', utilization: 0 })).toBe(0)
  })

  it('does not turn an invalid numeric value into a quota state', () => {
    expect(usedPercentOf({ name: 'five_hour', utilization: Number.NaN })).toBeNull()
  })

  it('treats an explicitly reached window as dangerous even below the percentage threshold', () => {
    setActivePinia(createPinia())
    const store = useMonitorStore()
    store.subscriptionQuota = {
      success: true,
      credentialStatus: 'valid',
      queriedAt: 0,
      quota: {
        provider: 'gpt',
        tool: 'codex_oauth',
        credentialStatus: 'valid',
        success: true,
        tiers: [{ name: 'five_hour', utilization: 10, limitReached: true }],
        updatedAt: 0,
        fromCache: false
      }
    }

    const { limitRows } = useQuotaLimits()
    expect(limitRows.value.find(row => row.key === 'codex')?.state).toBe('danger')
  })
})
