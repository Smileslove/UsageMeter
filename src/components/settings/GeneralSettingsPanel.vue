<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../../stores/monitor'
import { useUpdaterStore } from '../../stores/updater'
import { t } from '../../i18n'
import { type ThemeMode } from '../../types'
import SettingsSwitch from './SettingsSwitch.vue'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()

const appVersion = ref('')
const localLocale = ref(store.settings.locale)
const localRefreshInterval = ref(store.settings.refreshIntervalSeconds)
const localTheme = ref<ThemeMode>(store.settings.theme || 'system')
const autoStartEnabled = ref(store.settings.autoStart)
const checkUpdateFlash = ref(false)
let checkUpdateFlashTimer: ReturnType<typeof setTimeout> | null = null

watch(() => store.settings.locale, (value) => {
  localLocale.value = value
})

watch(() => store.settings.refreshIntervalSeconds, (value) => {
  localRefreshInterval.value = value
})

watch(() => store.settings.theme, (value) => {
  localTheme.value = value || 'system'
})

const handleLocaleChange = async () => {
  store.settings.locale = localLocale.value
  await store.saveSettings()
}

const handleRefreshIntervalChange = async () => {
  const value = Math.max(5, Math.min(300, Number(localRefreshInterval.value) || 30))
  localRefreshInterval.value = value
  store.settings.refreshIntervalSeconds = value
  await store.saveSettings()
}

const handleThemeChange = async () => {
  store.settings.theme = localTheme.value
  await store.saveSettings()
}

const toggleAutoStart = async () => {
  try {
    if (autoStartEnabled.value) {
      await invoke('disable_autostart')
      autoStartEnabled.value = false
    } else {
      await invoke('enable_autostart')
      autoStartEnabled.value = true
    }

    store.settings.autoStart = autoStartEnabled.value
    await store.saveSettings()
  } catch {
    try {
      autoStartEnabled.value = await invoke('is_autostart_enabled')
    } catch {
      autoStartEnabled.value = store.settings.autoStart
    }
  }
}

const toggleAutoCheckUpdate = async () => {
  store.settings.autoCheckUpdate = !store.settings.autoCheckUpdate
  await store.saveSettings()
}

const handleCheckUpdate = async () => {
  if (updaterStore.status === 'checking') {
    return
  }

  await updaterStore.checkForUpdate()
  if (updaterStore.status === 'idle') {
    checkUpdateFlash.value = true
    if (checkUpdateFlashTimer) {
      clearTimeout(checkUpdateFlashTimer)
    }
    checkUpdateFlashTimer = setTimeout(() => {
      checkUpdateFlash.value = false
    }, 2000)
  }
}

onMounted(async () => {
  try {
    const systemState = await invoke<boolean>('is_autostart_enabled')
    autoStartEnabled.value = systemState
    if (store.settings.autoStart !== systemState) {
      store.settings.autoStart = systemState
      await store.saveSettings()
    }
  } catch {
    autoStartEnabled.value = store.settings.autoStart
  }

  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    appVersion.value = await getVersion()
  } catch {
    appVersion.value = ''
  }
})

onUnmounted(() => {
  if (checkUpdateFlashTimer) {
    clearTimeout(checkUpdateFlashTimer)
  }
})
</script>

<template>
  <div class="space-y-2">
    <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">
      {{ t(store.settings.locale, 'settings.general') }}
    </h3>
    <div class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm divide-y divide-gray-50 dark:border-neutral-800 dark:bg-[#1C1C1E] dark:divide-neutral-800/50">
      <div class="flex items-center justify-between p-3 px-4 text-[13px]">
        <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.locale') }}</span>
        <select
          v-model="localLocale"
          class="cursor-pointer appearance-none bg-transparent text-right text-sm tracking-tight text-gray-500 outline-none dark:text-gray-400"
          @change="handleLocaleChange"
        >
          <option value="zh-CN">{{ t(store.settings.locale, 'settings.zhCN') }}</option>
          <option value="zh-TW">{{ t(store.settings.locale, 'settings.zhTW') }}</option>
          <option value="en-US">{{ t(store.settings.locale, 'settings.enUS') }}</option>
        </select>
      </div>

      <div class="flex items-center justify-between p-3 px-4 text-[13px]">
        <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.theme') }}</span>
        <div class="flex gap-1.5">
          <button
            v-for="theme in ['light', 'dark', 'system'] as ThemeMode[]"
            :key="theme"
            :class="[
              'rounded-md px-2.5 py-1 text-xs font-medium transition-all',
              localTheme === theme
                ? 'bg-blue-500 text-white'
                : 'bg-gray-100 text-gray-600 hover:bg-gray-200 dark:bg-neutral-700 dark:text-gray-400 dark:hover:bg-neutral-600'
            ]"
            @click="localTheme = theme; handleThemeChange()"
          >
            {{ t(store.settings.locale, `settings.theme${theme.charAt(0).toUpperCase() + theme.slice(1)}`) }}
          </button>
        </div>
      </div>

      <div class="flex items-center justify-between p-3 px-4 text-[13px]">
        <div class="flex flex-col">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.autoStart') }}</span>
          <span class="mt-0.5 text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.autoStartDesc') }}</span>
        </div>
        <SettingsSwitch :checked="autoStartEnabled" @toggle="toggleAutoStart" />
      </div>

      <div class="flex items-center justify-between p-3 px-4 text-[13px]">
        <div class="flex flex-col">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.update.autoCheck') }}</span>
          <span class="mt-0.5 text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.update.autoCheckDesc') }}</span>
        </div>
        <SettingsSwitch :checked="store.settings.autoCheckUpdate" @toggle="toggleAutoCheckUpdate" />
      </div>

      <div class="flex items-center justify-between p-3 px-4 text-[13px]">
        <span class="text-gray-500 dark:text-gray-400">
          {{ t(store.settings.locale, 'settings.update.currentVersion') }}
          <span v-if="appVersion" class="ml-1 font-mono text-[12px]">v{{ appVersion }}</span>
        </span>
        <button
          class="h-7 rounded-lg border border-gray-200 px-3 text-[11px] text-gray-600 transition-colors hover:bg-gray-50 disabled:opacity-50 dark:border-neutral-700 dark:text-gray-400 dark:hover:bg-neutral-800"
          :disabled="updaterStore.status === 'checking'"
          @click="handleCheckUpdate"
        >
          <span v-if="updaterStore.status === 'checking'">{{ t(store.settings.locale, 'settings.update.checking') }}</span>
          <span v-else-if="checkUpdateFlash" class="text-green-500">✓ {{ t(store.settings.locale, 'settings.update.upToDate') }}</span>
          <span v-else-if="updaterStore.status === 'error'" class="text-red-400">{{ t(store.settings.locale, updaterStore.errorMessage === 'downloadFailed' ? 'settings.update.downloadFailed' : 'settings.update.checkFailed') }}</span>
          <span v-else>{{ t(store.settings.locale, 'settings.update.checkNow') }}</span>
        </button>
      </div>

      <div class="flex items-center justify-between p-3 px-4 text-[13px]">
        <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.refreshInterval') }}</span>
        <div class="flex items-center gap-1">
          <input
            v-model.number="localRefreshInterval"
            type="number"
            min="5"
            max="300"
            class="w-12 bg-transparent p-0 text-right text-sm font-mono text-gray-500 outline-none dark:text-gray-400"
            @blur="handleRefreshIntervalChange"
            @keyup.enter="handleRefreshIntervalChange"
          />
          <span class="text-xs text-gray-400">{{ t(store.settings.locale, 'common.seconds') }}</span>
        </div>
      </div>
    </div>
  </div>
</template>
