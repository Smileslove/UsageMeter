<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from './stores/monitor'
import { useUpdaterStore } from './stores/updater'
import type { UpdateInfo } from './stores/updater'
import Overview from './views/Overview.vue'
import Statistics from './views/Statistics.vue'
import Sessions from './views/Sessions.vue'
import Settings from './views/Settings.vue'
import UpdateDialog from './components/UpdateDialog.vue'
import SourceSelector from './components/SourceSelector.vue'
import ToolSelector from './components/ToolSelector.vue'
import ThemeSelector from './components/ThemeSelector.vue'
import { applyResolvedTheme } from './theme'
import { RefreshCw, ArrowLeftRight } from 'lucide-vue-next'
import { t } from './i18n'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()

const currentView = ref('overview')
const navItems = [
  { id: 'overview', key: 'common.dashboard' },
  { id: 'statistics', key: 'common.statistics' },
  { id: 'sessions', key: 'sessions.title' },
  { id: 'settings', key: 'common.settings' }
]

// 监听系统主题变化
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

// 外部配置变更通知
interface ConfigChangedPayload {
  new_real_base_url: string
  source_id: string
}
interface TakeoverConflictPayload {
  tool: string
  config_path: string
  external_base_url: string
  reclaim_count: number
  window_ms: number
}
const configChangedNotification = ref<ConfigChangedPayload | null>(null)
const takeoverConflictNotification = ref<TakeoverConflictPayload | null>(null)
let configChangedTimer: ReturnType<typeof setTimeout> | null = null

function dismissConfigNotification() {
  configChangedNotification.value = null
  if (configChangedTimer) {
    clearTimeout(configChangedTimer)
    configChangedTimer = null
  }
}

function dismissTakeoverConflictNotification() {
  takeoverConflictNotification.value = null
}

async function resolveTakeoverConflict(action: 'force_reclaim' | 'pause' | 'disable_takeover') {
  const notification = takeoverConflictNotification.value
  if (!notification) {
    return
  }
  await invoke('resolve_takeover_conflict', { tool: notification.tool, action })
  await store.loadSettings()
  await store.getProxyStatus()
  dismissTakeoverConflictNotification()
}

// 退出事件监听器
let unlistenQuit: UnlistenFn | null = null
let unlistenSourceDetected: UnlistenFn | null = null
let unlistenConfigChanged: UnlistenFn | null = null
let unlistenTakeoverConflict: UnlistenFn | null = null
let unlistenUpdateAvailable: UnlistenFn | null = null
let unlistenUpdateProgress: UnlistenFn | null = null

onMounted(async () => {
  await store.initialize()
  store.startAutoRefresh()

  await nextTick()
  applyResolvedTheme(store.settings.theme)

  // 监听系统主题变化
  mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  mediaQuery.addEventListener('change', handleSystemThemeChange)

  // 监听退出请求事件
  unlistenQuit = await listen('app-quit-requested', async () => {
    // 停止自动刷新
    store.stopAutoRefresh()
    // 准备退出（停止代理、恢复配置）
    await store.prepareExit()
    // 通知后端可以退出了
    await invoke('confirm_exit')
  })

  // 监听新来源检测事件
  unlistenSourceDetected = await listen('source_detected', async () => {
    // 重新加载设置以获取最新的来源列表
    await store.loadSettings()
  })

  // 监听外部工具（如 cc switch）修改代理配置事件
  unlistenConfigChanged = await listen<ConfigChangedPayload>('proxy_config_changed', async (event) => {
    configChangedNotification.value = event.payload
    // 重新加载设置以同步最新来源
    await store.loadSettings()
    // 5 秒后自动消失
    if (configChangedTimer) clearTimeout(configChangedTimer)
    configChangedTimer = setTimeout(dismissConfigNotification, 5000)
  })

  unlistenTakeoverConflict = await listen<TakeoverConflictPayload>('takeover_conflict_detected', async (event) => {
    dismissConfigNotification()
    takeoverConflictNotification.value = event.payload
    await store.loadSettings()
    await store.getProxyStatus()
  })

  // 监听后台检查到有新版本可用
  unlistenUpdateAvailable = await listen<UpdateInfo>('update-available', (event) => {
    updaterStore.onUpdateAvailable(event.payload)
  })

  // 监听更新下载进度
  unlistenUpdateProgress = await listen<{ downloadedBytes: number; totalBytes: number | null }>(
    'update-download-progress',
    (event) => {
      updaterStore.onDownloadProgress(event.payload.downloadedBytes, event.payload.totalBytes)
    }
  )
})

