<script setup lang="ts">
import { ref, watch, computed, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { type DataSource, type ThemeMode, type ToolTakeoverStatus, type SyncStatus, type RemoteSyncDevice } from '../types'
import ModelPricingSettings from '../components/ModelPricingSettings.vue'
import ApiSourceList from '../components/ApiSourceList.vue'
import CurrencySettings from '../components/CurrencySettings.vue'
import LobeIcon from '../components/LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'
import { Eye, EyeOff, RefreshCw, TestTube2 } from 'lucide-vue-next'

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
  localAutoSync.value = val?.autoSync ?? false
  if (!passwordFieldsFocused.value) {
    webdavPassword.value = val?.password ?? ''
    syncPassword.value = val?.syncPassword ?? ''
  }
}, { deep: true })

// ============ 网络代理 ============

type NetworkProxyScheme = 'http' | 'https' | 'socks5'

function buildProxyUrl(scheme: string, host: string, port: number): string {
  if (!host) return ''
  return `${scheme}://${host}:${port}`
}

function parseProxyUrl(url: string): { scheme: NetworkProxyScheme; host: string; port: number } | null {
  try {
    const u = new URL(url.trim())
    const scheme = u.protocol.replace(':', '') as NetworkProxyScheme
    if (!['http', 'https', 'socks5'].includes(scheme)) return null
    const port = u.port ? parseInt(u.port) : (scheme === 'https' ? 443 : 1080)
    if (!u.hostname || isNaN(port) || port < 1 || port > 65535) return null
    return { scheme, host: u.hostname, port }
  } catch {
    return null
  }
}

const npEnabled = ref(store.settings.networkProxy?.enabled ?? false)
const npUrl = ref(
  store.settings.networkProxy?.host
    ? buildProxyUrl(
        store.settings.networkProxy.scheme ?? 'http',
        store.settings.networkProxy.host,
        store.settings.networkProxy.port ?? 7890
      )
    : ''
)
const npSavedFlash = ref(false)
const npError = ref('')

const proxyTargetIcons: Record<string, string> = {
  github: 'M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12',
  anthropic: 'M12.4553 3H11.0947L4.5 21h2.8026l1.4702-4.1082h6.4538L16.6974 21H19.5L12.4553 3ZM9.4614 14.4695 11.775 7.9185l2.3136 6.551H9.4614Z',
  openai: 'M22.282 9.821a5.985 5.985 0 0 0-.516-4.911 6.046 6.046 0 0 0-6.51-2.9 6.065 6.065 0 0 0-4.981-2.529 6.046 6.046 0 0 0-5.777 4.196 6.065 6.065 0 0 0-3.998 2.9 6.046 6.046 0 0 0 .743 7.097 5.98 5.98 0 0 0 .511 4.911 6.051 6.051 0 0 0 6.515 2.9A5.985 5.985 0 0 0 13.26 24a6.056 6.056 0 0 0 5.772-4.206 5.99 5.99 0 0 0 3.997-2.9 6.056 6.056 0 0 0-.747-7.073zM13.26 22.43a4.476 4.476 0 0 1-2.876-1.041l.141-.081 4.779-2.758a.775.775 0 0 0 .392-.681v-6.737l2.02 1.169a.071.071 0 0 1 .038.052v5.583a4.504 4.504 0 0 1-4.494 4.494zM3.6 18.304a4.47 4.47 0 0 1-.535-3.014l.142.085 4.783 2.759a.771.771 0 0 0 .78 0l5.843-3.369v2.332a.08.08 0 0 1-.033.062L9.74 19.95a4.5 4.5 0 0 1-6.14-1.646zM2.34 7.896a4.485 4.485 0 0 1 2.366-1.973V11.6a.766.766 0 0 0 .388.676l5.815 3.355-2.02 1.168a.076.076 0 0 1-.071 0l-4.83-2.786A4.504 4.504 0 0 1 2.34 7.872zm16.597 3.855-5.803-3.358L15.154 7.2a.076.076 0 0 1 .071 0l4.83 2.791a4.494 4.494 0 0 1-.676 8.104v-5.678a.79.79 0 0 0-.407-.666zm2.01-3.023-.141-.085-4.774-2.782a.776.776 0 0 0-.785 0L9.409 9.23V6.897a.066.066 0 0 1 .028-.061l4.83-2.787a4.5 4.5 0 0 1 6.68 4.66zm-12.64 4.135-2.02-1.167a.08.08 0 0 1-.038-.057V6.075a4.5 4.5 0 0 1 7.375-3.453l-.142.08-4.778 2.758a.775.775 0 0 0-.392.681zm1.097-2.365 2.602-1.5 2.607 1.5v2.999l-2.597 1.5-2.607-1.5z',
}

