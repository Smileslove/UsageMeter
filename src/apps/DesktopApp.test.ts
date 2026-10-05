// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, shallowMount } from '@vue/test-utils'

const { listenMock, unlistenRefresh, unlistenLocalUsageSynced, store, nav } = vi.hoisted(() => ({
  listenMock: vi.fn(),
  unlistenRefresh: vi.fn(),
  unlistenLocalUsageSynced: vi.fn(),
  store: {
    settings: { theme: { appearance: 'system' } },
    initialize: vi.fn(),
    startAutoRefresh: vi.fn(),
    stopAutoRefresh: vi.fn(),
    refreshUsageAndSessionViews: vi.fn(),
    refreshSources: vi.fn().mockResolvedValue(undefined),
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
      event === 'desktop-refresh' ? unlistenRefresh : event === 'local_usage_synced' ? unlistenLocalUsageSynced : vi.fn()
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
    expect(unlistenLocalUsageSynced).toHaveBeenCalledOnce()
    expect(store.stopAutoRefresh).toHaveBeenCalledOnce()
  })

  it('refreshes after the background local usage scan completes', async () => {
    const wrapper = shallowMount(DesktopApp)
    await flushPromises()

    const syncListener = listenMock.mock.calls.find(([event]) => event === 'local_usage_synced')?.[1]
    expect(syncListener).toBeTypeOf('function')

    syncListener({ payload: null })
    await flushPromises()
    expect(store.refreshSources).toHaveBeenCalledOnce()
    expect(store.refreshUsageAndSessionViews).toHaveBeenCalledOnce()
    wrapper.unmount()
  })

  it('registers the refresh listener before initialization can block startup', async () => {
    const order: string[] = []
    store.initialize.mockImplementationOnce(async () => {
      order.push('initialize')
    })
    listenMock.mockImplementationOnce(async (event: string) => {
      if (event === 'desktop-refresh') order.push('listen')
      return unlistenRefresh
    })

    const wrapper = shallowMount(DesktopApp)
    await flushPromises()

    expect(order).toEqual(['listen', 'initialize'])
    wrapper.unmount()
  })
})
