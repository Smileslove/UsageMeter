import { defineStore } from 'pinia'
import { useMonitorStore } from '../../stores/monitor'
import type { DesktopNavigationTarget, DesktopPage } from '../../types'

/**
 * 桌面主窗口的轻量 hash 路由状态（不引入 vue-router）。
 *
 * URL 只包含非敏感页面标识：
 *   #/desktop/overview | analytics | sessions | activity | gateway | settings
 *   #/desktop/sessions/session/<opaqueKey>
 *
 * hashchange -> store 同步由宿主组件（DesktopApp.vue）注册监听并调用 syncFromHash；
 * store 变更 -> hash 写入统一走 writeHash（先比较再写，天然防循环）。
 */

export const DESKTOP_PAGES: DesktopPage[] = ['overview', 'analytics', 'sessions', 'activity', 'gateway', 'settings']

export const DESKTOP_HASH_PREFIX = '#/desktop'

/** 会话列表返回时的恢复信息（M1：scrollTop + 简单筛选对象）。 */
export interface SessionsListQuery {
  scrollTop: number
  sourceId?: string | null
  tool?: string | null
}

/** 深链携带的全局筛选上下文（待对应页面消费；与 DesktopNavigationTarget 全字段对齐）。 */
export interface DesktopNavigationFilters {
  sourceId?: string | null
  tool?: string | null
  window?: string | null
  metric?: string | null
  view?: string | null
  sessionKey?: string | null
}

export function parseDesktopHash(hash: string): { page: DesktopPage; sessionKey: string | null } {
  const rest = hash.replace(/^#\/desktop\/?/, '')
  const parts = rest.split('/').filter(Boolean)
  const raw = parts[0] ?? ''
  const page = (DESKTOP_PAGES as readonly string[]).includes(raw) ? (raw as DesktopPage) : 'overview'
  let sessionKey: string | null = null
  if (parts[0] === 'sessions' && parts[1] === 'session' && parts[2]) {
    try {
      sessionKey = decodeURIComponent(parts[2])
    } catch {
      sessionKey = parts[2]
    }
  }
  // 活动页二级路由：与 #/desktop/sessions/session/<key> 同模式（设计文档 4.2）。
  if (parts[0] === 'activity' && parts[1]) {
    try {
      sessionKey = decodeURIComponent(parts[1])
    } catch {
      sessionKey = parts[1]
    }
  }
  return { page, sessionKey }
}

export function desktopHashFor(page: DesktopPage): string {
  return `${DESKTOP_HASH_PREFIX}/${page}`
}

export function desktopSessionHash(sessionKey: string): string {
  return `${DESKTOP_HASH_PREFIX}/sessions/session/${encodeURIComponent(sessionKey)}`
}

/** 活动页二级路由（设计文档 4.2：#/activity/:opaqueSessionKey）。 */
export function desktopActivityHash(sessionKey: string): string {
  return `${DESKTOP_HASH_PREFIX}/activity/${encodeURIComponent(sessionKey)}`
}

export const useDesktopNavigationStore = defineStore('desktopNavigation', {
  state: () => ({
    currentPage: 'overview' as DesktopPage,
    activeSessionKey: null as string | null,
    sidebarCollapsed: false,
    previousSessionsQuery: null as SessionsListQuery | null,
    pendingFilters: null as DesktopNavigationFilters | null,
    /** 设置页待定位的左侧分组（由活动页“打开设置”跳转时写入，设置页消费后清空）。 */
    settingsTargetSection: null as string | null,
    /** 会话列表视图当前滚动位置（由 DesktopSessions 滚动时上报；openSession 时作为恢复值）。 */
    sessionListScrollTop: 0
  }),
  actions: {
    /** 从当前 location.hash 解析并同步路由状态（hashchange 监听回调）。 */
    syncFromHash() {
      const { page, sessionKey } = parseDesktopHash(window.location.hash)
      if (this.currentPage !== page) {
        this.currentPage = page
      }
      if (this.activeSessionKey !== sessionKey) {
        this.activeSessionKey = sessionKey
      }
    },
    /** 比较后再写 hash，避免与 hashchange 回调形成循环；写入后同步状态保证 UI 即时响应。 */
    writeHash(next: string) {
      if (window.location.hash === next) {
        return
      }
      window.location.hash = next
      this.syncFromHash()
    },
    navigate(page: DesktopPage) {
      this.writeHash(desktopHashFor(page))
    },
    /** 打开会话详情：进入前保存会话列表恢复信息（真实滚动位置 + 当前全局筛选）。 */
    openSession(sessionKey: string) {
      const monitor = useMonitorStore()
      this.previousSessionsQuery = {
        scrollTop: this.sessionListScrollTop,
        sourceId: monitor.settings.sourceAware.activeSourceFilter,
        tool: monitor.settings.clientTools.activeToolFilter
      }
      this.writeHash(desktopSessionHash(sessionKey))
    },
    /** 会话列表滚动上报（DesktopSessions 节流调用；仅列表视图滚动时）。 */
    reportSessionListScrollTop(scrollTop: number) {
      this.sessionListScrollTop = scrollTop
    },
    /** 恢复滚动后清零（由会话列表视图在消费恢复值后调用）。 */
    clearSessionScrollTop() {
      this.sessionListScrollTop = 0
    },
    /** 从会话详情返回列表（恢复信息保留在 previousSessionsQuery 供列表视图消费）。 */
    backToSessions() {
      this.activeSessionKey = null
      this.writeHash(desktopHashFor('sessions'))
    },
    /** 打开活动页并固定到指定会话（#/desktop/activity/<key>）。 */
    openActivity(sessionKey: string) {
      this.writeHash(desktopActivityHash(sessionKey))
    },
    /** 跳到设置页并定位到指定分组（如 privacy）；设置页消费 settingsTargetSection。 */
    openSettingsSection(section: string) {
      this.settingsTargetSection = section
      this.navigate('settings')
    },
    /** 消费并清空会话列表恢复信息（由会话列表视图在挂载时调用）。 */
    consumePreviousSessionsQuery(): SessionsListQuery | null {
      const query = this.previousSessionsQuery
      this.previousSessionsQuery = null
      return query
    },
    /** 应用深链/事件导航目标：先暂存全字段筛选上下文，再按 sessionKey 或 page 导航。 */
    applyNavigationTarget(target: DesktopNavigationTarget) {
      this.pendingFilters = {
        sourceId: target.sourceId ?? null,
        tool: target.tool ?? null,
        window: target.window ?? null,
        metric: target.metric ?? null,
        view: target.view ?? null,
        sessionKey: target.sessionKey ?? null
      }
      if (target.sessionKey) {
        this.openSession(target.sessionKey)
      } else {
        this.navigate(target.page)
      }
    },
    /** 消费并清空深链携带的筛选上下文（由目标页面在挂载时调用）。 */
    consumePendingFilters(): DesktopNavigationFilters | null {
      const filters = this.pendingFilters
      this.pendingFilters = null
      return filters
    }
  }
})