type TestState = { status: 'idle' | 'testing' | 'success' | 'error'; latency?: number; errorKey?: string; errorDetail?: string }
const npTests = ref<Record<string, TestState>>({
  github: { status: 'idle' },
  anthropic: { status: 'idle' },
  openai: { status: 'idle' },
})

watch(() => store.settings.networkProxy, (val) => {
  const newEnabled = val?.enabled ?? false
  const newUrl = val?.host ? buildProxyUrl(val.scheme ?? 'http', val.host, val.port ?? 7890) : ''
  // 只有外部真正改变时才同步本地状态，避免自己保存触发覆盖
  const enabledChanged = newEnabled !== npEnabled.value
  const urlChanged = newUrl !== npUrl.value
  if (enabledChanged) npEnabled.value = newEnabled
  if (urlChanged) npUrl.value = newUrl
  // 仅在外部真正变化时才重置测试状态，防止 saveNetworkProxy 写 store 触发的 watch 把刚启动的测试清掉
  if (enabledChanged || urlChanged) resetTestStates()
}, { deep: true })

const networkProxyChipText = computed(() => {
  if (!npEnabled.value) {
    return t(store.settings.locale, 'settings.networkProxyChipFollowSystem')
  }
  return npUrl.value || t(store.settings.locale, 'settings.networkProxyChipFollowSystem')
})

const networkProxyDirty = computed(() => {
  const cur = store.settings.networkProxy
  const curEnabled = cur?.enabled ?? false
  const curUrl = cur?.host ? buildProxyUrl(cur.scheme ?? 'http', cur.host, cur.port ?? 7890) : ''
  return curEnabled !== npEnabled.value || curUrl !== npUrl.value
})

function validateNetworkProxy(): string {
  if (!npEnabled.value) return ''
  if (!npUrl.value.trim()) return t(store.settings.locale, 'settings.networkProxyUrlRequired')
  if (!parseProxyUrl(npUrl.value)) return t(store.settings.locale, 'settings.networkProxyUrlInvalid')
  return ''
}

function currentProxyPayload() {
  // 透传已保存的 username/password，防止点击保存时把用户之前配置的代理凭据抹除
  const saved = store.settings.networkProxy
  const credentials = {
    username: saved?.username,
    password: saved?.password,
  }
  if (!npEnabled.value || !npUrl.value.trim()) {
    return { enabled: false, scheme: 'http' as NetworkProxyScheme, host: '', port: 7890, ...credentials }
  }
  const parsed = parseProxyUrl(npUrl.value)!
  return { enabled: true, scheme: parsed.scheme, host: parsed.host, port: parsed.port, ...credentials }
}

const toggleNetworkProxy = async () => {
  npEnabled.value = !npEnabled.value
  npError.value = ''
  resetTestStates()
  // 关闭时直接保存 disabled 状态；开启时等用户填写 URL 后点保存
  if (!npEnabled.value) {
    const previous = store.settings.networkProxy
    // 透传凭据，避免 disable 操作抹除已保存的 username/password
    store.settings.networkProxy = {
      enabled: false,
      scheme: previous?.scheme ?? 'http',
      host: previous?.host ?? '',
      port: previous?.port ?? 7890,
      username: previous?.username,
      password: previous?.password,
    }
    try {
      await store.saveSettings()
    } catch (e) {
      store.settings.networkProxy = previous
      npEnabled.value = true
    }
  }
}

