<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { Eye, EyeOff, RefreshCw, TestTube2 } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import type { RemoteSyncDevice, SyncStatus } from '../../types'
import ConfirmDialog from './ConfirmDialog.vue'
import SettingsSwitch from './SettingsSwitch.vue'

const store = useMonitorStore()

const localSyncEnabled = ref(store.settings.sync?.enabled ?? false)
const localSyncUrl = ref(store.settings.sync?.url ?? '')
const localSyncUsername = ref(store.settings.sync?.username ?? '')
const localSyncDeviceId = ref(store.settings.sync?.deviceId ?? '')
const localSyncIntervalMinutes = ref(store.settings.sync?.intervalMinutes ?? 15)
const localAutoSync = ref(store.settings.sync?.autoSync ?? false)
const webdavPassword = ref(store.settings.sync?.password ?? '')
const syncPassword = ref(store.settings.sync?.syncPassword ?? '')
const rotatePasswordExpanded = ref(false)
const rotateCurrentSyncPassword = ref('')
const rotateNewSyncPassword = ref('')
const rotateConfirmSyncPassword = ref('')
const rotatePasswordBusy = ref(false)
const rotatePasswordError = ref('')
const passwordFieldsFocused = ref(false)
const showWebdavPassword = ref(false)
const showSyncPassword = ref(false)
const showRotateCurrentPassword = ref(false)
const showRotateNewPassword = ref(false)
const showRotateConfirmPassword = ref(false)
const syncBusy = ref(false)
const syncStatus = ref<SyncStatus | null>(null)
const syncDevices = ref<RemoteSyncDevice[]>([])
const syncMessage = ref('')
const syncDeviceIdError = ref('')
const syncConfirmMode = ref<'remove-device' | 'clear-imported' | null>(null)
const syncConfirmDeviceId = ref('')
let syncStatusPollTimer: ReturnType<typeof setInterval> | null = null

const DEVICE_ID_MAX_LENGTH = 48
const syncIntervalPresets = [30, 60]

watch(() => store.settings.sync, (value) => {
  localSyncEnabled.value = value?.enabled ?? false
  localSyncUrl.value = value?.url ?? ''
  localSyncUsername.value = value?.username ?? ''
  localSyncDeviceId.value = value?.deviceId ?? ''
  localSyncIntervalMinutes.value = value?.intervalMinutes ?? 15
  localAutoSync.value = value?.autoSync ?? false
  if (!passwordFieldsFocused.value) {
    webdavPassword.value = value?.password ?? ''
    syncPassword.value = value?.syncPassword ?? ''
  }
}, { deep: true })

watch(localSyncEnabled, (enabled) => {
  if (enabled) {
    startSyncStatusPoll()
  } else {
    stopSyncStatusPoll()
  }
})

const isCustomSyncInterval = computed(() => !syncIntervalPresets.includes(Number(localSyncIntervalMinutes.value)))

const normalizeDeviceId = (value: string) => {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '')
    .slice(0, DEVICE_ID_MAX_LENGTH)
}

const validateDeviceId = (value: string) => {
  if (!value) return t(store.settings.locale, 'settings.syncDeviceIdRequired')
  if (value.length < 3) return t(store.settings.locale, 'settings.syncDeviceIdTooShort')
  if (value.length > DEVICE_ID_MAX_LENGTH) return t(store.settings.locale, 'settings.syncDeviceIdTooLong')
  if (!/^[a-z0-9._-]+$/.test(value)) return t(store.settings.locale, 'settings.syncDeviceIdInvalid')
  if (!/[a-z0-9]/.test(value)) return t(store.settings.locale, 'settings.syncDeviceIdInvalid')
  return ''
}

const applyDeviceIdInput = () => {
  const normalized = normalizeDeviceId(localSyncDeviceId.value)
  localSyncDeviceId.value = normalized
  syncDeviceIdError.value = validateDeviceId(normalized)
}

const saveSyncSettings = async () => {
  applyDeviceIdInput()
  if (syncDeviceIdError.value) {
    return false
  }
  store.settings.sync = {
    enabled: localSyncEnabled.value,
    provider: 'webdav',
    url: localSyncUrl.value.trim(),
    username: localSyncUsername.value.trim(),
    password: webdavPassword.value,
    syncPassword: syncPassword.value,
    deviceId: localSyncDeviceId.value,
    intervalMinutes: Math.max(1, Number(localSyncIntervalMinutes.value) || 15),
    autoSync: localAutoSync.value,
    includeSessionText: false
  }
  await store.saveSettings()
  return true
}

