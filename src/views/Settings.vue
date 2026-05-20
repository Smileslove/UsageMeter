<script setup lang="ts">
import { ref, watch, computed, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { type DataSource, type ThemeMode, type ToolTakeoverStatus, type SyncStatus, type RemoteSyncDevice } from '../types'
import ModelPricingSettings from '../components/ModelPricingSettings.vue'
import ApiSourceList from '../components/ApiSourceList.vue'
import CurrencySettings from '../components/CurrencySettings.vue'
import LobeIcon from '../components/LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'
import { Eye, EyeOff } from 'lucide-vue-next'

const store = useMonitorStore()

// 子页面状态
const subView = ref<'main' | 'model-pricing' | 'api-sources' | 'currency'>('main')

// 监听 subView 变化，触发子组件刷新
const modelPricingKey = ref(0)
watch(subView, (newVal) => {
  if (newVal === 'model-pricing') {
    // 强制重新创建组件
    modelPricingKey.value++
  }
})

// 返回主设置
const goBack = () => {
  subView.value = 'main'
}

// 进入模型价格设置
const openModelPricing = () => {
  subView.value = 'model-pricing'
}

// 进入 API 来源管理
const openApiSources = () => {
  subView.value = 'api-sources'
}

// 进入货币设置
const openCurrency = () => {
  subView.value = 'currency'
}

// 本地状态用于双向绑定
const localLocale = ref(store.settings.locale)
const localRefreshInterval = ref(store.settings.refreshIntervalSeconds)
const localDataSource = ref<DataSource>(store.settings.dataSource)
const localTheme = ref<ThemeMode>(store.settings.theme || 'system')
const localIncludeErrorRequests = ref(store.settings.proxy.includeErrorRequests ?? true)
const localSyncEnabled = ref(store.settings.sync?.enabled ?? false)
const localSyncUrl = ref(store.settings.sync?.url ?? '')
const localSyncUsername = ref(store.settings.sync?.username ?? '')
const localSyncDeviceId = ref(store.settings.sync?.deviceId ?? '')
const localSyncIntervalMinutes = ref(store.settings.sync?.intervalMinutes ?? 15)
const localSyncOnStartup = ref(store.settings.sync?.syncOnStartup ?? false)
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
const takeoverStatuses = ref<ToolTakeoverStatus[]>([])
const takeoverLoading = ref<Record<string, boolean>>({})
const showOfficialApiWarning = ref(false)
let resolveOfficialApiWarning: ((accepted: boolean) => void) | null = null

// 开机自启动状态（从配置初始化，页面加载后同步系统状态）
const autoStartEnabled = ref(store.settings.autoStart)

// 监听 store 变化同步到本地
watch(() => store.settings.locale, (val) => {
  localLocale.value = val
})

watch(() => store.settings.refreshIntervalSeconds, (val) => {
  localRefreshInterval.value = val
})

watch(() => store.settings.dataSource, (val) => {
  localDataSource.value = val
})

watch(() => store.settings.theme, (val) => {
  localTheme.value = val || 'system'
})

watch(() => store.settings.proxy.includeErrorRequests, (val) => {
  localIncludeErrorRequests.value = val ?? true
})

watch(() => store.settings.sync, (val) => {
  localSyncEnabled.value = val?.enabled ?? false
  localSyncUrl.value = val?.url ?? ''
  localSyncUsername.value = val?.username ?? ''
  localSyncDeviceId.value = val?.deviceId ?? ''
  localSyncIntervalMinutes.value = val?.intervalMinutes ?? 15
  localSyncOnStartup.value = val?.syncOnStartup ?? false
  if (!passwordFieldsFocused.value) {
    webdavPassword.value = val?.password ?? ''
    syncPassword.value = val?.syncPassword ?? ''
  }
}, { deep: true })

// 更新本地状态并保存
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

const handleDataSourceChange = async () => {
  store.settings.dataSource = localDataSource.value
  await store.saveSettings()
}

const handleThemeChange = async () => {
  store.settings.theme = localTheme.value
  await store.saveSettings()
}

const handleIncludeErrorRequestsChange = async () => {
  store.settings.proxy.includeErrorRequests = localIncludeErrorRequests.value
  await store.saveSettings()
}

const DEVICE_ID_MAX_LENGTH = 48

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
  if (!value) {
    return t(store.settings.locale, 'settings.syncDeviceIdRequired')
  }
  if (value.length < 3) {
    return t(store.settings.locale, 'settings.syncDeviceIdTooShort')
  }
  if (value.length > DEVICE_ID_MAX_LENGTH) {
    return t(store.settings.locale, 'settings.syncDeviceIdTooLong')
  }
  if (!/^[a-z0-9._-]+$/.test(value)) {
    return t(store.settings.locale, 'settings.syncDeviceIdInvalid')
  }
  if (!/[a-z0-9]/.test(value)) {
    return t(store.settings.locale, 'settings.syncDeviceIdInvalid')
  }
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
    syncOnStartup: localSyncOnStartup.value,
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
  if (message.startsWith('ERR_SYNC_DEVICE_ID_CONFLICT')) {
    return t(store.settings.locale, 'settings.syncDeviceIdConflict')
  }
  if (message.startsWith('ERR_SYNC_DEVICE_ID_REQUIRED')) {
    return t(store.settings.locale, 'settings.syncDeviceIdRequired')
  }
  if (message.startsWith('ERR_SYNC_DEVICE_ID_TOO_SHORT')) {
    return t(store.settings.locale, 'settings.syncDeviceIdTooShort')
  }
  if (message.startsWith('ERR_SYNC_DEVICE_ID_TOO_LONG')) {
    return t(store.settings.locale, 'settings.syncDeviceIdTooLong')
  }
  if (message.startsWith('ERR_SYNC_DEVICE_ID_INVALID')) {
    return t(store.settings.locale, 'settings.syncDeviceIdInvalid')
  }
  if (message.startsWith('ERR_WEBDAV_URL_REQUIRED')) {
    return t(store.settings.locale, 'settings.syncUrlRequired')
  }
  if (message.startsWith('ERR_WEBDAV_USERNAME_REQUIRED')) {
    return t(store.settings.locale, 'settings.syncUsernameRequired')
  }
  if (message.startsWith('ERR_WEBDAV_PASSWORD_REQUIRED')) {
    return t(store.settings.locale, 'settings.syncWebdavPasswordRequired')
  }
  if (message.startsWith('ERR_SYNC_PASSWORD_TOO_SHORT')) {
    return t(store.settings.locale, 'settings.syncPasswordTooShort')
  }
  if (message.startsWith('ERR_SYNC_PASSWORD_ROTATION_REQUIRES_SYNC')) {
    return t(store.settings.locale, 'settings.syncPasswordRotationRequiresSync')
  }
  if (message.startsWith('ERR_SYNC_DECRYPT_FAILED')) {
    return t(store.settings.locale, 'settings.syncPasswordIncorrect')
  }
  if (message.startsWith('ERR_SYNC_KEYRING_MISSING')) {
    return t(store.settings.locale, 'settings.syncPasswordRotationRequiresSync')
  }
  if (message.startsWith('ERR_SYNC_LEGACY_PACKAGE_UNSUPPORTED')) {
    return t(store.settings.locale, 'settings.syncLegacyPackageUnsupported')
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
  } catch (e) {
    syncMessage.value = syncErrorMessage(e)
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
    await store.refreshUsage()
  } catch (e) {
    syncMessage.value = syncErrorMessage(e)
  } finally {
    syncBusy.value = false
  }
}

