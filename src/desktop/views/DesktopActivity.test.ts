// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { shallowMount } from '@vue/test-utils'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }))
vi.mock('../composables/useActivityData', async () => {
  const { ref } = await import('vue')
  return {
    useActivityData: () => ({
      activeSessionKey: ref('session-1'),
      viewState: ref('ready'),
      agents: ref([]),
      toolSummary: ref([]),
      events: ref([]),
      total: ref(0),
      hasMore: ref(false),
      loading: ref(false),
      loadingMore: ref(false),
      building: ref(false),
      error: ref(''),
      selectedKinds: ref(new Set()),
      activeAgentKey: ref(null),
      relationLevel: ref('none'),
      badgeMeta: ref(null),
      fulltextEnabled: ref(false),
      loadSession: vi.fn(),
      loadEvents: vi.fn(),
      rebuildAndReload: vi.fn(),
      retry: vi.fn(),
    })
  }
})

import DesktopActivity from './DesktopActivity.vue'

describe('DesktopActivity', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    Object.defineProperty(window, 'matchMedia', {
      configurable: true,
      value: vi.fn().mockReturnValue({
        matches: true,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })
    })
  })

  it('keeps the activity layout visible when full-text search is disabled', () => {
    const wrapper = shallowMount(DesktopActivity)

    expect(wrapper.text()).toContain('全文索引')
    expect(wrapper.findComponent({ name: 'ActivityTimeline' }).exists()).toBe(true)
  })
})
