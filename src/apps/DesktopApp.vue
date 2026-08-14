<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { takePendingDesktopNavigation } from '../api/appApi'
import { useMonitorStore } from '../stores/monitor'
import { useDesktopNavigationStore } from '../desktop/stores/desktopNavigation'
import { applyResolvedTheme } from '../theme'
import type { DesktopNavigationTarget } from '../types'
import DesktopShell from '../desktop/layout/DesktopShell.vue'
import DesktopPageBoundary from '../desktop/components/DesktopPageBoundary.vue'
import DesktopOverview from '../desktop/views/DesktopOverview.vue'
import DesktopAnalytics from '../desktop/views/DesktopAnalytics.vue'
import DesktopSessions from '../desktop/views/DesktopSessions.vue'
import DesktopActivity from '../desktop/views/DesktopActivity.vue'
import DesktopGateway from '../desktop/views/DesktopGateway.vue'
import DesktopSettings from '../desktop/views/DesktopSettings.vue'

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

function applyNavigationTarget(target: DesktopNavigationTarget) {
  nav.applyNavigationTarget(target)
}

onMounted(async () => {
  // 初始化 monitor store（参考 App.vue；跨 WebView 各自初始化，后端操作幂等）
  await store.initialize()
  await nextTick()
  applyResolvedTheme(store.settings.theme)

  mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  mediaQuery.addEventListener('change', handleSystemThemeChange)

  // hash 路由：hashchange -> store 同步；store 写入 hash 走 writeHash（防循环）
  window.addEventListener('hashchange', handleHashChange)
  nav.syncFromHash()

  // 深链一：窗口已存在时，Rust 通过事件投递导航目标
  unlistenNavigation = await listen<DesktopNavigationTarget>('desktop-navigation', event => {
    applyNavigationTarget(event.payload)
  })

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
