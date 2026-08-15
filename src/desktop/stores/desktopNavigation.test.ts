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
    // 点击「项目」：hash 相同，writeHash 不应提前 return，必须同步状态
    nav.navigate('projects')
    expect(nav.currentPage).toBe('projects')
    expect(window.location.hash).toBe('#/desktop/projects')
  })

  it('Tauri WebView 时序：hash 写入后立即读取仍为旧值时，导航状态基于目标参数而非旧 hash（回归：点击跳转概览）', () => {
    const nav = useDesktopNavigationStore()
    // 模拟用户当前在概览页
    window.location.hash = '#/desktop/overview'
    nav.syncFromHash()
    expect(nav.currentPage).toBe('overview')
    // 模拟 Tauri WebView 的时序问题：writeHash 内部只依赖传入的目标参数解析，
    // 不读取 window.location.hash（WebView 中写入后立即读取可能返回旧值）。
    // 这里直接断言 writeHash 的解析来源：即使外部 hash 读取异常，导航也按目标解析。
    nav.writeHash('#/desktop/requests')
    expect(nav.currentPage).toBe('requests')
    nav.writeHash('#/desktop/projects')
    expect(nav.currentPage).toBe('projects')
    // 深链（applyNavigationTarget）也走同一路径
    nav.applyNavigationTarget({ page: 'requests' })
    expect(nav.currentPage).toBe('requests')
  })
})
