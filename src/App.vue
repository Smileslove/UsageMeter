<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from './stores/monitor'
import Overview from './views/Overview.vue'
import Statistics from './views/Statistics.vue'
import Sessions from './views/Sessions.vue'
import Settings from './views/Settings.vue'
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
})

onUnmounted(() => {
  store.stopAutoRefresh()
  if (mediaQuery) {
    mediaQuery.removeEventListener('change', handleSystemThemeChange)
  }
  if (unlistenQuit) {
    unlistenQuit()
  }
})
</script>

<template>
  <main class="w-full h-full overflow-hidden bg-[#F4F6F4] flex flex-col text-gray-800 antialiased dark:bg-[#09090B] dark:text-gray-200 shadow-xl rounded-[14px] border border-gray-200/80 dark:border-neutral-800/80">
    <!-- Header -->
    <header class="pt-3 pb-1.5 px-4 shrink-0 flex flex-col gap-2 drag-region bg-transparent">
      <div class="flex items-center justify-between relative px-1">
        <!-- 标题（左侧） -->
        <div class="font-extrabold text-[1.05rem] text-gray-800 dark:text-gray-100 tracking-tight flex items-center gap-2">
          <div class="relative flex items-center justify-center w-2.5 h-2.5">
            <div class="w-1.5 h-1.5 rounded-full bg-green-500 z-10 animate-pulse"></div>
            <div class="absolute inset-0 rounded-full border-2 border-green-500/30 animate-pulse shadow-[0_0_8px_rgba(34,197,94,0.4)]"></div>
          </div>
          UsageMeter
        </div>

        <!-- 操作按钮（右侧） -->
        <div class="flex items-center gap-1 shrink-0 drag-region-none" style="-webkit-app-region: no-drag; app-region: no-drag">
          <button @click="store.refreshUsage()" class="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-200/60 dark:hover:text-gray-200 dark:hover:bg-neutral-800/80 transition-all select-none" :title="t(store.settings.locale, 'common.refresh')">
            <RefreshCw class="w-3.5 h-3.5" :class="{ 'animate-spin': store.loading }" />
          </button>

          <button @click="toggleTheme" class="p-1.5 rounded-lg text-gray-400 hover:text-gray-700 hover:bg-gray-200/60 dark:hover:text-gray-200 dark:hover:bg-neutral-800/80 transition-all select-none cursor-pointer" :title="t(store.settings.locale, 'common.toggleTheme')">
            <Sun v-if="store.settings.theme === 'light'" class="w-3.5 h-3.5 text-amber-500" />
            <Moon v-else-if="store.settings.theme === 'dark'" class="w-3.5 h-3.5 text-indigo-400" />
            <Monitor v-else class="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      <!-- Segmented Control -->
      <nav class="flex p-1 bg-gray-200/60 dark:bg-neutral-800/80 rounded-lg">
        <button
          v-for="item in navItems"
          :key="item.id"
          @click="currentView = item.id"
          :class="['flex-1 flex justify-center items-center py-1 rounded-md text-xs font-medium transition-all', currentView === item.id ? 'bg-white text-gray-900 shadow-sm dark:bg-[#1C1C1E] dark:text-gray-100' : 'text-gray-500 hover:text-gray-700 dark:hover:text-gray-300']"
        >
          {{ t(store.settings.locale, item.key) }}
        </button>
      </nav>
    </header>

    <!-- View Content -->
    <div class="flex-1 overflow-y-auto px-4 pb-4 pt-1.5 relative no-scrollbar">
      <Overview v-if="currentView === 'overview'" />
      <Statistics v-else-if="currentView === 'statistics'" />
      <Sessions v-else-if="currentView === 'sessions'" />
      <Settings v-else-if="currentView === 'settings'" />
    </div>
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
