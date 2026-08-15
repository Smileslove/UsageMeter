// @vitest-environment happy-dom
import { describe, it, expect, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useDesktopNavigationStore, parseDesktopHash, desktopHashFor } from './desktopNavigation'

describe('desktop navigation 新页面路由', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    window.location.hash = ''
  })

  it('parseDesktopHash 解析 projects/requests', () => {
    expect(parseDesktopHash('#/desktop/projects').page).toBe('projects')
    expect(parseDesktopHash('#/desktop/requests').page).toBe('requests')
    expect(parseDesktopHash('#/desktop/sessions').page).toBe('sessions')
  })

  it('desktopHashFor 生成正确 hash', () => {
    expect(desktopHashFor('projects')).toBe('#/desktop/projects')
    expect(desktopHashFor('requests')).toBe('#/desktop/requests')
  })

  it('navigate 到 projects/requests 后 currentPage 更新', () => {
    const nav = useDesktopNavigationStore()
    nav.navigate('projects')
    expect(nav.currentPage).toBe('projects')
    expect(window.location.hash).toBe('#/desktop/projects')
    nav.navigate('requests')
    expect(nav.currentPage).toBe('requests')
    expect(window.location.hash).toBe('#/desktop/requests')
  })

  it('hash 相同时点击同页导航项仍同步 currentPage（回归：残留 hash 导致点击无反应）', () => {
    const nav = useDesktopNavigationStore()
    // 模拟状态脱节：hash 已是 projects 但 currentPage 是 overview（如深链后未同步）
    window.location.hash = '#/desktop/projects'
    nav.currentPage = 'overview'
    // 点击「项目」：hash 相同，writeHash 不应提前 return，必须 syncFromHash
    nav.navigate('projects')
    expect(nav.currentPage).toBe('projects')
    expect(window.location.hash).toBe('#/desktop/projects')
  })
})
