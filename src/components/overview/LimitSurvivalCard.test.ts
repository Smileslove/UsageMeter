// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { shallowMount } from '@vue/test-utils'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

import LimitSurvivalCard from './LimitSurvivalCard.vue'
import { useMonitorStore } from '../../stores/monitor'

describe('LimitSurvivalCard', () => {
  it('translates the remaining percentage instead of showing its translation key', () => {
    const pinia = createPinia()
    setActivePinia(pinia)
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
        tiers: [
          { name: 'five_hour', utilization: 11 },
          { name: 'seven_day', utilization: 5 },
        ],
        updatedAt: 0,
        fromCache: false,
      },
    }

    const wrapper = shallowMount(LimitSurvivalCard, { global: { plugins: [pinia] } })

    expect(wrapper.text()).toContain('剩余 89%')
    expect(wrapper.findAll('.quota-strip-tier-row .compact-progress-fill').map(item => item.attributes('style')))
      .toEqual(['width: 89%;', 'width: 95%;'])
  })

  it('shows an explicitly reached Codex window as exhausted with no remaining progress', () => {
    const pinia = createPinia()
    setActivePinia(pinia)
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
        fromCache: false,
      },
    }

    const wrapper = shallowMount(LimitSurvivalCard, {
      global: { plugins: [pinia] },
    })

    expect(wrapper.text()).toContain('已耗尽')
    expect(wrapper.findAll('.compact-progress-fill').at(-1)?.attributes('style')).toContain('width: 0%')
  })
})
