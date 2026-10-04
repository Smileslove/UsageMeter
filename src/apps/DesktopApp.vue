<script setup lang="ts">
import { defineAsyncComponent, nextTick, onMounted, onUnmounted, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { takePendingDesktopNavigation } from '../api/appApi'
import { useMonitorStore } from '../stores/monitor'
import { useDesktopNavigationStore } from '../desktop/stores/desktopNavigation'
import { applyResolvedTheme } from '../theme'
import type { DesktopNavigationTarget } from '../types'
import DesktopShell from '../desktop/layout/DesktopShell.vue'
import DesktopPageBoundary from '../desktop/components/DesktopPageBoundary.vue'

const DesktopOverview = defineAsyncComponent(() => import('../desktop/views/DesktopOverview.vue'))
const DesktopAnalytics = defineAsyncComponent(() => import('../desktop/views/DesktopAnalytics.vue'))
const DesktopSessions = defineAsyncComponent(() => import('../desktop/views/DesktopSessions.vue'))
const DesktopProjects = defineAsyncComponent(() => import('../desktop/views/DesktopProjects.vue'))
const DesktopRequests = defineAsyncComponent(() => import('../desktop/views/DesktopRequests.vue'))
const DesktopActivity = defineAsyncComponent(() => import('../desktop/views/DesktopActivity.vue'))
const DesktopGateway = defineAsyncComponent(() => import('../desktop/views/DesktopGateway.vue'))
const DesktopSettings = defineAsyncComponent(() => import('../desktop/views/DesktopSettings.vue'))

const store = useMonitorStore()
const nav = useDesktopNavigationStore()

// 监听系统主题变化（与快速面板一致：system 外观下跟随系统切换）
let mediaQuery: MediaQueryList | null = null
const handleSystemThemeChange = () => {
  if (store.settings.theme.appearance === 'system') {
    applyResolvedTheme(store.settings.theme)
  }
}

watch(
  () => store.settings.theme,
  newTheme => applyResolvedTheme(newTheme),
  { deep: true }
)

const handleHashChange = () => nav.syncFromHash()

let unlistenNavigation: UnlistenFn | null = null
let unlistenRefresh: UnlistenFn | null = null
let unlistenLocalUsageSynced: UnlistenFn | null = null
let refreshReady = false
let refreshPending = false

async function flushPendingRefresh() {
  if (!refreshReady || !refreshPending || store.loading) {
    return
  }
  refreshPending = false
  await store.refreshUsageAndSessionViews()
}

function requestWindowRefresh() {
  refreshPending = true
  void flushPendingRefresh()
}

watch(
  () => store.loading,
  loading => {
    if (!loading) {
      void flushPendingRefresh()
    }
  }
)

function applyNavigationTarget(target: DesktopNavigationTarget) {
  nav.applyNavigationTarget(target)
}

onMounted(async () => {
  // hash 路由先注册：即使下方 initialize 失败（网络/权限等），hash 导航与深链同步仍可用。
  window.addEventListener('hashchange', handleHashChange)
  nav.syncFromHash()

  // 先注册刷新事件，再初始化数据。初始化包含扫描和额度查询，若顺序相反，
  // 用户在启动阶段打开桌面窗口时发出的刷新事件会被丢弃。
  try {
    unlistenRefresh = await listen('desktop-refresh', () => {
      requestWindowRefresh()
    })
  } catch (error) {
    console.error('[DesktopApp] Failed to listen for desktop refresh:', error)
  }
  try {
    // 初次扫描在后台完成；完成后刷新快照，避免启动时只显示旧缓存或空数据。
    unlistenLocalUsageSynced = await listen('local_usage_synced', () => {
      requestWindowRefresh()
    })
  } catch (error) {
    console.error('[DesktopApp] Failed to listen for local usage sync:', error)
  }

  // 初始化 monitor store（参考 App.vue；跨 WebView 各自初始化，后端操作幂等）
  try {
    await store.initialize()
  } catch (error) {
    // 初始化失败不阻塞 UI：数据区显示空态，导航与页面骨架仍可操作
    console.error('[DesktopApp] store.initialize 失败（导航与骨架仍可用）:', error)
  }
  refreshReady = true
  await flushPendingRefresh()
  // 桌面窗口有独立的 WebView 和 Pinia store，需要自行启动用量轮询。
  store.startAutoRefresh()

  await nextTick()
  applyResolvedTheme(store.settings.theme)

  mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  mediaQuery.addEventListener('change', handleSystemThemeChange)

  // 深链一：窗口已存在时，Rust 通过事件投递导航目标
  try {
    unlistenNavigation = await listen<DesktopNavigationTarget>('desktop-navigation', event => {
      applyNavigationTarget(event.payload)
    })
  } catch (error) {
    // capability 未授权时降级：失去"窗口已存在"路径的实时深链，挂载时暂存导航仍可用
    console.error('[DesktopApp] listen desktop-navigation 失败（capability 可能未授权 desktop 窗口）:', error)
  }

  // 深链二：窗口加载期间 Rust 暂存的待处理导航（先注册监听再取，避免竞态）
  try {
    const pending = await takePendingDesktopNavigation()
    if (pending) {
      applyNavigationTarget(pending)
    }
  } catch (error) {
    console.error('[DesktopApp] Failed to take pending desktop navigation:', error)
  }
})

onUnmounted(() => {
  store.stopAutoRefresh()
  if (unlistenRefresh) {
    unlistenRefresh()
  }
  if (unlistenLocalUsageSynced) {
    unlistenLocalUsageSynced()
  }
  if (mediaQuery) {
    mediaQuery.removeEventListener('change', handleSystemThemeChange)
  }
  window.removeEventListener('hashchange', handleHashChange)
  if (unlistenNavigation) {
    unlistenNavigation()
  }
})
</script>

<template>
  <DesktopShell>
    <Transition name="desktop-page-fade" mode="out-in">
      <!-- 每个视图外包渲染错误边界：任一视图抛错时显示可诊断错误卡片而非空白 -->
      <DesktopPageBoundary v-if="nav.currentPage === 'overview'" key="overview">
        <DesktopOverview />
      </DesktopPageBoundary>
      <DesktopPageBoundary v-else-if="nav.currentPage === 'analytics'" key="analytics">
        <DesktopAnalytics />
      </DesktopPageBoundary>
      <DesktopPageBoundary v-else-if="nav.currentPage === 'sessions'" key="sessions">
        <DesktopSessions />
      </DesktopPageBoundary>
      <DesktopPageBoundary v-else-if="nav.currentPage === 'projects'" key="projects">
        <DesktopProjects />
      </DesktopPageBoundary>
      <DesktopPageBoundary v-else-if="nav.currentPage === 'requests'" key="requests">
        <DesktopRequests />
      </DesktopPageBoundary>
      <DesktopPageBoundary v-else-if="nav.currentPage === 'activity'" key="activity">
        <DesktopActivity />
      </DesktopPageBoundary>
      <DesktopPageBoundary v-else-if="nav.currentPage === 'gateway'" key="gateway">
        <DesktopGateway />
      </DesktopPageBoundary>
      <DesktopPageBoundary v-else key="settings">
        <DesktopSettings />
      </DesktopPageBoundary>
    </Transition>
  </DesktopShell>
</template>

<style>
.desktop-page-fade-enter-active,
.desktop-page-fade-leave-active {
  transition: opacity 0.18s ease-out;
}
.desktop-page-fade-enter-from,
.desktop-page-fade-leave-to {
  opacity: 0;
}
</style>
