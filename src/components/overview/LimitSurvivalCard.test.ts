// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { shallowMount } from '@vue/test-utils'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

import LimitSurvivalCard from './LimitSurvivalCard.vue'
import { useMonitorStore } from '../../stores/monitor'

describe('LimitSurvivalCard', () => {
  it('renders measured remaining counts and translated labels for each window', () => {
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
          { name: 'five_hour', utilization: 0, maxValue: 100, remainingValue: 89 },
          { name: 'seven_day', utilization: 5 },
        ],
        updatedAt: 0,
        fromCache: false,
      },
    }

    const wrapper = shallowMount(LimitSurvivalCard, { global: { plugins: [pinia] } })

    expect(wrapper.findAll('.quota-stat-emphasis dd').map(item => item.text())).toEqual(['89%', '95%'])
    expect(wrapper.find('.quota-stat-emphasis dt').text()).toBe('剩余')
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

  it('keeps unavailable quota separate from exhausted quota and refreshes the existing source', async () => {
    const pinia = createPinia()
    setActivePinia(pinia)
    const store = useMonitorStore()
    store.subscriptionQuota = {
      success: true, credentialStatus: 'valid', queriedAt: 0,
      quota: {
        provider: 'gpt', tool: 'codex_oauth', credentialStatus: 'valid', success: true,
        tiers: [{ name: 'five_hour', utilization: 0, utilizationAvailable: false, resetsAt: 'invalid' }],
        updatedAt: 0, fromCache: false,
      },
    }
    const refresh = vi.spyOn(store, 'refreshSubscriptionQuota').mockResolvedValue(undefined)
    const wrapper = shallowMount(LimitSurvivalCard, { global: { plugins: [pinia] } })
    const tier = wrapper.find('.quota-strip-tier-row')
    expect(tier.find('[role="progressbar"]').exists()).toBe(false)
    expect(tier.find('.quota-stat-emphasis dd').text()).toBe('--')
    expect(tier.findAll('.quota-stat dd').at(-1)?.text()).toBe('--')
    await wrapper.find('.compact-refresh').trigger('click')
    expect(refresh).toHaveBeenCalledOnce()
    store.subscriptionLoading = true
    await wrapper.vm.$nextTick()
    expect(wrapper.find('.compact-refresh').attributes('disabled')).toBeDefined()
  })

  it.each(['zh-CN', 'zh-TW', 'en-US'])('renders local time independently of token usage in %s', locale => {
    const pinia = createPinia()
    setActivePinia(pinia)
    const store = useMonitorStore()
    store.settings.locale = locale as typeof store.settings.locale
    store.limitSurvival = {
      generatedAtEpoch: 0, sourceKind: 'baseline',
      block: {
        startEpoch: 0, resetsAtEpoch: 18000, elapsedSeconds: 3600, remainingSeconds: 14400,
        usedTokens: 50000, usedRequests: 12, projectedEndTokens: 250000,
      },
      burn: { tokensPerHour: 0, requestsPerHour: 0, sampleSeconds: 0, sampleRequests: 0, confidence: 'low' },
      baseline: { avgTokensPerHour: 10000, relativeToBaseline: null },
    }
    const wrapper = shallowMount(LimitSurvivalCard, { global: { plugins: [pinia] } })
    expect(wrapper.find('[role="progressbar"]').attributes('aria-valuenow')).toBe('20')
    expect(wrapper.find('.quota-local-stats dd').text()).toBe('20.0%')
    expect(wrapper.findAll('.quota-local-stats dd').map(item => item.text())).toEqual(['20.0%', '1h / 5h', '4h', locale === 'en-US' ? 'Last 7 days' : '近 7 日'])
    expect(wrapper.text()).not.toContain('survival.')
    store.limitSurvival.block!.elapsedSeconds = 0
    const emptyWrapper = shallowMount(LimitSurvivalCard, { global: { plugins: [pinia] } })
    expect(emptyWrapper.findAll('.quota-local-stats dd')[1].text()).toBe('0m / 5h')
  })

})