const removeSyncDevice = async (deviceId: string) => {
  syncConfirmMode.value = 'remove-device'
  syncConfirmDeviceId.value = deviceId
}

const clearImportedSyncData = async () => {
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
  } catch (e) {
    syncMessage.value = syncErrorMessage(e)
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
  if (rotateCurrentSyncPassword.value.length < 8) {
    rotatePasswordError.value = t(store.settings.locale, 'settings.syncPasswordTooShort')
    return
  }
  if (rotateNewSyncPassword.value.length < 8) {
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
  } catch (e) {
    rotatePasswordError.value = syncErrorMessage(e)
  } finally {
    rotatePasswordBusy.value = false
  }
}

// 代理控制
const proxyEnabled = computed(() => store.isProxyRunning)

const toggleProxy = async () => {
  await store.toggleProxy()
  await loadTakeoverStatuses()
}

const loadTakeoverStatuses = async () => {
  try {
    takeoverStatuses.value = await invoke<ToolTakeoverStatus[]>('get_takeover_statuses')
  } catch {
    takeoverStatuses.value = []
  }
}

const managedToolProfiles = computed(() => {
  return store.settings.clientTools.profiles.filter(profile => ['claude_code', 'codex'].includes(profile.tool))
})

const takeoverStatusFor = (tool: string) => {
  return takeoverStatuses.value.find(status => status.tool === tool)
}

const takeoverActiveFor = (tool: string) => {
  return takeoverStatusFor(tool)?.takeoverActive ?? false
}

const takeoverEnabledFor = (tool: string) => {
  return takeoverStatusFor(tool)?.enabled ?? managedToolProfiles.value.find(profile => profile.tool === tool)?.enabled ?? false
}

const ensureTakeoverStatusFor = async (tool: string) => {
  let status = takeoverStatusFor(tool)
  if (status) return status
  await loadTakeoverStatuses()
  return takeoverStatusFor(tool)
}

const getToolIcon = (tool: string, icon?: string) => {
  return icon || TOOL_LOBE_ICONS[tool] || null
}

const confirmOfficialApiRisk = () => new Promise<boolean>((resolve) => {
  resolveOfficialApiWarning = resolve
  showOfficialApiWarning.value = true
})

const closeOfficialApiWarning = (accepted: boolean) => {
  showOfficialApiWarning.value = false
  resolveOfficialApiWarning?.(accepted)
  resolveOfficialApiWarning = null
}

const toggleToolTakeover = async (tool: string) => {
  const nextEnabled = !takeoverEnabledFor(tool)
  const status = nextEnabled ? await ensureTakeoverStatusFor(tool) : takeoverStatusFor(tool)
  if (nextEnabled && (!status || status.officialProvider)) {
    const accepted = await confirmOfficialApiRisk()
    if (!accepted) {
      return
    }
  }
  takeoverLoading.value = { ...takeoverLoading.value, [tool]: true }
  try {
    await invoke('set_takeover_for_app', { app: tool, enabled: nextEnabled })
    await store.loadSettings()
    await store.getProxyStatus()
    await loadTakeoverStatuses()
  } catch {
    await loadTakeoverStatuses()
  } finally {
    takeoverLoading.value = { ...takeoverLoading.value, [tool]: false }
  }
}