const saveNetworkProxy = async () => {
  const err = validateNetworkProxy()
  if (err) { npError.value = err; return }
  npError.value = ''
  const previous = store.settings.networkProxy
  store.settings.networkProxy = currentProxyPayload()
  try {
    await store.saveSettings()
    npSavedFlash.value = true
    setTimeout(() => { npSavedFlash.value = false }, 1500)
    if (npEnabled.value) testAllTargets()
  } catch (e) {
    store.settings.networkProxy = previous
    npError.value = String(e)
  }
}

interface NetworkProxyTestResult {
  ok: boolean
  latencyMs?: number
  status?: number
  errorKind?: string
  errorDetail?: string
}

function resetTestStates() {
  npTests.value = {
    github: { status: 'idle' },
    anthropic: { status: 'idle' },
    openai: { status: 'idle' },
  }
}

async function testTarget(target: string) {
  npTests.value[target] = { status: 'testing' }
  try {
    const result = await invoke<NetworkProxyTestResult>('test_network_proxy', {
      config: currentProxyPayload(),
      target,
    })
    if (result.ok) {
      npTests.value[target] = { status: 'success', latency: result.latencyMs }
    } else {
      npTests.value[target] = {
        status: 'error',
        errorKey: result.errorKind ?? 'testUnknownError',
        errorDetail: result.errorDetail,
        latency: result.latencyMs,
      }
    }
  } catch (e) {
    npTests.value[target] = { status: 'error', errorKey: 'testUnknownError', errorDetail: String(e) }
  }
}

function testAllTargets() {
  const err = validateNetworkProxy()
  if (err) { npError.value = err; return }
  npError.value = ''
  testTarget('github')
  testTarget('anthropic')
  testTarget('openai')
}


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
  if (message.startsWith('ERR_WEBDAV_AUTH_FAILED')) {
    return t(store.settings.locale, 'settings.syncAuthFailed')
  }
  if (message.startsWith('ERR_WEBDAV_URL_INVALID')) {
    return t(store.settings.locale, 'settings.syncUrlInvalid')
  }
  if (message.startsWith('ERR_WEBDAV_REQUEST_FAILED')) {
    return t(store.settings.locale, 'settings.syncRequestFailed')
  }
  if (message.startsWith('ERR_WEBDAV_MKCOL_FAILED')) {
    return t(store.settings.locale, 'settings.syncMkcolFailed')
  }
  if (message.startsWith('ERR_WEBDAV_PUT_FAILED')) {
    return t(store.settings.locale, 'settings.syncPutFailed')
  }
  if (message.startsWith('ERR_WEBDAV_GET_FAILED')) {
    return t(store.settings.locale, 'settings.syncGetFailed')
  }
  if (message.startsWith('ERR_WEBDAV_PROPFIND_FAILED')) {
    return t(store.settings.locale, 'settings.syncPropfindFailed')
  }
  if (message.startsWith('ERR_SYNC_SCHEMA_UNSUPPORTED')) {
    return t(store.settings.locale, 'settings.syncSchemaUnsupported')
  }
  if (message.startsWith('ERR_SYNC_KEYRING_SCHEMA_UNSUPPORTED')) {
    return t(store.settings.locale, 'settings.syncSchemaUnsupported')
  }
  if (message.startsWith('ERR_SYNC_DEK_INVALID')) {
    return t(store.settings.locale, 'settings.syncDekInvalid')
  }
  if (message.startsWith('ERR_SYNC_BATCH_PRUNED_RETRY')) {
    return t(store.settings.locale, 'settings.syncBatchPrunedRetry')
  }
  if (message.startsWith('ERR_SYNC_BATCH_MISSING')) {
    return t(store.settings.locale, 'settings.syncBatchMissing')
  }
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

