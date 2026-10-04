<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { openDesktopWindow, openShareWindow } from './api/appApi'
import type { DesktopNavigationTarget } from './types'
import { resolveTakeoverConflict as resolveTakeoverConflictRequest } from './api/proxyApi'
import { useMonitorStore } from './stores/monitor'
import { useUpdaterStore } from './stores/updater'
import type { UpdateInfo } from './stores/updater'
import Overview from './views/Overview.vue'
import Statistics from './views/Statistics.vue'
import Sessions from './views/Sessions.vue'
import Settings from './views/Settings.vue'
import Gateway from './views/Gateway.vue'
import UpdateDialog from './components/UpdateDialog.vue'
import SegmentedControl from './components/SegmentedControl.vue'
import SourceSelector from './components/SourceSelector.vue'
import ToolSelector from './components/ToolSelector.vue'
import ThemeSelector from './components/ThemeSelector.vue'
import { applyResolvedTheme } from './theme'
import { RefreshCw, ArrowLeftRight, Share2, PanelTopOpen, ArrowUpRight } from 'lucide-vue-next'
import { t } from './i18n'
import { formatToolDisplayName } from './utils/toolDisplay'
import { quitApplication } from './utils/appExit'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()

const currentView = ref('overview')
const navItems = [
  { id: 'overview', key: 'common.dashboard' },
  { id: 'statistics', key: 'common.statistics' },
  { id: 'sessions', key: 'sessions.title' },
  { id: 'gateway', key: 'common.gateway' },
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
interface ExternalManagerPayload {
  tool: string
  manager: string
  config_path: string
}
interface CcSwitchCleanedPayload {
  cleaned: number
  unresolved: number
  backupPath?: string | null
}
const configChangedNotification = ref<ConfigChangedPayload | null>(null)
const takeoverConflictNotification = ref<TakeoverConflictPayload | null>(null)
const externalManagerNotification = ref<ExternalManagerPayload | null>(null)
const externalManagerReleasedNotification = ref<ExternalManagerPayload | null>(null)
const ccswitchCleanedNotification = ref<CcSwitchCleanedPayload | null>(null)
/** 礼让 toast 内"强制夺回"失败时的错误码（如 ccswitchProxyActive） */
const externalManagerReclaimErrorCode = ref<string | null>(null)
let configChangedTimer: ReturnType<typeof setTimeout> | null = null
let ccswitchCleanedTimer: ReturnType<typeof setTimeout> | null = null
let externalManagerReleasedTimer: ReturnType<typeof setTimeout> | null = null

const externalManagerReclaimErrorKey = computed(() => {
  const code = externalManagerReclaimErrorCode.value
  if (!code) return null
  return code.startsWith('ccswitchProxyActive')
    ? 'settings.ccswitch.errorProxyActive'
    : 'settings.ccswitch.errorGeneric'
})

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

function dismissExternalManagerNotification() {
  externalManagerNotification.value = null
  externalManagerReclaimErrorCode.value = null
}

function dismissExternalManagerReleasedNotification() {
  externalManagerReleasedNotification.value = null
  if (externalManagerReleasedTimer) {
    clearTimeout(externalManagerReleasedTimer)
    externalManagerReleasedTimer = null
  }
}

function dismissCcswitchCleanedNotification() {
  ccswitchCleanedNotification.value = null
  if (ccswitchCleanedTimer) {
    clearTimeout(ccswitchCleanedTimer)
    ccswitchCleanedTimer = null
  }
}

async function forceReclaimFromExternalManager() {
  const notification = externalManagerNotification.value
  if (!notification) {
    return
  }
  externalManagerReclaimErrorCode.value = null
  try {
    await resolveTakeoverConflictRequest(notification.tool, 'force_reclaim')
  } catch (error) {
    console.error('[App] Failed to force reclaim from external manager:', error)
    // 夺回失败（如 cc-switch 代理仍在运行）：保留 toast 并在其中提示原因
    externalManagerReclaimErrorCode.value = String(error)
    await store.getProxyStatus()
    return
  }
  await store.loadSettings()
  await store.getProxyStatus()
  dismissExternalManagerNotification()
}

async function openSharePanel() {
  await openShareWindow()
}

/** 打开（或聚焦）主窗口，不带深链目标。 */
async function openDesktop() {
  try {
    await openDesktopWindow()
  } catch (error) {
    console.error('[App] Failed to open desktop window:', error)
  }
}

/** 当前视图对应的主窗口深链目标（overview / statistics / sessions 提供“在主窗口查看”）。 */
const desktopTarget = computed<DesktopNavigationTarget | null>(() => {
  switch (currentView.value) {
    case 'overview':
      return { page: 'overview', window: store.settings.summaryWindow }
    case 'statistics':
      // 面板统计视图的窗口/指标是 Statistics.vue 组件局部状态（store 无通道），
      // 这里沿用汇总窗口并取默认指标（费用），主窗口可在此基础上继续下钻。
      return { page: 'analytics', window: store.settings.summaryWindow, metric: 'cost' }
    case 'sessions':
      return { page: 'sessions' }
    default:
      return null
  }
})

/** 深链：携带当前视图上下文打开主窗口。 */
async function openInDesktop() {
  const target = desktopTarget.value
  if (!target) return
  try {
    await openDesktopWindow(target)
  } catch (error) {
    console.error('[App] Failed to open desktop window with target:', error)
  }
}

async function resolveTakeoverConflict(action: 'force_reclaim' | 'pause' | 'disable_takeover') {
  const notification = takeoverConflictNotification.value
  if (!notification) {
    return
  }
  await resolveTakeoverConflictRequest(notification.tool, action)
  await store.loadSettings()
  await store.getProxyStatus()
  dismissTakeoverConflictNotification()
}

// 退出事件监听器
let unlistenQuit: UnlistenFn | null = null
let unlistenRefresh: UnlistenFn | null = null
let unlistenLocalUsageSynced: UnlistenFn | null = null
let unlistenSourceDetected: UnlistenFn | null = null
let unlistenConfigChanged: UnlistenFn | null = null
let unlistenTakeoverConflict: UnlistenFn | null = null
let unlistenExternalManagerDetected: UnlistenFn | null = null
let unlistenExternalManagerReleased: UnlistenFn | null = null
let unlistenCcswitchCleaned: UnlistenFn | null = null
let unlistenUpdateAvailable: UnlistenFn | null = null
let unlistenUpdateProgress: UnlistenFn | null = null
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

onMounted(async () => {
  // 先注册窗口事件，再初始化数据。初始化包含扫描和额度查询，若顺序相反，
  // 用户在启动阶段点击托盘图标时发出的刷新事件会被丢弃。
  unlistenQuit = await listen('app-quit-requested', async () => {
    try {
      await quitApplication(store)
    } catch (error) {
      console.error('[App] Failed to quit app from tray event:', error)
    }
  })

  // 托盘图标重新打开快速面板时，刷新当前用量和会话数据。
  unlistenRefresh = await listen('app-refresh', () => {
    requestWindowRefresh()
  })
  // 初次扫描在后台完成；完成后刷新快照，避免启动时只显示旧缓存或空数据。
  unlistenLocalUsageSynced = await listen('local_usage_synced', () => {
    requestWindowRefresh()
  })

  await store.initialize()
  refreshReady = true
  await flushPendingRefresh()
  store.startAutoRefresh()

  await nextTick()
  applyResolvedTheme(store.settings.theme)

  // 监听系统主题变化
  mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  mediaQuery.addEventListener('change', handleSystemThemeChange)

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

  // 检测到 cc-switch 内置代理接管，UsageMeter 已礼让
  unlistenExternalManagerDetected = await listen<ExternalManagerPayload>('external_manager_detected', async (event) => {
    dismissConfigNotification()
    externalManagerNotification.value = event.payload
    await store.getProxyStatus()
  })

  // cc-switch 内置代理关闭，UsageMeter 恢复接管：展示 5 秒自动消失的恢复提示
  unlistenExternalManagerReleased = await listen<ExternalManagerPayload>('external_manager_released', async (event) => {
    dismissExternalManagerNotification()
    externalManagerReleasedNotification.value = event.payload
    if (externalManagerReleasedTimer) clearTimeout(externalManagerReleasedTimer)
    externalManagerReleasedTimer = setTimeout(dismissExternalManagerReleasedNotification, 5000)
    await store.getProxyStatus()
  })

  // cc-switch 供应商库残留清理完成
  unlistenCcswitchCleaned = await listen<CcSwitchCleanedPayload>('ccswitch_db_cleaned', (event) => {
    if (event.payload.cleaned <= 0) {
      return
    }
    ccswitchCleanedNotification.value = event.payload
    if (ccswitchCleanedTimer) clearTimeout(ccswitchCleanedTimer)
    ccswitchCleanedTimer = setTimeout(dismissCcswitchCleanedNotification, 5000)
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
  if (unlistenRefresh) unlistenRefresh()
  if (unlistenLocalUsageSynced) unlistenLocalUsageSynced()
  if (unlistenSourceDetected) unlistenSourceDetected()
  if (unlistenConfigChanged) unlistenConfigChanged()
  if (unlistenTakeoverConflict) unlistenTakeoverConflict()
  if (unlistenExternalManagerDetected) unlistenExternalManagerDetected()
  if (unlistenExternalManagerReleased) unlistenExternalManagerReleased()
  if (unlistenCcswitchCleaned) unlistenCcswitchCleaned()
  if (unlistenUpdateAvailable) unlistenUpdateAvailable()
  if (unlistenUpdateProgress) unlistenUpdateProgress()
  if (configChangedTimer) clearTimeout(configChangedTimer)
  if (ccswitchCleanedTimer) clearTimeout(ccswitchCleanedTimer)
  if (externalManagerReleasedTimer) clearTimeout(externalManagerReleasedTimer)
})
</script>

<template>
  <main class="app-shell relative flex h-full w-full flex-col overflow-hidden rounded-[23px] antialiased">
    <div class="app-shell__bg pointer-events-none absolute inset-0"></div>
    <div class="app-shell__hairline pointer-events-none absolute inset-x-5 top-0 h-px"></div>
    <!-- Header -->
    <header class="relative shrink-0 flex flex-col gap-2 px-5 pt-3.5 pb-0.5 drag-region bg-transparent">
      <div class="flex items-center justify-between relative px-1">
        <!-- 标题（左侧） -->
        <div class="flex items-center gap-2 text-[1.05rem] font-bold tracking-tight text-[var(--theme-text-primary)]">
          <div class="relative flex items-center justify-center w-2.5 h-2.5">
            <div class="w-1.5 h-1.5 rounded-full bg-emerald-500 z-10"></div>
            <div class="absolute inset-0 rounded-full bg-emerald-400/20 shadow-[0_0_14px_rgba(16,185,129,0.48)]"></div>
          </div>
          {{ t(store.settings.locale, 'app.name') }}
        </div>

        <!-- 操作按钮（右侧） -->
        <div class="flex shrink-0 items-center gap-1.5 drag-region-none" style="-webkit-app-region: no-drag; app-region: no-drag">
          <!-- 数据范围：将两个筛选器收拢为一个视觉单元。 -->
          <div class="flex items-center gap-0.5 rounded-full border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)]/55 p-0.5">
            <SourceSelector />
            <ToolSelector />
          </div>

          <!-- 快捷操作：常驻动作统一收进紧凑工具条，分享直接触达。 -->
          <div class="flex items-center gap-0.5 rounded-full border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)]/55 p-0.5">
            <button @click="store.refreshUsageAndSessionViews()" class="theme-icon-button rounded-full p-1.5 transition-all select-none" :aria-label="t(store.settings.locale, 'common.refresh')" :title="t(store.settings.locale, 'common.refresh')">
              <RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': store.loading }" />
            </button>
            <ThemeSelector />
            <button @click="openDesktop()" class="theme-icon-button rounded-full p-1.5 transition-all select-none" :aria-label="t(store.settings.locale, 'desktop.open')" :title="t(store.settings.locale, 'desktop.open')">
              <PanelTopOpen class="h-3.5 w-3.5" />
            </button>
            <button @click="openSharePanel()" class="theme-icon-button rounded-full p-1.5 transition-all select-none" :aria-label="t(store.settings.locale, 'desktop.shareWindow')" :title="t(store.settings.locale, 'desktop.shareWindow')">
              <Share2 class="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      </div>

      <!-- Segmented Control -->
      <SegmentedControl
        tag="nav"
        :active-index="Math.max(0, navItems.findIndex(item => item.id === currentView))"
        class="segmented-control flex w-full rounded-[18px] p-0.5 backdrop-blur-xl"
      >
        <button
          v-for="item in navItems"
          :key="item.id"
          type="button"
          @click="currentView = item.id"
          :aria-current="currentView === item.id ? 'page' : undefined"
          :class="['flex-1 flex justify-center items-center py-1 rounded-[15px] text-xs font-semibold transition-colors duration-200', currentView === item.id ? 'segmented-control__item segmented-control__item--active' : 'segmented-control__item segmented-control__item--idle']"
        >
          {{ t(store.settings.locale, item.key) }}
        </button>
      </SegmentedControl>
    </header>

    <!-- View Content -->
    <div
      class="relative min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 pb-5 pt-1 no-scrollbar"
      style="-webkit-app-region: no-drag; app-region: no-drag"
    >
      <Overview v-if="currentView === 'overview'" />
      <Statistics v-else-if="currentView === 'statistics'" />
      <Sessions v-else-if="currentView === 'sessions'" />
      <Gateway v-else-if="currentView === 'gateway'" />
      <Settings v-else-if="currentView === 'settings'" />

      <!-- 深链：在主窗口查看（设计 3.2；仅概览 / 统计 / 会话三页提供） -->
      <div v-if="desktopTarget" class="flex justify-center pt-1">
        <button
          type="button"
          @click="openInDesktop()"
          class="inline-flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-xs font-medium text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
          :aria-label="t(store.settings.locale, 'desktop.viewInDesktop')"
        >
          <ArrowUpRight class="h-3.5 w-3.5" />
          {{ t(store.settings.locale, 'desktop.viewInDesktop') }}
        </button>
      </div>
    </div>
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
      v-if="externalManagerNotification && !takeoverConflictNotification"
      class="theme-toast theme-toast--warning fixed bottom-4 left-1/2 z-50 flex w-[calc(100%-32px)] max-w-[360px] -translate-x-1/2 items-start gap-2.5 rounded-2xl px-3.5 py-3 backdrop-blur-xl"
    >
      <div class="theme-toast__icon mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full">
        <ArrowLeftRight class="h-3 w-3 text-[var(--theme-status-warning-fg)]" />
      </div>
      <div class="min-w-0 flex-1">
        <p class="text-[11.5px] font-semibold leading-tight text-[var(--theme-status-warning-fg)]">
          {{ t(store.settings.locale, 'settings.ccswitch.yieldToastTitle') }}
        </p>
        <p class="mt-0.5 text-[10.5px] leading-snug text-[var(--theme-status-warning-fg)] opacity-80">
          {{ t(store.settings.locale, 'settings.ccswitch.yieldToastDesc', { tool: formatToolDisplayName(externalManagerNotification.tool, store.settings.locale, store.settings.clientTools.profiles) }) }}
        </p>
        <div class="mt-2 flex flex-wrap gap-1.5">
          <button
            class="rounded-lg bg-amber-600/15 px-2 py-1 text-[10px] font-semibold text-[var(--theme-status-warning-fg)] transition-colors hover:bg-amber-600/25"
            @click="forceReclaimFromExternalManager"
          >
            {{ t(store.settings.locale, 'settings.ccswitch.forceReclaim') }}
          </button>
        </div>
        <p
          v-if="externalManagerReclaimErrorKey"
          class="mt-1.5 text-[10px] leading-snug text-red-500"
        >
          {{ t(store.settings.locale, externalManagerReclaimErrorKey) }}
        </p>
      </div>
      <button
        @click="dismissExternalManagerNotification"
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
      v-if="externalManagerReleasedNotification && !takeoverConflictNotification && !externalManagerNotification"
      class="theme-toast fixed bottom-4 left-1/2 z-50 flex w-[calc(100%-32px)] max-w-[360px] -translate-x-1/2 items-start gap-2.5 rounded-2xl px-3.5 py-3 backdrop-blur-xl"
    >
      <div class="theme-toast__icon mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full">
        <ArrowLeftRight class="h-3 w-3 text-emerald-500" />
      </div>
      <div class="min-w-0 flex-1">
        <p class="text-[11.5px] font-semibold leading-tight text-[var(--theme-text-primary)]">
          {{ t(store.settings.locale, 'settings.ccswitch.releasedToastTitle') }}
        </p>
        <p class="mt-0.5 text-[10.5px] leading-snug text-[var(--theme-text-secondary)]">
          {{ t(store.settings.locale, 'settings.ccswitch.releasedToastDesc', { tool: formatToolDisplayName(externalManagerReleasedNotification.tool, store.settings.locale, store.settings.clientTools.profiles) }) }}
        </p>
      </div>
      <button
        @click="dismissExternalManagerReleasedNotification"
        class="ml-1 shrink-0 text-[var(--theme-text-tertiary)] opacity-60 transition-colors hover:opacity-100"
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
      v-if="ccswitchCleanedNotification && !takeoverConflictNotification && !externalManagerNotification && !externalManagerReleasedNotification"
      class="theme-toast fixed bottom-4 left-1/2 z-50 flex w-[calc(100%-32px)] max-w-[360px] -translate-x-1/2 items-start gap-2.5 rounded-2xl px-3.5 py-3 backdrop-blur-xl"
    >
      <div class="theme-toast__icon mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full">
        <ArrowLeftRight class="h-3 w-3 text-emerald-500" />
      </div>
      <div class="min-w-0 flex-1">
        <p class="text-[11.5px] font-semibold leading-tight text-[var(--theme-text-primary)]">
          {{ t(store.settings.locale, 'settings.ccswitch.cleanedToastTitle') }}
        </p>
        <p class="mt-0.5 text-[10.5px] leading-snug text-[var(--theme-text-secondary)]">
          {{ t(store.settings.locale, 'settings.ccswitch.cleanedToastDesc', { cleaned: ccswitchCleanedNotification.cleaned }) }}
        </p>
      </div>
      <button
        @click="dismissCcswitchCleanedNotification"
        class="ml-1 shrink-0 text-[var(--theme-text-tertiary)] opacity-60 transition-colors hover:opacity-100"
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
      v-if="configChangedNotification && !takeoverConflictNotification && !externalManagerNotification"
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
  isolation: isolate;
  border: 1px solid color-mix(in srgb, var(--theme-text-primary) 7%, transparent);
  background: color-mix(in srgb, var(--theme-text-primary) 10%, transparent);
  box-shadow: inset 0 0.5px 0 color-mix(in srgb, var(--theme-text-primary) 5%, transparent);
}
.segmented-control__item--active {
  background: transparent;
  color: var(--theme-accent-contrast);
}
.segmented-control__item--idle {
  color: var(--theme-text-tertiary);
}
.segmented-control__item--idle:hover {
  color: var(--theme-text-primary);
}
:root[data-appearance='dark'] .segmented-control {
  border: 1px solid var(--theme-border-default);
  background: var(--theme-dark-track-fill);
  box-shadow: none;
}
:root[data-appearance='dark'] .segmented-control__item--active {
  background: transparent;
  color: var(--theme-accent-contrast);
}
:root[data-appearance='dark'] .segmented-control__item--idle {
  color: var(--theme-dark-idle-label);
}
:root[data-appearance='dark'] .segmented-control__item--idle:hover {
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