// 开机自启动控制
const toggleAutoStart = async () => {
  try {
    if (autoStartEnabled.value) {
      await invoke('disable_autostart')
      autoStartEnabled.value = false
    } else {
      await invoke('enable_autostart')
      autoStartEnabled.value = true
    }
    // 同步保存到 settings
    store.settings.autoStart = autoStartEnabled.value
    await store.saveSettings()
  } catch (e) {
    console.error('Failed to toggle autostart:', e)
    // 发生错误时，恢复到系统实际状态
    try {
      autoStartEnabled.value = await invoke('is_autostart_enabled')
    } catch {
      // 忽略错误
    }
  }
}

// 初始化：从系统获取实际的 autostart 状态，与配置同步
onMounted(async () => {
  await loadTakeoverStatuses()
  await loadSyncStatus()
  await loadSyncDevices()
  try {
    const systemState = await invoke<boolean>('is_autostart_enabled')
    autoStartEnabled.value = systemState
    // 如果配置和系统状态不一致，同步配置
    if (store.settings.autoStart !== systemState) {
      store.settings.autoStart = systemState
      await store.saveSettings()
    }
  } catch (e) {
    console.error('Failed to check autostart status:', e)
    // 如果系统查询失败，使用配置中的值
    autoStartEnabled.value = store.settings.autoStart
  }
})

// 代理状态信息
const proxyStatusInfo = computed(() => {
  if (!store.proxyStatus) return null
  const status = store.proxyStatus
  return {
    port: status.port,
    uptime: formatUptime(status.uptimeSeconds),
    totalRequests: status.totalRequests,
    activeConnections: status.activeConnections,
    configTakenOver: status.configTakenOver,
    recordCount: status.recordCount
  }
})

