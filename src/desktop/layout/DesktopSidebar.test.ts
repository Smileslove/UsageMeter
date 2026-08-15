// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { mount } from '@vue/test-utils'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import DesktopSidebar from './DesktopSidebar.vue'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue([])
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {})
}))

describe('侧栏点击导航', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    window.location.hash = ''
  })

  it('点击项目/请求按钮后 currentPage 切换', async () => {
    const wrapper = mount(DesktopSidebar)
    const nav = useDesktopNavigationStore()
    const buttons = wrapper.findAll('nav button')
    const labels = buttons.map(b => b.attributes('aria-label'))
    expect(labels).toContain('项目')
    expect(labels).toContain('请求')
    // 点击「项目」
    const projectsBtn = buttons.find(b => b.attributes('aria-label') === '项目')
    await projectsBtn!.trigger('click')
    expect(nav.currentPage).toBe('projects')
    expect(window.location.hash).toBe('#/desktop/projects')
    // 点击「请求」
    const requestsBtn = buttons.find(b => b.attributes('aria-label') === '请求')
    await requestsBtn!.trigger('click')
    expect(nav.currentPage).toBe('requests')
    expect(window.location.hash).toBe('#/desktop/requests')
    wrapper.unmount()
  })
})