/** 从后端读回实际生效的 device_id，用于"留空时自动生成"场景 */
const refreshActiveDeviceId = async () => {
  if (localSyncDeviceId.value) return  // 用户已手动填写，不覆盖
  try {
    const activeId = await invoke<string | null>('get_active_sync_device_id')
    if (activeId && !localSyncDeviceId.value) {
      localSyncDeviceId.value = activeId
      store.settings.sync.deviceId = activeId
      // 无需 saveSettings，只是让 UI 反映后端实际值
    }
  } catch {
    // 读取失败静默忽略
  }
}

/** 开启 sync status 轮询（Settings 页面打开时，auto sync 开启时使用） */
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
    await refreshActiveDeviceId()
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
  // 如果 device_id 是空的，尝试从后端读回自动生成的值
  await refreshActiveDeviceId()
  // 开启状态轮询（auto sync 打开时后台静默同步，需要前端感知）
  if (localSyncEnabled.value) {
    startSyncStatusPoll()
  }
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
  // Escape 键：关闭弹窗
  window.addEventListener('keydown', handleGlobalKeydown)
})

onUnmounted(() => {
  stopSyncStatusPoll()
  window.removeEventListener('keydown', handleGlobalKeydown)
})

const handleGlobalKeydown = (e: KeyboardEvent) => {
  if (e.key === 'Escape') {
    if (syncConfirmMode.value) {
      closeSyncConfirm()
    } else if (showOfficialApiWarning.value) {
      closeOfficialApiWarning(false)
    } else if (rotatePasswordExpanded.value) {
      resetRotatePasswordForm()
    }
  }
}