// 格式化运行时间
const formatUptime = (seconds: number): string => {
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`
}

const syncStatusLabel = computed(() => {
  switch (syncStatus.value?.lastStatus) {
    case 'success':
      return t(store.settings.locale, 'settings.syncStateSuccess')
    case 'failed':
      return t(store.settings.locale, 'settings.syncStateFailed')
    case 'idle':
    default:
      return t(store.settings.locale, 'settings.syncStateIdle')
  }
})
</script>

<template>
  <div class="relative">
    <!-- 模型价格子页面 -->
    <ModelPricingSettings
      v-show="subView === 'model-pricing'"
      :key="modelPricingKey"
      @back="goBack"
    />

    <!-- API 来源管理子页面 -->
    <ApiSourceList
      v-show="subView === 'api-sources'"
      :onBack="goBack"
    />

    <!-- 货币设置子页面 -->
    <CurrencySettings
      v-show="subView === 'currency'"
      @back="goBack"
    />

    <!-- 主设置页面 -->
    <div v-show="subView === 'main'" class="space-y-5 animate-in fade-in zoom-in-95 duration-300 pb-6">
      <div class="space-y-2">
        <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider px-1">{{ t(store.settings.locale, 'settings.general') }}</h3>
        <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden divide-y divide-gray-50 dark:divide-neutral-800/50 shadow-sm">
          <div class="p-3 px-4 flex items-center justify-between text-[13px]">
            <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.locale') }}</span>
            <select
              v-model="localLocale"
              @change="handleLocaleChange"
              class="bg-transparent text-gray-500 dark:text-gray-400 text-sm outline-none text-right tracking-tight cursor-pointer appearance-none"
            >
              <option value="zh-CN">{{ t(store.settings.locale, 'settings.zhCN') }}</option>
              <option value="zh-TW">{{ t(store.settings.locale, 'settings.zhTW') }}</option>
              <option value="en-US">{{ t(store.settings.locale, 'settings.enUS') }}</option>
            </select>
          </div>
          <div class="p-3 px-4 flex items-center justify-between text-[13px]">
            <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.theme') }}</span>
            <div class="flex gap-1.5">
              <button
                v-for="theme in ['light', 'dark', 'system'] as ThemeMode[]"
                :key="theme"
                @click="localTheme = theme; handleThemeChange()"
                :class="[
                  'px-2.5 py-1 rounded-md text-xs font-medium transition-all',
                  localTheme === theme
                    ? 'bg-blue-500 text-white'
                    : 'bg-gray-100 dark:bg-neutral-700 text-gray-600 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-neutral-600'
                ]"
              >
                {{ t(store.settings.locale, `settings.theme${theme.charAt(0).toUpperCase() + theme.slice(1)}`) }}
              </button>
            </div>
          </div>
          <div class="p-3 px-4 flex items-center justify-between text-[13px]">
            <div class="flex flex-col">
              <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.autoStart') }}</span>
              <span class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.autoStartDesc') }}</span>
            </div>
            <div
              :class="[
                'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
                autoStartEnabled ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
              ]"
              @click="toggleAutoStart"
            >
              <div
                :class="[
                  'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                  autoStartEnabled ? 'right-[2px]' : 'left-[2px]'
                ]"
              ></div>
            </div>
          </div>
        </div>
      </div>
  
      <!-- 数据统计方式 -->
      <div class="space-y-2">
        <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider px-1">{{ t(store.settings.locale, 'settings.dataSource') }}</h3>
        <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden divide-y divide-gray-50 dark:divide-neutral-800/50 shadow-sm">
          <div class="p-3 px-4">
            <div class="flex gap-2">
              <button
                v-for="source in ['local', 'proxy'] as DataSource[]"
                :key="source"
                @click="localDataSource = source; handleDataSourceChange()"
                :class="[
                  'flex-1 py-2 px-3 rounded-lg text-xs font-medium transition-all',
                  localDataSource === source
                    ? 'bg-blue-500 text-white'
                    : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-neutral-700'
                ]"
              >
                {{ t(store.settings.locale, `settings.dataSource${source.charAt(0).toUpperCase() + source.slice(1)}`) }}
              </button>
            </div>
            <!-- 数据源说明 -->
            <p class="mt-2 text-[10px] text-gray-400 dark:text-gray-500 leading-relaxed">
              {{ t(store.settings.locale, localDataSource === 'local' ? 'settings.dataSourceLocalDesc' : 'settings.dataSourceProxyDesc') }}
            </p>
          </div>
  
          <!-- 代理开关 -->
          <div class="p-3 px-4 flex items-center justify-between text-[13px]">
            <div class="flex flex-col">
              <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.interceptRequests') }}</span>
              <span v-if="proxyEnabled && proxyStatusInfo" class="text-[10px] text-gray-400 mt-0.5">
                {{ t(store.settings.locale, 'settings.proxyRunning') }} · {{ t(store.settings.locale, 'settings.port') }} {{ proxyStatusInfo.port }} · {{ proxyStatusInfo.uptime }}
              </span>
              <span v-else-if="localDataSource === 'proxy' && !proxyEnabled" class="text-[10px] text-amber-500 mt-0.5">
                {{ t(store.settings.locale, 'settings.startProxyHint') }}
              </span>
            </div>
            <!-- iOS styled switch -->
            <div
              :class="[
                'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
                proxyEnabled ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
              ]"
              @click="toggleProxy"
            >
              <div
                :class="[
                  'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                  proxyEnabled ? 'right-[2px]' : 'left-[2px]'
                ]"
              ></div>
            </div>
          </div>
  
          <!-- 代理详细状态（仅在运行时显示） -->
          <div v-if="proxyEnabled && proxyStatusInfo" class="p-3 px-4 bg-gray-50 dark:bg-neutral-800/50">
            <div class="grid grid-cols-2 gap-2 text-[11px]">
              <div class="flex items-center gap-1.5">
                <span :class="['w-2 h-2 rounded-full', proxyStatusInfo.configTakenOver ? 'bg-green-500' : 'bg-amber-500']"></span>
                <span class="text-gray-500 dark:text-gray-400">
                  {{ proxyStatusInfo.configTakenOver ? t(store.settings.locale, 'settings.configTakenOver') : t(store.settings.locale, 'settings.configNotTakenOver') }}
                </span>
              </div>
              <div class="flex items-center gap-1.5">
                <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.requestCount') }}:</span>
                <span class="text-gray-700 dark:text-gray-300 font-mono">{{ proxyStatusInfo.totalRequests }}</span>
              </div>
              <div class="flex items-center gap-1.5">
                <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.recordCount') }}:</span>
                <span class="text-gray-700 dark:text-gray-300 font-mono">{{ proxyStatusInfo.recordCount }}</span>
              </div>
              <div class="flex items-center gap-1.5">
                <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.activeConnections') }}:</span>
                <span class="text-gray-700 dark:text-gray-300 font-mono">{{ proxyStatusInfo.activeConnections }}</span>
              </div>
            </div>
          </div>

          <!-- 工具接管 -->
          <div v-if="localDataSource === 'proxy'" class="p-3 px-4">
            <div class="mb-2 flex items-center justify-between">
              <div>
                <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.proxyTools') }}</div>
                <div class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.proxyToolsDesc') }}</div>
              </div>
              <span class="text-[10px] text-gray-400">
                {{ managedToolProfiles.filter(profile => takeoverEnabledFor(profile.tool)).length }}/{{ managedToolProfiles.length }}
              </span>
            </div>
            <div class="grid grid-cols-2 gap-2">
              <button
                v-for="profile in managedToolProfiles"
                :key="profile.id"
                :class="[
                  'min-w-0 rounded-xl border p-2.5 text-left transition-all',
                  takeoverEnabledFor(profile.tool)
                    ? 'border-green-200 bg-green-50/80 dark:border-green-500/30 dark:bg-green-500/10'
                    : 'border-gray-100 bg-gray-50/70 dark:border-neutral-800 dark:bg-neutral-900/60',
                  takeoverLoading[profile.tool] ? 'opacity-60 pointer-events-none' : 'hover:border-blue-200 dark:hover:border-blue-500/40'
                ]"
                @click="toggleToolTakeover(profile.tool)"
              >
                <div class="flex items-center justify-between gap-2">
                  <div class="flex min-w-0 items-center gap-2">
                    <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-white shadow-[0_1px_4px_rgba(0,0,0,0.04)] dark:bg-neutral-800">
                      <LobeIcon
                        v-if="getToolIcon(profile.tool, profile.icon)"
                        :slug="getToolIcon(profile.tool, profile.icon)!"
                        :size="17"
                        @error="() => {}"
                      />
                      <span v-else class="h-2.5 w-2.5 rounded-full bg-gray-400"></span>
                    </div>
                    <span class="truncate text-[12px] font-medium text-gray-700 dark:text-gray-200">{{ profile.displayName || profile.tool }}</span>
                  </div>
                  <span
                    :class="[
                      'relative flex h-5 w-9 shrink-0 items-center rounded-full transition-colors',
                      takeoverEnabledFor(profile.tool) ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
                    ]"
                  >
                    <span
                      :class="[
                        'absolute h-[17px] w-[17px] rounded-full bg-white shadow shadow-black/10 transition-all',
                        takeoverEnabledFor(profile.tool) ? 'right-[2px]' : 'left-[2px]'
                      ]"
                    ></span>
                  </span>
                </div>
                <div class="mt-2 truncate text-[10px] text-gray-400">
                  {{ takeoverActiveFor(profile.tool) ? t(store.settings.locale, 'settings.configTakenOver') : t(store.settings.locale, 'settings.configNotTakenOver') }}
                  <template v-if="takeoverStatusFor(profile.tool)?.activeSourceId"> · {{ takeoverStatusFor(profile.tool)?.activeSourceId }}</template>
                </div>
                <div v-if="takeoverStatusFor(profile.tool)?.lastError" class="mt-1 truncate text-[10px] text-red-500">
                  {{ takeoverStatusFor(profile.tool)?.lastError }}
                </div>
              </button>
            </div>
          </div>
  
          <!-- 包含错误请求（仅代理模式显示） -->
          <div v-if="localDataSource === 'proxy'" class="p-3 px-4 flex items-center justify-between text-[13px]">
            <div class="flex flex-col">
              <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.includeErrorRequests') }}</span>
              <span class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.includeErrorRequestsDesc') }}</span>
            </div>
            <!-- iOS styled switch -->
            <div
              :class="[
                'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
                localIncludeErrorRequests ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
              ]"
              @click="localIncludeErrorRequests = !localIncludeErrorRequests; handleIncludeErrorRequestsChange()"
            >
              <div
                :class="[
                  'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                  localIncludeErrorRequests ? 'right-[2px]' : 'left-[2px]'
                ]"
              ></div>
            </div>
          </div>
  
          <!-- 刷新间隔 -->
          <div class="p-3 px-4 flex items-center justify-between text-[13px]">
            <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.refreshInterval') }}</span>
            <div class="flex items-center gap-1">
              <input
                type="number"
                v-model.number="localRefreshInterval"
                @blur="handleRefreshIntervalChange"
                @keyup.enter="handleRefreshIntervalChange"
                min="5"
                max="300"
                class="w-12 bg-transparent text-gray-500 dark:text-gray-400 text-sm font-mono outline-none text-right p-0"
              />
              <span class="text-xs text-gray-400">{{ t(store.settings.locale, 'common.seconds') }}</span>
            </div>
          </div>
        </div>
      </div>
  
      <!-- 数据与配额 -->
      <div class="space-y-2">
        <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider px-1">{{ t(store.settings.locale, 'settings.dataManagement') }}</h3>
        <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden divide-y divide-gray-50 dark:divide-neutral-800/50 shadow-sm">
  
          <!-- API 来源入口（仅代理模式显示） -->
          <div
            v-if="localDataSource === 'proxy'"
            @click="openApiSources"
            class="p-3 px-4 cursor-pointer hover:bg-gray-50 dark:hover:bg-neutral-800/50 transition-colors"
          >
            <div class="flex items-center justify-between">
              <div>
                <div class="flex items-center gap-2">
                  <div class="text-[13px] text-gray-700 dark:text-gray-200">
                    {{ t(store.settings.locale, 'sources.manage') }}
                  </div>
                  <span
                    v-if="store.settings.sourceAware.sources.filter(s => s.autoDetected && !s.displayName).length > 0"
                    class="px-1.5 py-0.5 text-[10px] font-medium bg-red-100 text-red-600 dark:bg-red-500/20 dark:text-red-400 rounded-full"
                  >
                    {{ store.settings.sourceAware.sources.filter(s => s.autoDetected && !s.displayName).length }}
                  </span>
                </div>
                <div class="text-[10px] text-gray-400 mt-0.5">
                  {{ store.settings.sourceAware.sources.length > 0
                    ? `${store.settings.sourceAware.sources.length} ${t(store.settings.locale, 'sources.sourcesCount')}`
                    : t(store.settings.locale, 'sources.noSources')
                  }}
                </div>
              </div>
              <svg class="w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </div>
          </div>
  
          <!-- 模型价格入口 -->
          <div
            @click="openModelPricing"
            class="p-3 px-4 cursor-pointer hover:bg-gray-50 dark:hover:bg-neutral-800/50 transition-colors"
          >
            <div class="flex items-center justify-between">
              <div>
                <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.modelPricing') }}</div>
                <div class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.modelPricingDesc') }}</div>
              </div>
              <svg class="w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </div>
          </div>
  
          <!-- 货币设置入口 -->
          <div
            @click="openCurrency"
            class="p-3 px-4 cursor-pointer hover:bg-gray-50 dark:hover:bg-neutral-800/50 transition-colors"
          >
            <div class="flex items-center justify-between">
              <div>
                <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.currency') }}</div>
                <div class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.currencyDesc') }}</div>
              </div>
              <svg class="w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </div>
          </div>

          <!-- WebDAV 同步 -->
          <div class="p-3 px-4">
            <div class="mb-3 flex items-center justify-between gap-3">
              <div class="min-w-0">
                <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.syncWebdav') }}</div>
                <div class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.syncWebdavDesc') }}</div>
              </div>
              <div
                :class="[
                  'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
                  localSyncEnabled ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
                ]"
                @click="localSyncEnabled = !localSyncEnabled; saveSyncSettings()"
              >
                <div
                  :class="[
                    'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                    localSyncEnabled ? 'right-[2px]' : 'left-[2px]'
                  ]"
                ></div>
              </div>
            </div>
            <div v-if="localSyncEnabled" class="space-y-2 rounded-xl border border-gray-100 bg-gray-50/70 p-2.5 dark:border-neutral-800 dark:bg-neutral-900/60">
              <div class="space-y-1">
                <div class="text-[11px] font-medium text-gray-500 dark:text-gray-400">
                  {{ t(store.settings.locale, 'settings.syncUrl') }}
                </div>
                <input
                  v-model="localSyncUrl"
                  @blur="saveSyncSettings"
                  :placeholder="t(store.settings.locale, 'settings.syncUrl')"
                  class="w-full rounded-lg border border-white/70 bg-white px-3 py-2 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-200"
                />
              </div>
              <div class="grid grid-cols-2 gap-2">
                <div class="space-y-1">
                  <div class="text-[11px] font-medium text-gray-500 dark:text-gray-400">
                    {{ t(store.settings.locale, 'settings.syncUsername') }}
                  </div>
                  <input
                    v-model="localSyncUsername"
                    @blur="saveSyncSettings"
                    :placeholder="t(store.settings.locale, 'settings.syncUsername')"
                    class="w-full rounded-lg border border-white/70 bg-white px-3 py-2 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-200"
                  />
                </div>
                <div class="space-y-1">
                  <div class="text-[11px] font-medium text-gray-500 dark:text-gray-400">
                    {{ t(store.settings.locale, 'settings.syncDeviceId') }}
                  </div>
                  <input
                    v-model="localSyncDeviceId"
                    @input="applyDeviceIdInput"
                    @blur="saveSyncSettings"
                    :placeholder="t(store.settings.locale, 'settings.syncDeviceId')"
                    :class="[
                      'w-full rounded-lg border px-3 py-2 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:bg-neutral-950 dark:text-gray-200',
                      syncDeviceIdError
                        ? 'border-red-200 bg-red-50/60 dark:border-red-900 dark:bg-red-950/20'
                      : 'border-white/70 bg-white dark:border-neutral-700'
                    ]"
                  />
                  <div class="text-[10px] leading-relaxed text-gray-400 dark:text-gray-500">
                    {{ t(store.settings.locale, 'settings.syncDeviceIdDesc') }}
                  </div>
                </div>
              </div>
              <div v-if="syncDeviceIdError" class="text-[10px] leading-relaxed text-red-500 dark:text-red-400">
                {{ syncDeviceIdError }}
              </div>
              <div class="grid grid-cols-2 gap-2">
                <div class="space-y-1">
                  <div class="text-[11px] font-medium text-gray-500 dark:text-gray-400">
                    {{ t(store.settings.locale, 'settings.syncInterval') }}
                  </div>
                  <div class="flex items-center gap-2">
                    <input
                      v-model="localSyncIntervalMinutes"
                      type="number"
                      min="1"
                      max="1440"
                      @blur="saveSyncSettings"
                      class="w-full rounded-lg border border-white/70 bg-white px-3 py-2 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-200"
                    />
                    <div class="shrink-0 text-[10px] text-gray-400 dark:text-gray-500">
                      {{ t(store.settings.locale, 'settings.syncIntervalUnit') }}
                    </div>
                  </div>
                </div>
                <div class="grid grid-cols-1 gap-1.5 pt-5">
                  <label class="flex items-center justify-between gap-2 rounded-lg border border-white/70 bg-white px-2.5 py-2 text-[11px] text-gray-600 dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-300">
                    <span>{{ t(store.settings.locale, 'settings.syncOnStartup') }}</span>
                    <input v-model="localSyncOnStartup" type="checkbox" @change="saveSyncSettings" class="h-3.5 w-3.5 rounded border-gray-300 text-blue-500 focus:ring-blue-500" />
                  </label>
                </div>
              </div>
              <div class="grid grid-cols-2 gap-2">
                <div class="space-y-1">
                  <div class="text-[11px] font-medium text-gray-500 dark:text-gray-400">
                    {{ t(store.settings.locale, 'settings.syncWebdavPassword') }}
                  </div>
                  <div class="relative min-w-0">
                    <input
                      v-model="webdavPassword"
                      @focus="passwordFieldsFocused = true"
                      @blur="passwordFieldsFocused = false; saveSyncSettings()"
                      :type="showWebdavPassword ? 'text' : 'password'"
                      :placeholder="t(store.settings.locale, 'settings.syncWebdavPassword')"
                      class="w-full rounded-lg border border-white/70 bg-white py-2 pl-3 pr-9 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-200"
                    />
                    <button
                      type="button"
                      class="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-md text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600 dark:text-gray-500 dark:hover:bg-neutral-800 dark:hover:text-gray-300"
                      :aria-label="t(store.settings.locale, showWebdavPassword ? 'settings.hidePassword' : 'settings.showPassword')"
                      @mousedown.prevent
                      @click="showWebdavPassword = !showWebdavPassword"
                    >
                      <EyeOff v-if="showWebdavPassword" class="h-3.5 w-3.5" :stroke-width="2.2" />
                      <Eye v-else class="h-3.5 w-3.5" :stroke-width="2.2" />
                    </button>
                  </div>
                </div>
                <div class="space-y-1">
                  <div class="flex items-center justify-between gap-2">
                    <div class="text-[11px] font-medium text-gray-500 dark:text-gray-400">
                      {{ t(store.settings.locale, 'settings.syncEncryptPassword') }}
                    </div>
                    <button
                      type="button"
                      class="shrink-0 text-[10px] font-medium text-blue-500 transition-colors hover:text-blue-600 dark:text-blue-400 dark:hover:text-blue-300"
                      @click="rotatePasswordExpanded = !rotatePasswordExpanded; rotatePasswordError = ''"
                    >
                      {{ t(store.settings.locale, 'settings.syncPasswordChange') }}
                    </button>
                  </div>
                  <div class="relative min-w-0">
                    <input
                      v-model="syncPassword"
                      @focus="passwordFieldsFocused = true"
                      @blur="passwordFieldsFocused = false; saveSyncSettings()"
                      :type="showSyncPassword ? 'text' : 'password'"
                      :placeholder="t(store.settings.locale, 'settings.syncEncryptPassword')"
                      class="w-full rounded-lg border border-white/70 bg-white py-2 pl-3 pr-9 text-xs text-gray-700 outline-none shadow-[inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-neutral-700 dark:bg-neutral-950 dark:text-gray-200"
                    />
                    <button
                      type="button"
                      class="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-md text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-600 dark:text-gray-500 dark:hover:bg-neutral-800 dark:hover:text-gray-300"
                      :aria-label="t(store.settings.locale, showSyncPassword ? 'settings.hidePassword' : 'settings.showPassword')"
                      @mousedown.prevent
                      @click="showSyncPassword = !showSyncPassword"
                    >
                      <EyeOff v-if="showSyncPassword" class="h-3.5 w-3.5" :stroke-width="2.2" />
                      <Eye v-else class="h-3.5 w-3.5" :stroke-width="2.2" />
                    </button>
                  </div>
                </div>
              </div>
              <div
                v-if="rotatePasswordExpanded"
                class="space-y-2 rounded-lg border border-blue-100 bg-white/85 p-2.5 dark:border-blue-500/20 dark:bg-neutral-950/80"
              >
                <div class="text-[10px] text-gray-400 dark:text-gray-500">
                  {{ t(store.settings.locale, 'settings.syncPasswordChangeDesc') }}
                </div>
                <div class="grid grid-cols-3 gap-2">
                  <div class="space-y-1">
                    <div class="text-[10px] font-medium text-gray-500 dark:text-gray-400">
                      {{ t(store.settings.locale, 'settings.syncPasswordCurrent') }}
                    </div>
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
                    <div class="text-[10px] font-medium text-gray-500 dark:text-gray-400">
                      {{ t(store.settings.locale, 'settings.syncPasswordNew') }}
                    </div>
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
                    <div class="text-[10px] font-medium text-gray-500 dark:text-gray-400">
                      {{ t(store.settings.locale, 'settings.syncPasswordConfirm') }}
                    </div>
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
              <div class="flex items-center justify-between gap-2">
                <div class="min-w-0 text-[10px] text-gray-400">
                  <span v-if="syncStatus">{{ syncStatusLabel }}</span>
                  <span v-if="syncStatus"> · </span>
                  <span v-if="syncStatus?.lastSyncAt">
                    {{ t(store.settings.locale, 'settings.syncLast') }} {{ new Date(syncStatus.lastSyncAt * 1000).toLocaleString() }}
                  </span>
                  <span v-else>{{ t(store.settings.locale, 'settings.syncNever') }}</span>
                  <span v-if="syncStatus"> · {{ syncStatus.uploadedRequests }} / {{ syncStatus.importedRequests }}</span>
                </div>
                <div class="flex shrink-0 gap-1.5">
                  <button
                    class="rounded-lg bg-gray-100 px-2.5 py-1.5 text-[11px] font-medium text-gray-600 transition-colors hover:bg-gray-200 disabled:opacity-50 dark:bg-neutral-800 dark:text-gray-300 dark:hover:bg-neutral-700"
                    :disabled="syncBusy"
                    @click="testWebdav"
                  >
                    {{ t(store.settings.locale, 'settings.syncTest') }}
                  </button>
                  <button
                    class="rounded-lg bg-blue-500 px-2.5 py-1.5 text-[11px] font-medium text-white transition-colors hover:bg-blue-600 disabled:opacity-50"
                    :disabled="syncBusy"
                    @click="runWebdavSync"
                  >
                    {{ syncBusy ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'settings.syncNow') }}
                  </button>
                </div>
              </div>
              <div v-if="syncMessage" class="truncate text-[10px] text-gray-400">
                {{ syncMessage }}
              </div>
              <div class="space-y-2 rounded-lg border border-white/70 bg-white/80 p-2 dark:border-neutral-800 dark:bg-neutral-950/70">
                <div class="flex items-center justify-between gap-2">
                  <div class="text-[11px] font-medium text-gray-500 dark:text-gray-400">
                    {{ t(store.settings.locale, 'settings.syncRemoteDevices') }}
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
                    class="flex items-center justify-between gap-2 rounded-lg border border-gray-100 bg-gray-50/80 px-2.5 py-2 dark:border-neutral-800 dark:bg-neutral-900/70"
                  >
                    <div class="min-w-0">
                      <div class="truncate text-[11px] font-medium text-gray-700 dark:text-gray-200">
                        {{ device.deviceId }}
                      </div>
                      <div class="truncate text-[10px] text-gray-400 dark:text-gray-500">
                        seq {{ device.lastExportSeq }}<span v-if="device.lastSeenAt"> · {{ new Date(device.lastSeenAt * 1000).toLocaleString() }}</span>
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
          </div>

        </div>
      </div>
  
      <!-- 加载/保存状态 -->
      <div v-if="store.saving" class="text-center text-xs text-gray-400">
        {{ t(store.settings.locale, 'common.saving') }}
      </div>
      <div v-if="store.error" class="text-center text-xs text-red-500">
        {{ store.error }}
      </div>
    </div>

    <!-- 官方 API 风险确认 -->
    <Teleport to="body">
      <div
        v-if="showOfficialApiWarning"
        class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
        @click.self="closeOfficialApiWarning(false)"
      >
        <div class="w-[320px] overflow-hidden rounded-2xl bg-white shadow-xl dark:bg-[#1C1C1E]" @click.stop>
          <div class="p-4">
            <div class="mx-auto mb-3 flex h-10 w-10 items-center justify-center rounded-full bg-amber-100 dark:bg-amber-500/20">
              <svg class="h-5 w-5 text-amber-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v4m0 4h.01M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z" />
              </svg>
            </div>
            <h3 class="mb-2 text-center text-sm font-semibold text-gray-900 dark:text-gray-100">
              {{ t(store.settings.locale, 'settings.officialApiRiskTitle') }}
            </h3>
            <div class="space-y-2 text-xs leading-relaxed text-gray-500 dark:text-gray-400">
              <p>{{ t(store.settings.locale, 'settings.officialApiRiskBody') }}</p>
              <p>{{ t(store.settings.locale, 'settings.officialApiRiskAccount') }}</p>
              <p class="font-medium text-amber-600 dark:text-amber-400">{{ t(store.settings.locale, 'settings.officialApiRiskAccept') }}</p>
            </div>
          </div>
          <div class="flex border-t border-gray-100 dark:border-neutral-800">
            <button
              @click="closeOfficialApiWarning(false)"
              class="flex-1 py-2.5 text-xs font-medium text-gray-600 transition-colors hover:bg-gray-50 dark:text-gray-400 dark:hover:bg-neutral-800"
            >
              {{ t(store.settings.locale, 'common.cancel') }}
            </button>
            <button
              @click="closeOfficialApiWarning(true)"
              class="flex-1 border-l border-gray-100 py-2.5 text-xs font-medium text-amber-600 transition-colors hover:bg-amber-50 dark:border-neutral-800 dark:text-amber-400 dark:hover:bg-amber-500/10"
            >
              {{ t(store.settings.locale, 'settings.officialApiRiskContinue') }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>

    <Teleport to="body">
      <div
        v-if="syncConfirmMode"
        class="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4"
        @click.self="closeSyncConfirm"
      >
        <div class="w-[320px] overflow-hidden rounded-2xl border border-white/70 bg-white shadow-xl dark:border-neutral-800 dark:bg-[#1C1C1E]" @click.stop>
          <div class="p-4">
            <h3 class="text-sm font-semibold text-gray-900 dark:text-gray-100">
              {{ t(store.settings.locale, syncConfirmMode === 'remove-device' ? 'settings.syncConfirmRemoveTitle' : 'settings.syncConfirmClearTitle') }}
            </h3>
            <p class="mt-2 text-xs leading-relaxed text-gray-500 dark:text-gray-400">
              {{ t(store.settings.locale, syncConfirmMode === 'remove-device' ? 'settings.syncConfirmRemoveBody' : 'settings.syncConfirmClearBody') }}
            </p>
          </div>
          <div class="flex border-t border-gray-100 dark:border-neutral-800">
            <button
              class="flex-1 py-2.5 text-xs font-medium text-gray-600 transition-colors hover:bg-gray-50 dark:text-gray-400 dark:hover:bg-neutral-800"
              :disabled="syncBusy"
              @click="closeSyncConfirm"
            >
              {{ t(store.settings.locale, 'common.cancel') }}
            </button>
            <button
              class="flex-1 border-l border-gray-100 py-2.5 text-xs font-medium text-red-600 transition-colors hover:bg-red-50 disabled:opacity-50 dark:border-neutral-800 dark:text-red-400 dark:hover:bg-red-500/10"
              :disabled="syncBusy"
              @click="confirmSyncDanger"
            >
              {{ syncBusy ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'common.confirm') }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>