onUnmounted(() => {
  store.stopAutoRefresh()
  if (mediaQuery) {
    mediaQuery.removeEventListener('change', handleSystemThemeChange)
  }
  if (unlistenQuit) unlistenQuit()
  if (unlistenSourceDetected) unlistenSourceDetected()
  if (unlistenConfigChanged) unlistenConfigChanged()
  if (unlistenTakeoverConflict) unlistenTakeoverConflict()
  if (unlistenUpdateAvailable) unlistenUpdateAvailable()
  if (unlistenUpdateProgress) unlistenUpdateProgress()
  if (configChangedTimer) clearTimeout(configChangedTimer)
})
</script>

<template>
  <main class="app-shell relative flex h-full w-full flex-col overflow-hidden rounded-[23px] antialiased">
    <div class="app-shell__bg pointer-events-none absolute inset-0"></div>
    <div class="app-shell__hairline pointer-events-none absolute inset-x-5 top-0 h-px"></div>
    <div class="app-shell__divider pointer-events-none absolute inset-x-4 top-[78px] h-px"></div>
    <!-- Header -->
    <header class="relative shrink-0 flex flex-col gap-2 px-5 pt-3.5 pb-0.5 drag-region bg-transparent">
      <div class="flex items-center justify-between relative px-1">
        <!-- 标题（左侧） -->
        <div class="flex items-center gap-2 text-[1.05rem] font-bold tracking-tight text-[var(--theme-text-primary)]">
          <div class="relative flex items-center justify-center w-2.5 h-2.5">
            <div class="w-1.5 h-1.5 rounded-full bg-emerald-500 z-10"></div>
            <div class="absolute inset-0 rounded-full bg-emerald-400/20 shadow-[0_0_14px_rgba(16,185,129,0.48)]"></div>
          </div>
          UsageMeter
        </div>

        <!-- 操作按钮（右侧） -->
        <div class="flex items-center gap-1 shrink-0 drag-region-none" style="-webkit-app-region: no-drag; app-region: no-drag">
          <SourceSelector />
          <ToolSelector />
          <button @click="store.refreshUsageAndSessionViews()" class="theme-icon-button p-1.5 rounded-full transition-all select-none" :title="t(store.settings.locale, 'common.refresh')">
            <RefreshCw class="w-3.5 h-3.5" :class="{ 'animate-spin': store.loading }" />
          </button>

          <ThemeSelector />
        </div>
      </div>

      <!-- Segmented Control -->
      <nav class="segmented-control flex rounded-[18px] p-0.5 backdrop-blur-xl">
        <button
          v-for="item in navItems"
          :key="item.id"
          @click="currentView = item.id"
          :class="['flex-1 flex justify-center items-center py-1 rounded-[15px] text-xs font-semibold transition-all', currentView === item.id ? 'segmented-control__item segmented-control__item--active' : 'segmented-control__item segmented-control__item--idle']"
        >
          {{ t(store.settings.locale, item.key) }}
        </button>
      </nav>
    </header>

    <!-- View Content -->
    <div class="relative min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 pb-5 pt-1 no-scrollbar">
      <Overview v-if="currentView === 'overview'" />
      <Statistics v-else-if="currentView === 'statistics'" />
      <Sessions v-else-if="currentView === 'sessions'" />
      <Settings v-else-if="currentView === 'settings'" />
    </div>
    <div class="app-shell__fade-top pointer-events-none absolute inset-x-0 top-[78px] z-10 h-2"></div>
    <div class="app-shell__fade-bottom pointer-events-none absolute inset-x-0 bottom-0 z-10 h-9"></div>
  </main>

  <!-- 外部工具修改配置通知 Toast -->
  <UpdateDialog />

  <Transition name="toast-slide">
    <div
      v-if="takeoverConflictNotification"
      class="theme-toast theme-toast--warning fixed bottom-4 left-1/2 z-50 flex w-[calc(100%-32px)] max-w-[360px] -translate-x-1/2 items-start gap-2.5 rounded-2xl px-3.5 py-3 backdrop-blur-xl"
    >
      <div class="theme-toast__icon mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full">
        <ArrowLeftRight class="h-3 w-3 text-[var(--theme-status-warning-fg)]" />
      </div>
      <div class="min-w-0 flex-1">
        <p class="text-[11.5px] font-semibold leading-tight text-[var(--theme-status-warning-fg)]">
          {{ t(store.settings.locale, 'settings.takeoverConflictDetected') }}
        </p>
        <p class="mt-0.5 text-[10.5px] leading-snug text-[var(--theme-status-warning-fg)] opacity-80">
          {{ t(store.settings.locale, 'settings.takeoverConflictDetectedDesc') }}
        </p>
        <div class="mt-2 flex flex-wrap gap-1.5">
          <button
            class="rounded-lg bg-amber-600/15 px-2 py-1 text-[10px] font-semibold text-[var(--theme-status-warning-fg)] transition-colors hover:bg-amber-600/25"
            @click="resolveTakeoverConflict('force_reclaim')"
          >
            {{ t(store.settings.locale, 'settings.takeoverConflictForce') }}
          </button>
          <button
            class="rounded-lg bg-amber-600/10 px-2 py-1 text-[10px] font-semibold text-[var(--theme-status-warning-fg)] opacity-80 transition-colors hover:opacity-100"
            @click="resolveTakeoverConflict('disable_takeover')"
          >
            {{ t(store.settings.locale, 'settings.takeoverConflictDisable') }}
          </button>
        </div>
      </div>
      <button
        @click="dismissTakeoverConflictNotification"
        class="ml-1 shrink-0 text-[var(--theme-status-warning-fg)] opacity-60 transition-colors hover:opacity-100"
        :aria-label="t(store.settings.locale, 'common.cancel')"
      >
        <svg class="h-3.5 w-3.5" viewBox="0 0 12 12" fill="none">
          <path d="M1 1l10 10M11 1L1 11" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
        </svg>
      </button>
    </div>
  </Transition>

  <Transition name="toast-slide">
    <div
      v-if="configChangedNotification && !takeoverConflictNotification"
      class="theme-toast theme-toast--warning fixed bottom-4 left-1/2 z-50 flex w-[calc(100%-32px)] max-w-[360px] -translate-x-1/2 items-start gap-2.5 rounded-2xl px-3.5 py-3 backdrop-blur-xl"
    >
      <div class="theme-toast__icon mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full">
        <ArrowLeftRight class="h-3 w-3 text-[var(--theme-status-warning-fg)]" />
      </div>
      <div class="min-w-0 flex-1">
        <p class="text-[11.5px] font-semibold leading-tight text-[var(--theme-status-warning-fg)]">
          {{ t(store.settings.locale, 'settings.externalConfigChanged') }}
        </p>
        <p class="mt-0.5 truncate text-[10.5px] leading-tight text-[var(--theme-status-warning-fg)] opacity-80">
          {{ t(store.settings.locale, 'settings.externalConfigChangedDesc') }}
        </p>
      </div>
      <button
        @click="dismissConfigNotification"
        class="ml-1 shrink-0 text-[var(--theme-status-warning-fg)] opacity-60 transition-colors hover:opacity-100"
        :aria-label="t(store.settings.locale, 'common.cancel')"
      >
        <svg class="h-3.5 w-3.5" viewBox="0 0 12 12" fill="none">
          <path d="M1 1l10 10M11 1L1 11" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
        </svg>
      </button>
    </div>
  </Transition>