const resetRotatePasswordForm = () => {
  rotatePasswordExpanded.value = false
  rotateCurrentSyncPassword.value = ''
  rotateNewSyncPassword.value = ''
  rotateConfirmSyncPassword.value = ''
  rotatePasswordError.value = ''
  showRotateCurrentPassword.value = false
  showRotateNewPassword.value = false
  showRotateConfirmPassword.value = false
}

const syncCredentials = () => ({
  password: webdavPassword.value || store.settings.sync?.password || '',
  syncPassword: syncPassword.value || store.settings.sync?.syncPassword || ''
})

const syncErrorMessage = (error: unknown) => {
  const message = String(error)
  if (message.startsWith('ERR_SYNC_DEVICE_ID_CONFLICT')) return t(store.settings.locale, 'settings.syncDeviceIdConflict')
  if (message.startsWith('ERR_SYNC_DEVICE_ID_REQUIRED')) return t(store.settings.locale, 'settings.syncDeviceIdRequired')
  if (message.startsWith('ERR_SYNC_DEVICE_ID_TOO_SHORT')) return t(store.settings.locale, 'settings.syncDeviceIdTooShort')
  if (message.startsWith('ERR_SYNC_DEVICE_ID_TOO_LONG')) return t(store.settings.locale, 'settings.syncDeviceIdTooLong')
  if (message.startsWith('ERR_SYNC_DEVICE_ID_INVALID')) return t(store.settings.locale, 'settings.syncDeviceIdInvalid')
  if (message.startsWith('ERR_WEBDAV_URL_REQUIRED')) return t(store.settings.locale, 'settings.syncUrlRequired')
  if (message.startsWith('ERR_WEBDAV_USERNAME_REQUIRED')) return t(store.settings.locale, 'settings.syncUsernameRequired')
  if (message.startsWith('ERR_WEBDAV_PASSWORD_REQUIRED')) return t(store.settings.locale, 'settings.syncWebdavPasswordRequired')
  if (message.startsWith('ERR_SYNC_PASSWORD_TOO_SHORT')) return t(store.settings.locale, 'settings.syncPasswordTooShort')
  if (message.startsWith('ERR_SYNC_PASSWORD_ROTATION_REQUIRES_SYNC')) return t(store.settings.locale, 'settings.syncPasswordRotationRequiresSync')
  if (message.startsWith('ERR_SYNC_DECRYPT_FAILED')) return t(store.settings.locale, 'settings.syncPasswordIncorrect')
  if (message.startsWith('ERR_SYNC_KEYRING_MISSING')) return t(store.settings.locale, 'settings.syncPasswordRotationRequiresSync')
  if (message.startsWith('ERR_SYNC_LEGACY_PACKAGE_UNSUPPORTED')) return t(store.settings.locale, 'settings.syncLegacyPackageUnsupported')
  if (message.startsWith('ERR_WEBDAV_AUTH_FAILED')) return t(store.settings.locale, 'settings.syncAuthFailed')
  if (message.startsWith('ERR_WEBDAV_URL_INVALID')) return t(store.settings.locale, 'settings.syncUrlInvalid')
  if (message.startsWith('ERR_WEBDAV_REQUEST_FAILED')) return t(store.settings.locale, 'settings.syncRequestFailed')
  if (message.startsWith('ERR_WEBDAV_MKCOL_FAILED')) return t(store.settings.locale, 'settings.syncMkcolFailed')
  if (message.startsWith('ERR_WEBDAV_PUT_FAILED')) return t(store.settings.locale, 'settings.syncPutFailed')
  if (message.startsWith('ERR_WEBDAV_GET_FAILED')) return t(store.settings.locale, 'settings.syncGetFailed')
  if (message.startsWith('ERR_WEBDAV_PROPFIND_FAILED')) return t(store.settings.locale, 'settings.syncPropfindFailed')
  if (message.startsWith('ERR_SYNC_SCHEMA_UNSUPPORTED')) return t(store.settings.locale, 'settings.syncSchemaUnsupported')
  if (message.startsWith('ERR_SYNC_KEYRING_SCHEMA_UNSUPPORTED')) return t(store.settings.locale, 'settings.syncSchemaUnsupported')
  if (message.startsWith('ERR_SYNC_DEK_INVALID')) return t(store.settings.locale, 'settings.syncDekInvalid')
  if (message.startsWith('ERR_SYNC_BATCH_PRUNED_RETRY')) return t(store.settings.locale, 'settings.syncBatchPrunedRetry')
  if (message.startsWith('ERR_SYNC_BATCH_MISSING')) return t(store.settings.locale, 'settings.syncBatchMissing')
  if (message.startsWith('ERR_SYNC_BATCH_SEQ_MISMATCH') || message.startsWith('ERR_SYNC_BATCH_CHAIN_BROKEN')) {
    return t(store.settings.locale, 'settings.syncBatchChainBroken')
  }
  return message
}

