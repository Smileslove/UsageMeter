// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, shallowMount } from '@vue/test-utils'

const { listenMock, unlistenRefresh, store, nav } = vi.hoisted(() => ({
  listenMock: vi.fn(),
  unlistenRefresh: vi.fn(),
  store: {
    settings: { theme: { appearance: 'system' } },
    initialize: vi.fn(),
    startAutoRefresh: vi.fn(),
    stopAutoRefresh: vi.fn(),
    refreshUsageAndSessionViews: vi.fn(),
  },
  nav: {
    currentPage: 'overview',
    syncFromHash: vi.fn(),
    applyNavigationTarget: vi.fn(),
  },
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }))
vi.mock('../api/appApi', () => ({ takePendingDesktopNavigation: vi.fn().mockResolvedValue(null) }))
vi.mock('../stores/monitor', () => ({ useMonitorStore: () => store }))
vi.mock('../desktop/stores/desktopNavigation', () => ({ useDesktopNavigationStore: () => nav }))
vi.mock('../theme', () => ({ applyResolvedTheme: vi.fn() }))

import DesktopApp from './DesktopApp.vue'

describe('desktop window refresh', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    store.initialize.mockResolvedValue(undefined)
    store.refreshUsageAndSessionViews.mockResolvedValue(undefined)
    listenMock.mockImplementation(async (event: string) =>
      event === 'desktop-refresh' ? unlistenRefresh : vi.fn()
    )
  })

  it('refreshes usage and session views once per reopen event and removes the listener on unmount', async () => {
    const wrapper = shallowMount(DesktopApp)
    await flushPromises()

    const refreshListener = listenMock.mock.calls.find(([event]) => event === 'desktop-refresh')?.[1]
    expect(refreshListener).toBeTypeOf('function')
    expect(store.refreshUsageAndSessionViews).not.toHaveBeenCalled()

    refreshListener({ payload: null })
    await flushPromises()
    expect(store.refreshUsageAndSessionViews).toHaveBeenCalledTimes(1)

    refreshListener({ payload: null })
    await flushPromises()
    expect(store.refreshUsageAndSessionViews).toHaveBeenCalledTimes(2)

    wrapper.unmount()
    expect(unlistenRefresh).toHaveBeenCalledOnce()
    expect(store.stopAutoRefresh).toHaveBeenCalledOnce()
  })
})