</template>

<style>
.app-shell {
  border: 1px solid var(--theme-border-default);
  background: var(--theme-bg-chrome);
  color: var(--theme-text-primary);
  box-shadow: var(--theme-shadow-card);
  backdrop-filter: blur(24px);
  -webkit-backdrop-filter: blur(24px);
  ring: 1px solid var(--theme-border-subtle);
}
.app-shell__bg {
  background: var(--theme-bg-shell-gradient);
}
.app-shell__hairline {
  background: var(--theme-effect-hairline);
}
.app-shell__divider {
  background: var(--theme-divider-default);
}
.app-shell__fade-top {
  background: var(--theme-effect-fade-top);
}
.app-shell__fade-bottom {
  background: var(--theme-effect-fade-bottom);
}
.theme-icon-button {
  color: var(--theme-text-tertiary);
}
.theme-icon-button:hover {
  background: var(--theme-bg-hover);
  color: var(--theme-text-primary);
  box-shadow: var(--theme-shadow-inline);
}
.segmented-control {
  border: 1px solid var(--theme-border-default);
  background: var(--theme-surface-muted-gradient);
  box-shadow: var(--theme-effect-segmented-shadow);
}
.segmented-control__item--active {
  background: var(--theme-overlay-gradient);
  color: var(--theme-text-primary);
  box-shadow: var(--theme-shadow-inline);
}
.segmented-control__item--idle {
  color: var(--theme-text-secondary);
}
.segmented-control__item--idle:hover {
  color: var(--theme-text-primary);
}
.theme-toast {
  border: 1px solid var(--theme-status-warning-border);
  background: var(--theme-status-warning-bg);
  box-shadow: var(--theme-shadow-overlay);
}
.theme-toast__icon {
  background: color-mix(in srgb, var(--theme-status-warning-fg) 14%, transparent);
}
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.15s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
.drag-region {
  -webkit-app-region: drag;
  app-region: drag;
}
.no-scrollbar::-webkit-scrollbar {
  display: none;
}
.no-scrollbar {
  -ms-overflow-style: none;
  scrollbar-width: none;
}
.toast-slide-enter-active,
.toast-slide-leave-active {
  transition: opacity 0.22s ease, transform 0.22s ease;
}
.toast-slide-enter-from,
.toast-slide-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(10px);
}
</style>