const loadSyncStatus = async () => {
  try {
    syncStatus.value = await invoke<SyncStatus>('get_sync_status', { settings: store.settings })
  } catch {
    syncStatus.value = null
  }
}

const loadSyncDevices = async () => {
  try {
    syncDevices.value = await invoke<RemoteSyncDevice[]>('list_sync_devices')
  } catch {
    syncDevices.value = []
  }
}

const refreshActiveDeviceId = async () => {
  if (localSyncDeviceId.value) return
  try {
    const activeId = await invoke<string | null>('get_active_sync_device_id')
    if (activeId && !localSyncDeviceId.value) {
      localSyncDeviceId.value = activeId
      store.settings.sync.deviceId = activeId
    }
  } catch {
    // ignore
  }
}

const startSyncStatusPoll = () => {
  if (syncStatusPollTimer) return
  syncStatusPollTimer = setInterval(async () => {
    if (localSyncEnabled.value) {
      await loadSyncStatus()
    }
  }, 15_000)
}

const stopSyncStatusPoll = () => {
  if (syncStatusPollTimer) {
    clearInterval(syncStatusPollTimer)
    syncStatusPollTimer = null
  }
}

const testWebdav = async () => {
  if (!(await saveSyncSettings())) {
    syncMessage.value = syncDeviceIdError.value
    return
  }
  syncBusy.value = true
  syncMessage.value = ''
  try {
    await invoke('test_webdav_connection', { settings: store.settings, credentials: syncCredentials() })
    syncMessage.value = t(store.settings.locale, 'settings.syncTestSuccess')
    await loadSyncStatus()
  } catch (error) {
    syncMessage.value = syncErrorMessage(error)
  } finally {
    syncBusy.value = false
  }
}

const runWebdavSync = async () => {
  if (!(await saveSyncSettings())) {
    syncMessage.value = syncDeviceIdError.value
    return
  }
  syncBusy.value = true
  syncMessage.value = ''
  try {
    syncStatus.value = await invoke<SyncStatus>('sync_now', { settings: store.settings, credentials: syncCredentials() })
    syncMessage.value = t(store.settings.locale, 'settings.syncSuccess')
    await loadSyncDevices()
    await refreshActiveDeviceId()
    await store.refreshUsage()
  } catch (error) {
    syncMessage.value = syncErrorMessage(error)
  } finally {
    syncBusy.value = false
  }
}

const removeSyncDevice = (deviceId: string) => {
  syncConfirmMode.value = 'remove-device'
  syncConfirmDeviceId.value = deviceId
}

const clearImportedSyncData = () => {
  syncConfirmMode.value = 'clear-imported'
}

const closeSyncConfirm = () => {
  syncConfirmMode.value = null
  syncConfirmDeviceId.value = ''
}

const confirmSyncDanger = async () => {
  syncBusy.value = true
  syncMessage.value = ''
  try {
    if (syncConfirmMode.value === 'remove-device') {
      await invoke('remove_sync_device', { deviceId: syncConfirmDeviceId.value })
      syncMessage.value = t(store.settings.locale, 'settings.syncDeviceRemoved')
    } else if (syncConfirmMode.value === 'clear-imported') {
      await invoke('clear_imported_sync_data')
      syncMessage.value = t(store.settings.locale, 'settings.syncImportedCleared')
    }
    await loadSyncDevices()
    await loadSyncStatus()
    await store.refreshUsage()
  } catch (error) {
    syncMessage.value = syncErrorMessage(error)
  } finally {
    syncBusy.value = false
    closeSyncConfirm()
  }
}

const rotateSyncPassword = async () => {
  if (!(await saveSyncSettings())) {
    syncMessage.value = syncDeviceIdError.value
    return
  }
  rotatePasswordError.value = ''
  if (rotateCurrentSyncPassword.value.length < 8 || rotateNewSyncPassword.value.length < 8) {
    rotatePasswordError.value = t(store.settings.locale, 'settings.syncPasswordTooShort')
    return
  }
  if (rotateNewSyncPassword.value !== rotateConfirmSyncPassword.value) {
    rotatePasswordError.value = t(store.settings.locale, 'settings.syncPasswordConfirmMismatch')
    return
  }

  rotatePasswordBusy.value = true
  syncMessage.value = ''
  try {
    await invoke('rotate_sync_password', {
      settings: store.settings,
      credentials: {
        password: webdavPassword.value,
        syncPassword: ''
      },
      payload: {
        currentSyncPassword: rotateCurrentSyncPassword.value,
        newSyncPassword: rotateNewSyncPassword.value
      }
    })
    syncPassword.value = rotateNewSyncPassword.value
    store.settings.sync.syncPassword = rotateNewSyncPassword.value
    await store.saveSettings()
    syncMessage.value = t(store.settings.locale, 'settings.syncPasswordRotationSuccess')
    resetRotatePasswordForm()
  } catch (error) {
    rotatePasswordError.value = syncErrorMessage(error)
  } finally {
    rotatePasswordBusy.value = false
  }
}

