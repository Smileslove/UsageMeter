// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { shallowMount } from '@vue/test-utils'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

import LimitSurvivalCard from './LimitSurvivalCard.vue'
import { useMonitorStore } from '../../stores/monitor'

describe('LimitSurvivalCard', () => {
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
