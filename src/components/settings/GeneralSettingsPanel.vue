<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../../stores/monitor'
import { useUpdaterStore } from '../../stores/updater'
import { t } from '../../i18n'
import SettingsSwitch from './SettingsSwitch.vue'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()

const appVersion = ref('')
const localLocale = ref(store.settings.locale)
const localRefreshInterval = ref(store.settings.refreshIntervalSeconds)
const autoStartEnabled = ref(store.settings.autoStart)
const checkUpdateFlash = ref(false)
let checkUpdateFlashTimer: ReturnType<typeof setTimeout> | null = null

watch(() => store.settings.locale, (value) => {
  localLocale.value = value
})

watch(() => store.settings.refreshIntervalSeconds, (value) => {
  localRefreshInterval.value = value
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

  if (updaterStore.hasUpdate) {
    updaterStore.openDialog()
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
  <div class="theme-settings-panel overflow-hidden rounded-xl border">

    <!-- 语言 -->
    <div class="flex items-center justify-between py-2 px-4 text-[13px]">
      <span class="text-[var(--theme-text-primary)]">{{ t(store.settings.locale, 'settings.locale') }}</span>
      <select
        v-model="localLocale"
        class="cursor-pointer appearance-none bg-transparent text-right text-sm tracking-tight text-[var(--theme-text-secondary)] outline-none"
        @change="handleLocaleChange"
      >
        <option value="zh-CN">{{ t(store.settings.locale, 'settings.zhCN') }}</option>
        <option value="zh-TW">{{ t(store.settings.locale, 'settings.zhTW') }}</option>
        <option value="en-US">{{ t(store.settings.locale, 'settings.enUS') }}</option>
      </select>
    </div>

    <!-- 开机自动启动 -->
    <div class="flex items-center justify-between py-2 px-4 text-[13px]">
      <div class="flex flex-col">
        <span class="text-[var(--theme-text-primary)]">{{ t(store.settings.locale, 'settings.autoStart') }}</span>
        <span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(store.settings.locale, 'settings.autoStartDesc') }}</span>
      </div>
      <SettingsSwitch :checked="autoStartEnabled" @toggle="toggleAutoStart" />
    </div>

    <!-- 数据刷新间隔 -->
    <div class="flex items-center justify-between py-2 px-4 text-[13px]">
      <div class="flex flex-col">
        <span class="text-[var(--theme-text-primary)]">{{ t(store.settings.locale, 'settings.refreshInterval') }}</span>
        <span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(store.settings.locale, 'settings.refreshIntervalDesc') }}</span>
      </div>
      <div class="flex items-center gap-1 rounded-lg border border-[var(--theme-border-default)] px-2 py-1">
        <input
          v-model.number="localRefreshInterval"
          type="number"
          min="5"
          max="300"
          class="w-8 bg-transparent p-0 text-right text-[12px] font-mono text-[var(--theme-text-secondary)] outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
          @blur="handleRefreshIntervalChange"
          @keyup.enter="handleRefreshIntervalChange"
        />
        <span class="text-[11px] text-[var(--theme-text-tertiary)]">{{ t(store.settings.locale, 'common.seconds') }}</span>
      </div>
    </div>

    <!-- 自动检查更新 -->
    <div class="flex items-center justify-between py-2 px-4 text-[13px]">
      <div class="flex flex-col">
        <span class="text-[var(--theme-text-primary)]">{{ t(store.settings.locale, 'settings.update.autoCheck') }}</span>
        <span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(store.settings.locale, 'settings.update.autoCheckDesc') }}</span>
      </div>
      <SettingsSwitch :checked="store.settings.autoCheckUpdate" @toggle="toggleAutoCheckUpdate" />
    </div>

    <!-- 当前版本（最后） -->
    <div class="flex items-center justify-between py-2 px-4 text-[13px]">
      <div class="flex flex-col">
        <div class="flex items-center gap-1.5 text-[var(--theme-text-secondary)]">
          <span>{{ t(store.settings.locale, 'settings.update.currentVersion') }}</span>
          <span v-if="appVersion" class="font-mono text-[12px]">v{{ appVersion }}</span>
        </div>
        <span
          v-if="updaterStore.hasUpdate && updaterStore.updateInfo"
          class="text-[10px] font-medium text-[var(--theme-status-info-fg)]"
        >
          {{ t(store.settings.locale, 'settings.update.newVersionReady', { version: updaterStore.updateInfo.version }) }}
        </span>
      </div>
      <button
        class="theme-action-button h-6 rounded-lg border px-2.5 text-[11px] transition-colors disabled:opacity-50"
        :disabled="updaterStore.status === 'checking'"
        @click="handleCheckUpdate"
      >
        <span v-if="updaterStore.status === 'checking'">{{ t(store.settings.locale, 'settings.update.checking') }}</span>
        <span v-else-if="updaterStore.hasUpdate">{{ t(store.settings.locale, 'settings.update.viewUpdate') }}</span>
        <span v-else-if="checkUpdateFlash" class="text-green-500">✓ {{ t(store.settings.locale, 'settings.update.upToDate') }}</span>
        <span v-else-if="updaterStore.status === 'error'" class="text-red-400">{{ t(store.settings.locale, updaterStore.errorMessage === 'downloadFailed' ? 'settings.update.downloadFailed' : 'settings.update.checkFailed') }}</span>
        <span v-else>{{ t(store.settings.locale, 'settings.update.checkNow') }}</span>
      </button>
    </div>

  </div>
</template>

<style scoped>
.theme-settings-panel {
  background: var(--theme-surface-gradient);
  border-color: var(--theme-border-default);
  box-shadow: var(--theme-shadow-inline);
}

.theme-settings-panel > :not([hidden]) ~ :not([hidden]) {
  border-top-width: 1px;
  border-color: color-mix(in srgb, var(--theme-border-default) 52%, white 48%);
}

.theme-action-button {
  border-color: var(--theme-border-default);
  color: var(--theme-text-secondary);
}

.theme-action-button:hover {
  background: var(--theme-surface-muted-gradient);
}
</style>