const applySyncIntervalPreset = async (minutes: number) => {
  localSyncIntervalMinutes.value = minutes
  await saveSyncSettings()
}

const formatSyncTimestamp = (timestamp: number | null | undefined) => {
  if (!timestamp) {
    return t(store.settings.locale, 'settings.syncNever')
  }
  return new Date(timestamp * 1000).toLocaleString()
}

const handleGlobalKeydown = (event: KeyboardEvent) => {
  if (event.key === 'Escape') {
    if (syncConfirmMode.value) {
      closeSyncConfirm()
    } else if (rotatePasswordExpanded.value) {
      resetRotatePasswordForm()
    }
  }
}

onMounted(async () => {
  await loadSyncStatus()
  await loadSyncDevices()
  await refreshActiveDeviceId()
  if (localSyncEnabled.value) {
    startSyncStatusPoll()
  }
  window.addEventListener('keydown', handleGlobalKeydown)
})

onUnmounted(() => {
  stopSyncStatusPoll()
  window.removeEventListener('keydown', handleGlobalKeydown)
})
</script>

<template>
  <div class="p-3 px-4">
    <div class="mb-3 flex items-center justify-between gap-3">
      <div class="min-w-0">
        <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.syncWebdav') }}</div>
        <div class="mt-0.5 text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.syncWebdavDesc') }}</div>
      </div>
      <SettingsSwitch :checked="localSyncEnabled" @toggle="localSyncEnabled = !localSyncEnabled; saveSyncSettings()" />
    </div>

    <div v-if="localSyncEnabled" class="space-y-2.5 rounded-xl border border-gray-100 bg-gray-50/70 p-2 dark:border-neutral-800 dark:bg-neutral-900/60">
      <div class="flex items-start justify-between gap-2 rounded-xl bg-white/90 px-3 py-2 dark:bg-neutral-950/80">
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-gray-400 dark:text-gray-500">
            <span class="flex items-center gap-1">
              <span :class="[
                'inline-block h-1.5 w-1.5 shrink-0 rounded-full',
                syncStatus?.lastStatus === 'success' ? 'bg-green-500' :
                syncStatus?.lastStatus === 'failed' ? 'bg-red-500' : 'bg-gray-300 dark:bg-neutral-600'
              ]"></span>
              <span>{{ t(store.settings.locale, 'settings.syncLast') }} {{ formatSyncTimestamp(syncStatus?.lastSyncAt) }}</span>
            </span>
            <span>{{ t(store.settings.locale, 'settings.syncLocalCount') }} {{ syncStatus?.localRequestCount ?? 0 }}</span>
            <span>{{ t(store.settings.locale, 'settings.syncTotalCount') }} {{ syncStatus?.totalRequestCount ?? 0 }}</span>
            <template v-if="syncStatus && (syncStatus.uploadedRequests > 0 || syncStatus.importedRequests > 0)">
              <span>{{ t(store.settings.locale, 'settings.syncUploaded') }} {{ syncStatus.uploadedRequests }}</span>
              <span>{{ t(store.settings.locale, 'settings.syncImported') }} {{ syncStatus.importedRequests }}</span>
            </template>
          </div>
          <div
            v-if="syncStatus?.lastStatus === 'failed' && syncStatus?.lastError && !syncMessage"
            class="mt-1 truncate text-[10px] text-red-500 dark:text-red-400"
          >
            {{ syncErrorMessage(syncStatus.lastError) }}
          </div>
          <div
            v-if="syncMessage"
            class="mt-1 truncate text-[10px]"
            :class="syncMessage === t(store.settings.locale, 'settings.syncSuccess') || syncMessage === t(store.settings.locale, 'settings.syncTestSuccess')
              ? 'text-green-600 dark:text-green-400' : 'text-gray-500 dark:text-gray-400'"
          >
            {{ syncMessage }}
          </div>
        </div>
        <div class="flex shrink-0 items-center gap-1.5 self-start">
          <button
            class="inline-flex h-8 items-center gap-1.5 rounded-lg bg-gray-100 px-2.5 text-[11px] font-medium text-gray-600 transition-colors hover:bg-gray-200 disabled:opacity-50 dark:bg-neutral-800 dark:text-gray-300 dark:hover:bg-neutral-700"
            :disabled="syncBusy"
            @click="testWebdav"
          >
            <TestTube2 class="h-3.5 w-3.5" />
            <span>{{ t(store.settings.locale, 'settings.syncTest') }}</span>
          </button>
          <button
            class="inline-flex h-8 items-center gap-1.5 rounded-lg bg-blue-500 px-2.5 text-[11px] font-medium text-white transition-colors hover:bg-blue-600 disabled:opacity-50"
            :disabled="syncBusy"
            @click="runWebdavSync"
          >
            <RefreshCw :class="['h-3.5 w-3.5', syncBusy ? 'animate-spin' : '']" />
            <span>{{ syncBusy ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'settings.syncNow') }}</span>
          </button>
        </div>
      </div>

      <div class="space-y-1.5">
        <div class="rounded-xl border border-gray-100 bg-white px-3 py-2 dark:border-neutral-800 dark:bg-neutral-950">
          <div class="flex items-center justify-between gap-3">
            <div class="min-w-0">
              <div class="text-[11px] font-medium text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.syncAuto') }}</div>
              <div class="mt-0.5 text-[10px] text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.syncAutoDesc') }}</div>
            </div>
            <SettingsSwitch :checked="localAutoSync" @toggle="localAutoSync = !localAutoSync; saveSyncSettings()" />
          </div>
          <div v-if="localAutoSync" class="mt-2 border-t border-gray-100 pt-2 dark:border-neutral-800">
            <div class="flex items-center gap-3 rounded-lg px-0.5 py-0.5">
              <div class="w-[74px] shrink-0 text-[10px] font-medium text-gray-400 dark:text-gray-500">
                {{ t(store.settings.locale, 'settings.syncInterval') }}
              </div>
              <div class="flex min-w-0 flex-1 items-stretch gap-1.5">
                <button
                  v-for="minutes in syncIntervalPresets"
                  :key="minutes"
                  type="button"
                  :class="[
                    'w-[56px] shrink-0 rounded-md px-1.5 py-1 text-[10px] font-medium transition-colors',
                    Number(localSyncIntervalMinutes) === minutes
                      ? 'bg-blue-500 text-white'
                      : 'bg-gray-50 text-gray-600 hover:bg-gray-100 dark:bg-neutral-900 dark:text-gray-300 dark:hover:bg-neutral-800'
                  ]"
                  @click="applySyncIntervalPreset(minutes)"
                >
                  {{ minutes }}{{ t(store.settings.locale, 'settings.syncIntervalUnit') }}
                </button>
                <div
                  :class="[
                    'flex min-w-[108px] flex-1 items-center justify-center gap-1.5 rounded-md px-2.5 py-1 text-[10px] whitespace-nowrap transition-colors dark:bg-neutral-900',
                    isCustomSyncInterval
                      ? 'bg-blue-50 text-blue-600 dark:bg-blue-500/15 dark:text-blue-300'
                      : 'bg-gray-50 text-gray-500 dark:text-gray-400'
                  ]"
                >
                  <button
                    type="button"
                    class="shrink-0 font-medium"
                    @click="localSyncIntervalMinutes = Math.max(1, Number(localSyncIntervalMinutes) || 15)"
                  >
                    {{ t(store.settings.locale, 'common.custom') }}
                  </button>
                  <div class="flex min-w-0 items-center justify-center gap-1 whitespace-nowrap">
                    <input
                      v-model="localSyncIntervalMinutes"
                      type="number"
                      min="1"
                      max="1440"
                      :class="[
                        'w-8 shrink-0 bg-transparent text-right text-[10px] outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none',
                        isCustomSyncInterval ? 'text-blue-600 dark:text-blue-200' : 'text-gray-700 dark:text-gray-200'
                      ]"
                      @blur="saveSyncSettings"
                    />
                    <span class="shrink-0 whitespace-nowrap">{{ t(store.settings.locale, 'settings.syncIntervalUnit') }}</span>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div class="space-y-1 rounded-xl border border-gray-100 bg-white p-1 dark:border-neutral-800 dark:bg-neutral-950">
          <div class="flex min-h-[30px] items-center gap-2 rounded-lg px-1.5 py-0.5">
            <div class="w-[78px] shrink-0 whitespace-nowrap text-[9px] font-medium text-gray-400 dark:text-gray-500">
              {{ t(store.settings.locale, 'settings.syncUrl') }}
            </div>
            <input
              v-model="localSyncUrl"
              :placeholder="t(store.settings.locale, 'settings.syncUrl')"
              class="min-w-0 flex-1 bg-transparent py-0 text-xs leading-4 text-gray-700 outline-none dark:text-gray-200"
              @blur="saveSyncSettings"
            />
          </div>

          <div class="h-px bg-gray-100 dark:bg-neutral-800"></div>

          <div class="flex min-h-[30px] items-center gap-2 rounded-lg px-1.5 py-0.5">
            <div class="w-[78px] shrink-0 whitespace-nowrap text-[9px] font-medium text-gray-400 dark:text-gray-500">
              {{ t(store.settings.locale, 'settings.syncUsername') }}
            </div>
            <input
              v-model="localSyncUsername"
              :placeholder="t(store.settings.locale, 'settings.syncUsername')"
              class="min-w-0 flex-1 bg-transparent py-0 text-xs leading-4 text-gray-700 outline-none dark:text-gray-200"
              @blur="saveSyncSettings"
            />
          </div>

          <div class="h-px bg-gray-100 dark:bg-neutral-800"></div>

          <div :class="['rounded-lg px-1.5 py-0.5', syncDeviceIdError ? 'bg-red-50/70 dark:bg-red-950/10' : '']">
            <div class="flex min-h-[30px] items-center gap-2">
              <div class="w-[78px] shrink-0 whitespace-nowrap text-[9px] font-medium text-gray-400 dark:text-gray-500">
                {{ t(store.settings.locale, 'settings.syncDeviceId') }}
              </div>
              <input
                v-model="localSyncDeviceId"
                :placeholder="t(store.settings.locale, 'settings.syncDeviceId')"
                :class="[
                  'min-w-0 flex-1 bg-transparent py-0 text-xs leading-4 outline-none',
                  syncDeviceIdError ? 'text-red-500 dark:text-red-400' : 'text-gray-700 dark:text-gray-200'
                ]"
                @input="applyDeviceIdInput"
                @blur="saveSyncSettings"
              />
            </div>
            <div v-if="syncDeviceIdError" class="mt-0.5 pl-[88px] text-[10px] leading-relaxed text-red-500 dark:text-red-400">
              {{ syncDeviceIdError }}
            </div>
          </div>

          <div class="h-px bg-gray-100 dark:bg-neutral-800"></div>

          <div class="flex min-h-[30px] items-center gap-2 rounded-lg px-1.5 py-0.5">
            <div class="w-[78px] shrink-0 whitespace-nowrap text-[9px] font-medium text-gray-400 dark:text-gray-500">
              {{ t(store.settings.locale, 'settings.syncWebdavPassword') }}
            </div>
            <div class="relative min-w-0 flex-1">
              <input
                v-model="webdavPassword"
                :type="showWebdavPassword ? 'text' : 'password'"
                :placeholder="t(store.settings.locale, 'settings.syncWebdavPassword')"
                class="w-full bg-transparent py-0 pr-6 text-xs leading-4 text-gray-700 outline-none dark:text-gray-200"
                @focus="passwordFieldsFocused = true"
                @blur="passwordFieldsFocused = false; saveSyncSettings()"
              />
              <button
                type="button"
                class="absolute right-0 top-1/2 flex h-4.5 w-4.5 -translate-y-1/2 items-center justify-center rounded text-gray-400 transition-colors hover:text-gray-600 dark:text-gray-500 dark:hover:text-gray-300"
                :aria-label="t(store.settings.locale, showWebdavPassword ? 'settings.hidePassword' : 'settings.showPassword')"
                @mousedown.prevent
                @click="showWebdavPassword = !showWebdavPassword"
              >
                <EyeOff v-if="showWebdavPassword" class="h-3.5 w-3.5" :stroke-width="2.2" />
                <Eye v-else class="h-3.5 w-3.5" :stroke-width="2.2" />
              </button>
            </div>
          </div>

          <div class="h-px bg-gray-100 dark:bg-neutral-800"></div>

          <div class="flex min-h-[30px] items-center gap-2 rounded-lg px-1.5 py-0.5">
            <div class="w-[78px] shrink-0 whitespace-nowrap text-[9px] font-medium text-gray-400 dark:text-gray-500">
              {{ t(store.settings.locale, 'settings.syncEncryptPassword') }}
            </div>
            <div class="flex min-w-0 flex-1 items-center gap-1.5">
              <div class="relative min-w-0 flex-1">
                <input
                  v-model="syncPassword"
                  :type="showSyncPassword ? 'text' : 'password'"
                  :placeholder="t(store.settings.locale, 'settings.syncEncryptPassword')"
                  class="w-full bg-transparent py-0 pr-6 text-xs leading-4 text-gray-700 outline-none dark:text-gray-200"
                  @focus="passwordFieldsFocused = true"
                  @blur="passwordFieldsFocused = false; saveSyncSettings()"
                />
                <button
                  type="button"
                  class="absolute right-0 top-1/2 flex h-4.5 w-4.5 -translate-y-1/2 items-center justify-center rounded text-gray-400 transition-colors hover:text-gray-600 dark:text-gray-500 dark:hover:text-gray-300"
                  :aria-label="t(store.settings.locale, showSyncPassword ? 'settings.hidePassword' : 'settings.showPassword')"
                  @mousedown.prevent
                  @click="showSyncPassword = !showSyncPassword"
                >
                  <EyeOff v-if="showSyncPassword" class="h-3.5 w-3.5" :stroke-width="2.2" />
                  <Eye v-else class="h-3.5 w-3.5" :stroke-width="2.2" />
                </button>
              </div>
              <button
                type="button"
                class="inline-flex h-6 shrink-0 items-center rounded-md bg-blue-50 px-1.5 py-0 text-[10px] font-medium text-blue-500 transition-colors hover:bg-blue-100 dark:bg-blue-500/10 dark:text-blue-400 dark:hover:bg-blue-500/20"
                @click="rotatePasswordExpanded = !rotatePasswordExpanded; rotatePasswordError = ''"
              >
                {{ t(store.settings.locale, 'settings.syncPasswordChange') }}
              </button>
            </div>
          </div>
        </div>
      </div>

      <div
        v-if="rotatePasswordExpanded"
        class="space-y-2 rounded-xl border border-blue-100 bg-white/90 p-2.5 dark:border-blue-500/20 dark:bg-neutral-950/85"
      >
        <div class="text-[10px] text-gray-400 dark:text-gray-500">
          {{ t(store.settings.locale, 'settings.syncPasswordChangeDesc') }}
        </div>
        <div class="grid grid-cols-3 gap-2">
          <div class="space-y-1">
            <div class="text-[10px] font-medium text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.syncPasswordCurrent') }}</div>
            <div class="relative min-w-0">
              <input
                v-model="rotateCurrentSyncPassword"
                :type="showRotateCurrentPassword ? 'text' : 'password'"
                class="w-full rounded-lg border border-white/70 bg-white py-2 pl-3 pr-9 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-200"
              />
              <button
                type="button"
                class="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-md text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600 dark:text-gray-500 dark:hover:bg-neutral-800 dark:hover:text-gray-300"
                :aria-label="t(store.settings.locale, showRotateCurrentPassword ? 'settings.hidePassword' : 'settings.showPassword')"
                @mousedown.prevent
                @click="showRotateCurrentPassword = !showRotateCurrentPassword"
              >
                <EyeOff v-if="showRotateCurrentPassword" class="h-3.5 w-3.5" :stroke-width="2.2" />
                <Eye v-else class="h-3.5 w-3.5" :stroke-width="2.2" />
              </button>
            </div>
          </div>
          <div class="space-y-1">
            <div class="text-[10px] font-medium text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.syncPasswordNew') }}</div>
            <div class="relative min-w-0">
              <input
                v-model="rotateNewSyncPassword"
                :type="showRotateNewPassword ? 'text' : 'password'"
                class="w-full rounded-lg border border-white/70 bg-white py-2 pl-3 pr-9 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-200"
              />
              <button
                type="button"
                class="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-md text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600 dark:text-gray-500 dark:hover:bg-neutral-800 dark:hover:text-gray-300"
                :aria-label="t(store.settings.locale, showRotateNewPassword ? 'settings.hidePassword' : 'settings.showPassword')"
                @mousedown.prevent
                @click="showRotateNewPassword = !showRotateNewPassword"
              >
                <EyeOff v-if="showRotateNewPassword" class="h-3.5 w-3.5" :stroke-width="2.2" />
                <Eye v-else class="h-3.5 w-3.5" :stroke-width="2.2" />
              </button>
            </div>
          </div>
          <div class="space-y-1">
            <div class="text-[10px] font-medium text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.syncPasswordConfirm') }}</div>
            <div class="relative min-w-0">
              <input
                v-model="rotateConfirmSyncPassword"
                :type="showRotateConfirmPassword ? 'text' : 'password'"
                class="w-full rounded-lg border border-white/70 bg-white py-2 pl-3 pr-9 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-200"
              />
              <button
                type="button"
                class="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-md text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600 dark:text-gray-500 dark:hover:bg-neutral-800 dark:hover:text-gray-300"
                :aria-label="t(store.settings.locale, showRotateConfirmPassword ? 'settings.hidePassword' : 'settings.showPassword')"
                @mousedown.prevent
                @click="showRotateConfirmPassword = !showRotateConfirmPassword"
              >
                <EyeOff v-if="showRotateConfirmPassword" class="h-3.5 w-3.5" :stroke-width="2.2" />
                <Eye v-else class="h-3.5 w-3.5" :stroke-width="2.2" />
              </button>
            </div>
          </div>
        </div>
        <div class="flex items-center justify-between gap-2">
          <div v-if="rotatePasswordError" class="min-w-0 truncate text-[10px] text-red-500 dark:text-red-400">
            {{ rotatePasswordError }}
          </div>
          <div v-else class="text-[10px] text-gray-400 dark:text-gray-500">
            {{ t(store.settings.locale, 'settings.syncPasswordChangeHint') }}
          </div>
          <div class="flex shrink-0 gap-1.5">
            <button
              type="button"
              class="rounded-lg bg-gray-100 px-2.5 py-1.5 text-[11px] font-medium text-gray-600 transition-colors hover:bg-gray-200 dark:bg-neutral-800 dark:text-gray-300 dark:hover:bg-neutral-700"
              :disabled="rotatePasswordBusy"
              @click="resetRotatePasswordForm"
            >
              {{ t(store.settings.locale, 'common.cancel') }}
            </button>
            <button
              type="button"
              class="rounded-lg bg-blue-500 px-2.5 py-1.5 text-[11px] font-medium text-white transition-colors hover:bg-blue-600 disabled:opacity-50"
              :disabled="rotatePasswordBusy"
              @click="rotateSyncPassword"
            >
              {{ rotatePasswordBusy ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'common.confirm') }}
            </button>
          </div>
        </div>
      </div>

      <div class="space-y-2">
        <div class="flex items-center justify-between gap-2">
          <div class="text-[10px] font-semibold uppercase tracking-[0.08em] text-gray-400 dark:text-gray-500">
            {{ t(store.settings.locale, 'settings.syncSectionDevices') }}
          </div>
          <button
            type="button"
            class="rounded-lg bg-gray-100 px-2 py-1 text-[10px] font-medium text-gray-600 transition-colors hover:bg-gray-200 disabled:opacity-50 dark:bg-neutral-800 dark:text-gray-300 dark:hover:bg-neutral-700"
            :disabled="syncBusy"
            @click="clearImportedSyncData"
          >
            {{ t(store.settings.locale, 'settings.syncClearImported') }}
          </button>
        </div>
        <div v-if="!syncDevices.length" class="text-[10px] text-gray-400 dark:text-gray-500">
          {{ t(store.settings.locale, 'settings.syncNoDevices') }}
        </div>
        <div v-else class="space-y-1.5">
          <div
            v-for="device in syncDevices"
            :key="device.deviceId"
            class="flex items-center justify-between gap-2 rounded-xl border border-gray-100 bg-white px-2.5 py-2.5 dark:border-neutral-800 dark:bg-neutral-950"
          >
            <div class="min-w-0">
              <div class="truncate text-[11px] font-medium text-gray-700 dark:text-gray-200">
                {{ device.deviceId }}
              </div>
              <div class="truncate text-[10px] text-gray-400 dark:text-gray-500">
                {{ t(store.settings.locale, 'settings.syncRemoteBatch') }} {{ device.lastExportSeq }}<span v-if="device.lastSeenAt"> · {{ formatSyncTimestamp(device.lastSeenAt) }}</span>
              </div>
            </div>
            <button
              type="button"
              class="shrink-0 rounded-lg bg-gray-100 px-2 py-1 text-[10px] font-medium text-gray-600 transition-colors hover:bg-gray-200 disabled:opacity-50 dark:bg-neutral-800 dark:text-gray-300 dark:hover:bg-neutral-700"
              :disabled="syncBusy"
              @click="removeSyncDevice(device.deviceId)"
            >
              {{ t(store.settings.locale, 'settings.syncRemoveDevice') }}
            </button>
          </div>
        </div>
      </div>
    </div>

    <ConfirmDialog
      :open="!!syncConfirmMode"
      :title="t(store.settings.locale, syncConfirmMode === 'remove-device' ? 'settings.syncConfirmRemoveTitle' : 'settings.syncConfirmClearTitle')"
      :body="t(store.settings.locale, syncConfirmMode === 'remove-device' ? 'settings.syncConfirmRemoveBody' : 'settings.syncConfirmClearBody')"
      :confirm-label="syncBusy ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'common.confirm')"
      :cancel-label="t(store.settings.locale, 'common.cancel')"
      :busy="syncBusy"
      @cancel="closeSyncConfirm"
      @confirm="confirmSyncDanger"
    />
  </div>
</template>
