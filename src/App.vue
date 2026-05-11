<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from './stores/monitor'
import Overview from './views/Overview.vue'
import Statistics from './views/Statistics.vue'
import Sessions from './views/Sessions.vue'
import Settings from './views/Settings.vue'
import SourceSelector from './components/SourceSelector.vue'
import ToolSelector from './components/ToolSelector.vue'
import type { ThemeMode } from './types'
import { RefreshCw, Sun, Moon, Monitor } from 'lucide-vue-next'
import { t } from './i18n'

const store = useMonitorStore()

const currentView = ref('overview')
const navItems = [
  { id: 'overview', key: 'common.dashboard' },
  { id: 'statistics', key: 'common.statistics' },
  { id: 'sessions', key: 'sessions.title' },
  { id: 'settings', key: 'common.settings' }
]

// 主题相关逻辑
const isDark = ref(false)

// 切换主题
const toggleTheme = () => {
  const themes: ThemeMode[] = ['light', 'dark', 'system']
  const currentIndex = themes.indexOf(store.settings.theme || 'system')
  const nextTheme = themes[(currentIndex + 1) % themes.length]
  store.settings.theme = nextTheme
  store.saveSettings()
}

// 检测系统主题偏好
const systemPrefersDark = () => {
  return window.matchMedia('(prefers-color-scheme: dark)').matches
}

// 应用主题
const applyTheme = (theme: ThemeMode) => {
  if (theme === 'dark') {
    isDark.value = true
  } else if (theme === 'light') {
    isDark.value = false
  } else {
    // 跟随系统
    isDark.value = systemPrefersDark()
  }

  // 更新 html class
  if (isDark.value) {
    document.documentElement.classList.add('dark')
  } else {
    document.documentElement.classList.remove('dark')
  }
}

// 监听系统主题变化
let mediaQuery: MediaQueryList | null = null
const handleSystemThemeChange = () => {
  if (store.settings.theme === 'system') {
    applyTheme('system')
  }
}

// 监听设置中的主题变化
watch(
  () => store.settings.theme,
  newTheme => {
    if (newTheme) {
      applyTheme(newTheme)
    }
  }
)

// 退出事件监听器
let unlistenQuit: UnlistenFn | null = null
let unlistenSourceDetected: UnlistenFn | null = null

onMounted(async () => {
  await store.initialize()
  store.startAutoRefresh()

  // 初始化主题（确保在 store 加载完成后）
  await nextTick()
  applyTheme(store.settings.theme || 'system')

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
})

onUnmounted(() => {
  store.stopAutoRefresh()
  if (mediaQuery) {
    mediaQuery.removeEventListener('change', handleSystemThemeChange)
  }
  if (unlistenQuit) {
    unlistenQuit()
  }
  if (unlistenSourceDetected) {
    unlistenSourceDetected()
  }
})
</script>

<template>
  <main class="relative flex h-full w-full flex-col overflow-hidden rounded-[23px] border border-white/68 bg-[#E7EBEF]/82 text-gray-800 shadow-[0_18px_52px_rgba(15,23,42,0.15)] backdrop-blur-2xl antialiased ring-1 ring-black/[0.035] dark:border-white/12 dark:bg-[#111216]/84 dark:text-gray-200 dark:shadow-[0_22px_64px_rgba(0,0,0,0.42)] dark:ring-white/[0.04]">
    <div class="pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_18%_0%,rgba(255,255,255,0.88),transparent_32%),radial-gradient(circle_at_86%_12%,rgba(203,213,225,0.42),transparent_32%),linear-gradient(180deg,rgba(248,250,252,0.36),rgba(226,232,240,0.26)_42%,rgba(203,213,225,0.36))] dark:bg-[radial-gradient(circle_at_20%_0%,rgba(255,255,255,0.12),transparent_32%),radial-gradient(circle_at_86%_12%,rgba(59,130,246,0.13),transparent_32%),linear-gradient(180deg,rgba(35,35,40,0.48),rgba(17,18,21,0.24)_42%,rgba(8,8,10,0.4))]"></div>
    <div class="pointer-events-none absolute inset-x-5 top-0 h-px bg-white/90 dark:bg-white/18"></div>
    <div class="pointer-events-none absolute inset-x-4 top-[78px] h-px bg-white/46 dark:bg-white/8"></div>
    <!-- Header -->
    <header class="relative shrink-0 flex flex-col gap-2 px-5 pt-3.5 pb-0.5 drag-region bg-transparent">
      <div class="flex items-center justify-between relative px-1">
        <!-- 标题（左侧） -->
        <div class="font-bold text-[1.05rem] text-slate-800 dark:text-gray-100 tracking-tight flex items-center gap-2">
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
          <button @click="store.refreshUsageAndSessionViews()" class="p-1.5 rounded-full text-slate-400 transition-all select-none hover:bg-white/70 hover:text-slate-700 hover:shadow-[0_1px_6px_rgba(15,23,42,0.08)] dark:text-white/36 dark:hover:bg-white/10 dark:hover:text-gray-200 dark:hover:shadow-none" :title="t(store.settings.locale, 'common.refresh')">
            <RefreshCw class="w-3.5 h-3.5" :class="{ 'animate-spin': store.loading }" />
          </button>

          <button @click="toggleTheme" class="p-1.5 rounded-full text-slate-400 transition-all select-none cursor-pointer hover:bg-white/70 hover:text-slate-700 hover:shadow-[0_1px_6px_rgba(15,23,42,0.08)] dark:text-white/36 dark:hover:bg-white/10 dark:hover:text-gray-200 dark:hover:shadow-none" :title="t(store.settings.locale, 'common.toggleTheme')">
            <Sun v-if="store.settings.theme === 'light'" class="w-3.5 h-3.5 text-amber-500" />
            <Moon v-else-if="store.settings.theme === 'dark'" class="w-3.5 h-3.5 text-indigo-400" />
            <Monitor v-else class="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      <!-- Segmented Control -->
      <nav class="flex rounded-[18px] border border-white/68 bg-white/44 p-0.5 shadow-[inset_0_1px_0_rgba(255,255,255,0.56),0_5px_14px_rgba(15,23,42,0.04)] backdrop-blur-xl dark:border-white/10 dark:bg-white/[0.06] dark:shadow-[inset_0_1px_0_rgba(255,255,255,0.06)]">
        <button
          v-for="item in navItems"
          :key="item.id"
          @click="currentView = item.id"
          :class="['flex-1 flex justify-center items-center py-1 rounded-[15px] text-xs font-semibold transition-all', currentView === item.id ? 'bg-white/92 text-slate-900 shadow-[0_2px_7px_rgba(15,23,42,0.08)] dark:bg-white/14 dark:text-gray-100 dark:shadow-none' : 'text-slate-500 hover:text-slate-700 dark:text-gray-400 dark:hover:text-gray-200']"
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
    <div class="pointer-events-none absolute inset-x-0 top-[78px] z-10 h-2 bg-gradient-to-b from-[#E7EBEF]/72 to-transparent dark:from-[#111216]/72"></div>
    <div class="pointer-events-none absolute inset-x-0 bottom-0 z-10 h-9 bg-gradient-to-t from-[#E7EBEF]/92 to-transparent dark:from-[#111216]/92"></div>
  </main>
</template>

<style>
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
</style>