// 当 sync 开关变化时，同步管理轮询
watch(localSyncEnabled, (enabled) => {
  if (enabled) {
    startSyncStatusPoll()
  } else {
    stopSyncStatusPoll()
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

const syncIntervalPresets = [30, 60]
const isCustomSyncInterval = computed(() => !syncIntervalPresets.includes(Number(localSyncIntervalMinutes.value)))

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

          <!-- 全局网络代理 -->
          <div class="p-3 px-4">
            <!-- 标题行：名称 + 描述副文本 + 开关 -->
            <div class="flex items-center justify-between gap-3">
              <div class="min-w-0 flex-1">
                <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.networkProxyTitle') }}</div>
                <div class="text-[10px] text-gray-400 mt-0.5 truncate">{{ networkProxyChipText }}</div>
              </div>
              <div
                :class="[
                  'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
                  npEnabled ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
                ]"
                @click="toggleNetworkProxy"
              >
                <div
                  :class="[
                    'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                    npEnabled ? 'right-[2px]' : 'left-[2px]'
                  ]"
                ></div>
              </div>
            </div>

            <!-- 展开区：仅在启用时显示 -->
            <div v-if="npEnabled" class="mt-3 space-y-3">
              <!-- 输入框 + 保存按钮同行 -->
              <div class="flex items-center gap-2">
                <input
                  v-model="npUrl"
                  type="text"
                  @input="npError = ''"
                  class="h-8 flex-1 min-w-0 text-[12px] px-3 rounded-lg border border-gray-200 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-gray-700 dark:text-gray-200 focus:outline-none focus:ring-1 focus:ring-emerald-300 placeholder:text-gray-300 dark:placeholder:text-neutral-600"
                  :placeholder="t(store.settings.locale, 'settings.networkProxyUrlPlaceholder')"
                />
                <button
                  type="button"
                  @click="saveNetworkProxy"
                  :disabled="!networkProxyDirty"
                  class="h-8 px-3 text-[12px] shrink-0 rounded-lg bg-emerald-500 text-white hover:bg-emerald-600 disabled:opacity-40 disabled:hover:bg-emerald-500 transition-colors"
                >
                  <span v-if="npSavedFlash" class="text-white">✓</span>
                  <span v-else>{{ t(store.settings.locale, 'settings.networkProxySave') }}</span>
                </button>
              </div>

              <!-- 错误提示 -->
              <div v-if="npError" class="text-[11px] text-red-500">{{ npError }}</div>

              <!-- 连通性测试：列表样式 -->
              <div class="rounded-xl overflow-hidden border border-gray-100 dark:border-neutral-700/60">
                <button
                  v-for="(target, i) in ['github', 'anthropic', 'openai']"
                  :key="target"
                  type="button"
                  @click="testTarget(target)"
                  class="w-full flex items-center justify-between px-3 py-2 text-[12px] transition-colors select-none"
                  :class="[
                    i < 2 ? 'border-b border-gray-100 dark:border-neutral-700/60' : '',
                    npTests[target].status === 'success'
                      ? 'bg-white dark:bg-neutral-800/40 hover:bg-gray-50 dark:hover:bg-neutral-800'
                      : npTests[target].status === 'error'
                        ? 'bg-white dark:bg-neutral-800/40 hover:bg-gray-50 dark:hover:bg-neutral-800'
                        : 'bg-white dark:bg-neutral-800/40 hover:bg-gray-50 dark:hover:bg-neutral-800'
                  ]"
                >
                  <span class="flex items-center gap-2">
                    <svg viewBox="0 0 24 24" class="w-3.5 h-3.5 shrink-0 transition-colors"
                      :class="[
                        npTests[target].status === 'testing' ? 'text-amber-400 animate-pulse' :
                        npTests[target].status === 'success' ? 'text-emerald-500' :
                        npTests[target].status === 'error'   ? 'text-red-400' :
                        'text-gray-400 dark:text-neutral-500'
                      ]"
                    ><path fill="currentColor" :d="proxyTargetIcons[target]" /></svg>
                    <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, `settings.networkProxyTarget_${target}`) }}</span>
                  </span>
                  <span class="tabular-nums text-[11px]"
                    :class="[
                      npTests[target].status === 'success' ? 'text-emerald-500' :
                      npTests[target].status === 'error'   ? 'text-red-400' :
                      'text-gray-300 dark:text-neutral-600'
                    ]"
                  >
                    <template v-if="npTests[target].status === 'idle'">—</template>
                    <template v-else-if="npTests[target].status === 'testing'">…</template>
                    <template v-else-if="npTests[target].status === 'success' && npTests[target].latency != null">{{ npTests[target].latency }}ms</template>
                    <template v-else-if="npTests[target].status === 'error'">{{ t(store.settings.locale, `settings.networkProxyErr_${npTests[target].errorKey ?? 'testUnknownError'}`) }}</template>
                  </span>
                </button>
              </div>
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
            <div v-if="localSyncEnabled" class="space-y-2.5 rounded-xl border border-gray-100 bg-gray-50/70 p-2 dark:border-neutral-800 dark:bg-neutral-900/60">
              <div class="flex items-start justify-between gap-2 rounded-xl bg-white/90 px-3 py-2 dark:bg-neutral-950/80">
                <div class="min-w-0 flex-1">
                  <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-gray-400 dark:text-gray-500">
                    <!-- 同步状态指示点 -->
                    <span class="flex items-center gap-1">
                      <span :class="[
                        'inline-block w-1.5 h-1.5 rounded-full shrink-0',
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
                  <!-- 失败时展示错误信息 -->
                  <div v-if="syncStatus?.lastStatus === 'failed' && syncStatus?.lastError && !syncMessage"
                       class="mt-1 truncate text-[10px] text-red-500 dark:text-red-400">
                    {{ syncErrorMessage(syncStatus.lastError) }}
                  </div>
                  <div v-if="syncMessage" class="mt-1 truncate text-[10px]"
                       :class="syncMessage === t(store.settings.locale, 'settings.syncSuccess') || syncMessage === t(store.settings.locale, 'settings.syncTestSuccess')
                         ? 'text-green-600 dark:text-green-400' : 'text-gray-500 dark:text-gray-400'">
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
                      <div class="text-[11px] font-medium text-gray-700 dark:text-gray-200">
                        {{ t(store.settings.locale, 'settings.syncAuto') }}
                      </div>
                      <div class="mt-0.5 text-[10px] text-gray-400 dark:text-gray-500">
                        {{ t(store.settings.locale, 'settings.syncAutoDesc') }}
                      </div>
                    </div>
                    <div
                      :class="[
                        'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
                        localAutoSync ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
                      ]"
                      @click="localAutoSync = !localAutoSync; saveSyncSettings()"
                    >
                      <div
                        :class="[
                          'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                          localAutoSync ? 'right-[2px]' : 'left-[2px]'
                        ]"
                      ></div>
                    </div>
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
                            'flex min-w-[108px] flex-1 items-center justify-center gap-1.5 rounded-md px-2.5 py-1 text-[10px] transition-colors whitespace-nowrap dark:bg-neutral-900',
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
                              @blur="saveSyncSettings"
                              :class="[
                                'w-8 shrink-0 bg-transparent text-right text-[10px] outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none',
                                isCustomSyncInterval ? 'text-blue-600 dark:text-blue-200' : 'text-gray-700 dark:text-gray-200'
                              ]"
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
                      @blur="saveSyncSettings"
                      :placeholder="t(store.settings.locale, 'settings.syncUrl')"
                      class="min-w-0 flex-1 bg-transparent py-0 text-xs leading-4 text-gray-700 outline-none dark:text-gray-200"
                    />
                  </div>

                  <div class="h-px bg-gray-100 dark:bg-neutral-800"></div>

                  <div class="flex min-h-[30px] items-center gap-2 rounded-lg px-1.5 py-0.5">
                    <div class="w-[78px] shrink-0 whitespace-nowrap text-[9px] font-medium text-gray-400 dark:text-gray-500">
                      {{ t(store.settings.locale, 'settings.syncUsername') }}
                    </div>
                    <input
                      v-model="localSyncUsername"
                      @blur="saveSyncSettings"
                      :placeholder="t(store.settings.locale, 'settings.syncUsername')"
                      class="min-w-0 flex-1 bg-transparent py-0 text-xs leading-4 text-gray-700 outline-none dark:text-gray-200"
                    />
                  </div>

                  <div class="h-px bg-gray-100 dark:bg-neutral-800"></div>

                  <div
                    :class="[
                      'rounded-lg px-1.5 py-0.5',
                      syncDeviceIdError ? 'bg-red-50/70 dark:bg-red-950/10' : ''
                    ]"
                  >
                    <div class="flex min-h-[30px] items-center gap-2">
                      <div class="w-[78px] shrink-0 whitespace-nowrap text-[9px] font-medium text-gray-400 dark:text-gray-500">
                        {{ t(store.settings.locale, 'settings.syncDeviceId') }}
                      </div>
                      <input
                        v-model="localSyncDeviceId"
                        @input="applyDeviceIdInput"
                        @blur="saveSyncSettings"
                        :placeholder="t(store.settings.locale, 'settings.syncDeviceId')"
                        :class="[
                          'min-w-0 flex-1 bg-transparent py-0 text-xs leading-4 outline-none',
                          syncDeviceIdError ? 'text-red-500 dark:text-red-400' : 'text-gray-700 dark:text-gray-200'
                        ]"
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
                        @focus="passwordFieldsFocused = true"
                        @blur="passwordFieldsFocused = false; saveSyncSettings()"
                        :type="showWebdavPassword ? 'text' : 'password'"
                        :placeholder="t(store.settings.locale, 'settings.syncWebdavPassword')"
                        class="w-full bg-transparent py-0 pr-6 text-xs leading-4 text-gray-700 outline-none dark:text-gray-200"
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
                          @focus="passwordFieldsFocused = true"
                          @blur="passwordFieldsFocused = false; saveSyncSettings()"
                          :type="showSyncPassword ? 'text' : 'password'"
                          :placeholder="t(store.settings.locale, 'settings.syncEncryptPassword')"
                          class="w-full bg-transparent py-0 pr-6 text-xs leading-4 text-gray-700 outline-none dark:text-gray-200"
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
