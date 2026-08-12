<script setup lang="ts">
/**
 * 桌面主窗口「网关」页（设计文档第 10 章）。
 * 业务逻辑整体复用 src/views/Gateway.vue（同一领域合同），布局按桌面密度重排：
 * 顶部状态带（监听状态/地址/活动连接/近期请求/错误率 + 唯一主操作「新增上游」），
 * 主体两栏（左 profile 列表 320px，右详情/编辑表单内嵌）。
 * 安全行为全部保留：key 默认遮罩、显式点击查看本地 key、创建后不再回显、
 * 复制 toast 不含 key、删除/撤销/停止接管需确认、敏感字段不进日志。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { Activity, AlertTriangle, Check, Copy, Eye, KeyRound, Link2, Pencil, Plus, RadioTower, RefreshCw, Save, ShieldCheck, Trash2, X } from 'lucide-vue-next'
import {
  createGatewayLocalKey,
  createGatewayProfile,
  createGatewayUpstreamKey,
  deleteGatewayProfile,
  deleteGatewayUpstreamKey,
  getGatewayStatus,
  listGatewayProfiles,
  listGatewayUpstreamModels,
  previewGatewayBaseUrl,
  revealGatewayLocalKey,
  revokeGatewayLocalKey,
  saveGatewayUpstreamModels,
  testGatewayUpstreamModel,
  updateGatewayProfile,
  updateGatewayUpstreamKey
} from '../../api/gatewayApi'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import type { GatewayBaseUrlPreview, GatewayCredentialRecovery, GatewayDispatchStrategy, GatewayModelTestResult, GatewayProfile, GatewayProtocol, GatewayStatus, GatewayUpstreamKey, GatewayUpstreamModelsResult } from '../../types'
import ProxyControlPanel from '../../components/settings/ProxyControlPanel.vue'
import CcSwitchCompatPanel from '../../components/settings/CcSwitchCompatPanel.vue'
import SettingsSwitch from '../../components/settings/SettingsSwitch.vue'

const store = useMonitorStore()
const profiles = ref<GatewayProfile[]>([])
const status = ref<GatewayStatus | null>(null)
const selectedId = ref<string | null>(null)
const editing = ref(false)
const saving = ref(false)
const loading = ref(false)
const activePanel = ref<'takeover' | 'manual'>('takeover')
const feedback = ref<'copied' | 'saved' | 'error' | null>(null)
const errorCode = ref('')
const upstreamRemark = ref('')
const upstreamSecret = ref('')
const localRemark = ref('')
const generatedLocalKey = ref('')
const keyPanel = ref<'upstream' | 'local' | null>(null)
const revealedLocalKeys = ref<Record<string, string>>({})
const modelsLoading = ref(false)
const modelsResult = ref<GatewayUpstreamModelsResult | null>(null)
const modelsPersisted = ref(false)
const selectedModelId = ref('')
const testingModel = ref(false)
const testResult = ref<GatewayModelTestResult | null>(null)
const emptyCredentialRecovery = (): GatewayCredentialRecovery => ({ upstreamKeyRequired: false, localKeyRotationRecommended: false })

const protocolOptions: Array<{ value: GatewayProtocol; labelKey: string; compactLabelKey: string; basePath: string }> = [
  { value: 'open_ai_chat_completions', labelKey: 'gateway.protocolOpenAiChat', compactLabelKey: 'gateway.protocolOpenAiChatCompact', basePath: '/v1' },
  { value: 'open_ai_responses', labelKey: 'gateway.protocolOpenAiResponses', compactLabelKey: 'gateway.protocolOpenAiResponses', basePath: '/v1' },
  { value: 'anthropic_messages', labelKey: 'gateway.protocolAnthropic', compactLabelKey: 'gateway.protocolAnthropic', basePath: '/v1' },
  { value: 'gemini_generate_content', labelKey: 'gateway.protocolGemini', compactLabelKey: 'gateway.protocolGemini', basePath: '/v1beta' }
]

const draft = ref({
  id: undefined as string | undefined,
  name: '',
  protocol: 'open_ai_chat_completions' as GatewayProtocol,
  baseUrl: '',
  enabled: true,
  clientLabel: '',
  dispatchStrategy: 'round_robin' as GatewayDispatchStrategy,
  upstreamKeys: [] as GatewayProfile['upstreamKeys'],
  localKeys: [] as GatewayProfile['localKeys'],
  upstreamModels: [] as GatewayProfile['upstreamModels'],
  credentialRecovery: emptyCredentialRecovery()
})

const locale = computed(() => store.settings.locale)

const baseUrlPreview = ref<GatewayBaseUrlPreview | null>(null)
let previewTimer: ReturnType<typeof setTimeout> | undefined
let previewSeq = 0

async function updateBaseUrlPreview() {
  const raw = draft.value.baseUrl.trim()
  const seq = ++previewSeq
  if (!raw) {
    if (seq === previewSeq) baseUrlPreview.value = null
    return
  }
  try {
    new URL(raw)
  } catch {
    if (seq === previewSeq) baseUrlPreview.value = null
    return
  }
  try {
    const preview = await previewGatewayBaseUrl(draft.value.protocol, raw)
    if (seq === previewSeq) baseUrlPreview.value = preview
  } catch {
    if (seq === previewSeq) baseUrlPreview.value = null
  }
}

watch(
  [() => draft.value.baseUrl, () => draft.value.protocol],
  () => {
    if (previewTimer) clearTimeout(previewTimer)
    previewTimer = setTimeout(updateBaseUrlPreview, 250)
  }
)

const selectedProfile = computed(() => profiles.value.find(profile => profile.id === selectedId.value) ?? null)
const listenerAddress = computed(() => status.value?.listenerAddress || `http://127.0.0.1:${store.settings.proxy.port}`)
const selectedAddress = computed(() => selectedProfile.value ? profileAddress(selectedProfile.value) : '')
const selectedProtocolLabel = computed(() => {
  const option = protocolOptions.find(item => item.value === (selectedProfile.value?.protocol ?? draft.value.protocol))
  return option ? t(locale.value, option.labelKey) : ''
})
const upstreamKeyRecoveryRequired = computed(() => draft.value.credentialRecovery.upstreamKeyRequired)
const localKeyRotationRecommended = computed(() => draft.value.credentialRecovery.localKeyRotationRecommended)
const feedbackMessage = computed(() => {
  if (feedback.value === 'saved') return t(locale.value, 'gateway.saveSuccess')
  if (feedback.value === 'copied') return t(locale.value, 'gateway.copied')
  if (feedback.value !== 'error') return ''

  const errorKey = errorCode.value.startsWith('gateway.')
    ? errorCode.value
    : errorCode.value === 'ERR_GATEWAY_LOCAL_KEY_ALREADY_EXISTS'
      ? 'gateway.singleLocalKeyHint'
    : errorCode.value === 'ERR_GATEWAY_UPSTREAM_KEY_ALREADY_EXISTS'
      ? 'gateway.singleUpstreamKeyHint'
    : errorCode.value === 'ERR_GATEWAY_UPSTREAM_KEY_MIGRATION_REQUIRED'
      ? 'gateway.upstreamKeyMigrationRequired'
    : errorCode.value === 'ERR_GATEWAY_LOCAL_KEY_REGENERATE_REQUIRED'
      ? 'gateway.localKeyRegenerateRequired'
    : errorCode.value.startsWith('ERR_GATEWAY_PROFILE_NAME')
      ? 'gateway.validationName'
      : errorCode.value.startsWith('ERR_GATEWAY_BASE_URL')
        ? 'gateway.validationUrl'
        : 'gateway.operationError'
  return t(locale.value, errorKey)
})

const testErrorMessage = computed(() => {
  if (!testResult.value || testResult.value.ok) return ''
  const kind = testResult.value.errorKind
  if (!kind) return t(locale.value, 'gateway.testFailed')
  const camelKind = kind.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase())
  return t(locale.value, `gateway.modelError.${camelKind}`, { status: testResult.value.httpStatus ?? '' })
})

// —— 状态带（设计文档 10.2）：监听状态 / 地址 / 活动连接 / 近期请求 / 错误率 ——
const listenerStatus = computed(() => status.value?.proxyRunning ?? store.isProxyRunning)
const activeConnections = computed(() => store.proxyStatus?.activeConnections ?? 0)
const recentRequests = computed(() => store.proxyStatus?.totalRequests ?? 0)
const errorRate = computed(() => {
  const proxy = store.proxyStatus
  if (!proxy || proxy.totalRequests <= 0) return '—'
  return `${((proxy.failedRequests / proxy.totalRequests) * 100).toFixed(1)}%`
})

// profile 列表项：最后活动（本地/上游 Key 最近使用时间；无则显示“从未”）
const lastActiveOf = (profile: GatewayProfile): number | null => {
  const times: number[] = []
  for (const key of profile.localKeys) {
    if (key.lastUsedAtMs) times.push(key.lastUsedAtMs)
  }
  for (const key of profile.upstreamKeys) {
    if ('lastUsedAtMs' in key && (key as GatewayUpstreamKey & { lastUsedAtMs?: number }).lastUsedAtMs) {
      times.push((key as GatewayUpstreamKey & { lastUsedAtMs?: number }).lastUsedAtMs as number)
    }
  }
  return times.length > 0 ? Math.max(...times) : null
}
const formatLastActive = (epochMs: number | null) => {
  if (!epochMs) return t(locale.value, 'desktop.gateway.lastActiveNever')
  const diffMs = Date.now() - epochMs
  const minutes = Math.floor(diffMs / 60000)
  if (minutes < 1) return t(locale.value, 'common.justNow')
  if (minutes < 60) return t(locale.value, 'desktop.gateway.lastActiveMinutes', { count: minutes })
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return t(locale.value, 'desktop.gateway.lastActiveHours', { count: hours })
  const days = Math.floor(hours / 24)
  return t(locale.value, 'desktop.gateway.lastActiveDays', { count: days })
}

function clearBaseUrlPreview() {
  baseUrlPreview.value = null
  if (previewTimer) clearTimeout(previewTimer)
  previewTimer = undefined
  previewSeq++
}

function resetDraft() {
  draft.value = { id: undefined, name: '', protocol: 'open_ai_chat_completions', baseUrl: '', enabled: true, clientLabel: '', dispatchStrategy: 'round_robin', upstreamKeys: [], localKeys: [], upstreamModels: [], credentialRecovery: emptyCredentialRecovery() }
  upstreamRemark.value = ''
  upstreamSecret.value = ''
  localRemark.value = ''
  generatedLocalKey.value = ''
  revealedLocalKeys.value = {}
  keyPanel.value = null
  modelsResult.value = null
  modelsPersisted.value = false
  selectedModelId.value = ''
  testResult.value = null
  modelsLoading.value = false
  testingModel.value = false
  clearBaseUrlPreview()
}

function selectProfile(profile: GatewayProfile) {
  selectedId.value = profile.id
  editing.value = true
  draft.value = { ...profile }
  upstreamSecret.value = ''
  feedback.value = null
  errorCode.value = ''
  keyPanel.value = null
  modelsResult.value = profile.upstreamModels.length > 0 ? { ok: true, models: profile.upstreamModels } : null
  modelsPersisted.value = profile.upstreamModels.length > 0
  selectedModelId.value = profile.upstreamModels[0]?.id ?? ''
  testResult.value = null
  modelsLoading.value = false
  testingModel.value = false
  clearBaseUrlPreview()
}

function startNewProfile() {
  activePanel.value = 'manual'
  selectedId.value = null
  editing.value = true
  resetDraft()
  feedback.value = null
  errorCode.value = ''
}

function cancelEditing() {
  editing.value = false
  selectedId.value = null
  resetDraft()
  feedback.value = null
  errorCode.value = ''
  keyPanel.value = null
}

async function load() {
  loading.value = true
  try {
    profiles.value = await listGatewayProfiles()
    status.value = await getGatewayStatus()
    await store.getProxyStatus()
    if (!selectedId.value && profiles.value.length > 0) selectedId.value = profiles.value[0].id
  } catch (error) {
    errorCode.value = String(error)
    feedback.value = 'error'
  } finally {
    loading.value = false
  }
}

function validateDraft() {
  if (!draft.value.name.trim()) return 'gateway.validationName'
  if (!draft.value.id && !upstreamSecret.value.trim()) return 'gateway.validationUpstreamKey'
  try {
    const parsed = new URL(draft.value.baseUrl.trim())
    if (
      parsed.protocol !== 'https:' ||
      !parsed.hostname ||
      parsed.username ||
      parsed.password ||
      parsed.search ||
      parsed.hash ||
      isDisallowedUpstreamHost(parsed.hostname)
    ) {
      return 'gateway.validationUrl'
    }
  } catch {
    return 'gateway.validationUrl'
  }
  return null
}

function isDisallowedUpstreamHost(hostname: string): boolean {
  const host = hostname.replace(/^\[|\]$/g, '').replace(/\.$/, '').toLowerCase()
  if (host === 'localhost') return true

  const ipv4 = host.split('.').map(Number)
  if (ipv4.length === 4 && ipv4.every(part => Number.isInteger(part) && part >= 0 && part <= 255)) {
    const [first, second] = ipv4
    return first === 0 ||
      first === 10 ||
      first === 127 ||
      first >= 224 ||
      (first === 100 && second >= 64 && second <= 127) ||
      (first === 169 && second === 254) ||
      (first === 172 && second >= 16 && second <= 31) ||
      (first === 192 && second === 168)
  }

  if (host === '::' || host === '::1' || host.startsWith('fc') || host.startsWith('fd')) return true
  if (/^fe[89ab]/.test(host) || host.startsWith('ff')) return true
  const mappedIpv4 = host.match(/^::ffff:(\d+\.\d+\.\d+\.\d+)$/)?.[1]
  return mappedIpv4 ? isDisallowedUpstreamHost(mappedIpv4) : false
}

async function saveProfile() {
  const validationKey = validateDraft()
  if (validationKey) {
    errorCode.value = validationKey
    feedback.value = 'error'
    return
  }
  saving.value = true
  feedback.value = null
  errorCode.value = ''
  const input = {
    name: draft.value.name.trim(),
    protocol: draft.value.protocol,
    baseUrl: draft.value.baseUrl.trim().replace(/\/$/, ''),
    enabled: draft.value.enabled,
    clientLabel: draft.value.clientLabel.trim(),
    dispatchStrategy: draft.value.dispatchStrategy,
    upstreamSecret: upstreamSecret.value.trim() || undefined
  }
  try {
    const saved = draft.value.id
      ? await updateGatewayProfile(draft.value.id, input)
      : await createGatewayProfile(input)
    profiles.value = draft.value.id
      ? profiles.value.map(profile => profile.id === saved.id ? saved : profile)
      : [...profiles.value, saved]
    selectedId.value = saved.id
    draft.value = { ...saved }
    editing.value = false
    feedback.value = 'saved'
    await store.loadSettings()
    status.value = await getGatewayStatus()
  } catch (error) {
    errorCode.value = String(error)
    feedback.value = 'error'
  } finally {
    saving.value = false
  }
}

function applyProfile(profile: GatewayProfile) {
  profiles.value = profiles.value.map(item => item.id === profile.id ? profile : item)
  if (draft.value.id === profile.id) draft.value = { ...profile }
}

async function addUpstreamKey() {
  if (!draft.value.id || !upstreamSecret.value.trim()) return
  saving.value = true
  try {
    const profile = await createGatewayUpstreamKey(draft.value.id, {
      remark: upstreamRemark.value.trim(), secret: upstreamSecret.value.trim(), enabled: true, weight: 1, priority: 0
    })
    applyProfile(profile)
    upstreamRemark.value = ''
    upstreamSecret.value = ''
    feedback.value = 'saved'
  } catch (error) {
    errorCode.value = String(error)
    feedback.value = 'error'
  } finally { saving.value = false }
}

async function deleteUpstreamKey(keyId: string) {
  if (!draft.value.id) return
  try {
    applyProfile(await deleteGatewayUpstreamKey(draft.value.id, keyId))
  } catch (error) { errorCode.value = String(error); feedback.value = 'error' }
}

async function updateUpstreamKey(key: GatewayUpstreamKey) {
  if (!draft.value.id || !Number.isInteger(key.weight) || key.weight < 1 || key.weight > 65535 || !Number.isInteger(key.priority) || key.priority < 0 || key.priority > 65535) {
    errorCode.value = 'gateway.operationError'
    feedback.value = 'error'
    return
  }
  saving.value = true
  try {
    applyProfile(await updateGatewayUpstreamKey(draft.value.id, key.id, {
      enabled: key.enabled, weight: key.weight, priority: key.priority
    }))
    feedback.value = 'saved'
  } catch (error) {
    errorCode.value = String(error)
    feedback.value = 'error'
  } finally { saving.value = false }
}

async function revokeLocalKey(keyId: string) {
  if (!draft.value.id) return
  try {
    applyProfile(await revokeGatewayLocalKey(draft.value.id, keyId))
  } catch (error) { errorCode.value = String(error); feedback.value = 'error' }
}

async function createReplacementLocalKey() {
  if (!draft.value.id) return
  saving.value = true
  try {
    const generated = await createGatewayLocalKey(draft.value.id, { remark: localRemark.value.trim() })
    generatedLocalKey.value = generated.key
    const updated = (await listGatewayProfiles()).find(profile => profile.id === draft.value.id)
    if (updated) applyProfile(updated)
    localRemark.value = ''
    feedback.value = 'saved'
  } catch (error) {
    errorCode.value = String(error)
    feedback.value = 'error'
  } finally { saving.value = false }
}

async function revealLocalKey(keyId: string) {
  if (!draft.value.id) return
  try {
    revealedLocalKeys.value[keyId] = await revealGatewayLocalKey(draft.value.id, keyId)
  } catch (error) { errorCode.value = String(error); feedback.value = 'error' }
}

async function copyStoredLocalKey(keyId: string) {
  await revealLocalKey(keyId)
  const key = revealedLocalKeys.value[keyId]
  if (!key) return
  try { await navigator.clipboard.writeText(key); feedback.value = 'copied' }
  catch { errorCode.value = 'gateway.operationError'; feedback.value = 'error' }
}

async function toggleProfile(profile: GatewayProfile) {
  try {
    const updated = await updateGatewayProfile(profile.id, { ...profile, enabled: !profile.enabled })
    profiles.value = profiles.value.map(item => item.id === updated.id ? updated : item)
    if (selectedId.value === updated.id) draft.value = { ...updated }
    status.value = await getGatewayStatus()
  } catch (error) {
    errorCode.value = String(error)
    feedback.value = 'error'
  }
}

async function deleteProfile(profile: GatewayProfile) {
  // 删除 profile 必须确认（安全要求）
  if (!window.confirm(t(locale.value, 'gateway.deleteConfirm'))) return
  try {
    await deleteGatewayProfile(profile.id)
    profiles.value = profiles.value.filter(item => item.id !== profile.id)
    if (selectedId.value === profile.id) cancelEditing()
    status.value = await getGatewayStatus()
  } catch (error) {
    errorCode.value = String(error)
    feedback.value = 'error'
  }
}

async function copyAddress(profile = selectedProfile.value) {
  if (!profile) return
  const option = protocolOptions.find(item => item.value === profile.protocol)
  const address = `${listenerAddress.value}/gateway/${profile.id}${option?.basePath ?? '/v1'}`
  try {
    await navigator.clipboard.writeText(address)
    feedback.value = 'copied'
    window.setTimeout(() => {
      if (feedback.value === 'copied') feedback.value = null
    }, 1800)
  } catch {
    errorCode.value = 'gateway.operationError'
    feedback.value = 'error'
  }
}

async function copyProfileBaseUrl(profile: GatewayProfile) {
  await navigator.clipboard.writeText(profileAddress(profile))
  feedback.value = 'copied'
}

async function copyProfileApiKey(profile: GatewayProfile) {
  const keyMeta = profile.localKeys[0]
  if (!keyMeta) return
  const key = await revealGatewayLocalKey(profile.id, keyMeta.id)
  await navigator.clipboard.writeText(key)
  feedback.value = 'copied'
}

async function refreshModels() {
  if (!draft.value.id || modelsLoading.value) return
  const profileId = draft.value.id
  modelsLoading.value = true
  testResult.value = null
  try {
    const result = await listGatewayUpstreamModels(profileId)
    // 用户可能已切换 profile，过期响应不得写入错误表单
    if (draft.value.id !== profileId) return
    modelsResult.value = result
    selectedModelId.value = result.models[0]?.id ?? ''
    if (result.ok) {
      if (result.models.length > 0) {
        try {
          const saved = await saveGatewayUpstreamModels(profileId, result.models)
          if (draft.value.id !== profileId) return
          modelsPersisted.value = true
          draft.value = { ...draft.value, upstreamModels: saved.upstreamModels }
        } catch {
          modelsPersisted.value = false
        }
      } else {
        modelsPersisted.value = false
      }
    } else {
      modelsPersisted.value = false
    }
  } catch (error) {
    if (draft.value.id !== profileId) return
    modelsResult.value = { ok: false, models: [], errorKind: 'transport_error', errorDetail: String(error) }
    selectedModelId.value = ''
    modelsPersisted.value = false
  } finally {
    if (draft.value.id === profileId) modelsLoading.value = false
  }
}

async function testSelectedModel() {
  if (!draft.value.id || !selectedModelId.value || testingModel.value) return
  const profileId = draft.value.id
  const modelId = selectedModelId.value
  testingModel.value = true
  try {
    testResult.value = await testGatewayUpstreamModel(profileId, modelId)
  } catch (error) {
    if (draft.value.id !== profileId) return
    testResult.value = { ok: false, modelId, errorKind: 'transport_error', errorDetail: String(error) }
  } finally {
    if (draft.value.id === profileId) testingModel.value = false
  }
}

function compactProtocolLabel(protocol: GatewayProtocol) {
  const option = protocolOptions.find(item => item.value === protocol)
  return option ? t(locale.value, option.compactLabelKey) : protocol
}

function profileAddress(profile: GatewayProfile) {
  const option = protocolOptions.find(item => item.value === profile.protocol)
  return `${listenerAddress.value}/gateway/${profile.id}${option?.basePath ?? '/v1'}`
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && editing.value && !saving.value) cancelEditing()
}

onMounted(() => {
  load()
  window.addEventListener('keydown', handleKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeydown)
  if (previewTimer) clearTimeout(previewTimer)
})
</script>

<template>
  <section class="flex flex-col gap-3 pb-4">
    <!-- 顶部状态带：监听状态 / 地址 / 活动连接 / 近期请求 / 错误率；右侧唯一主操作「新增上游」 -->
    <div class="theme-surface flex flex-wrap items-center gap-x-6 gap-y-2 rounded-xl border px-4 py-3">
      <div class="flex items-center gap-2">
        <span
          class="inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[10.5px] font-semibold leading-none"
          :class="listenerStatus ? 'theme-status-success' : 'theme-status-danger'"
        >
          <RadioTower class="h-3 w-3" aria-hidden="true" />
          {{ listenerStatus ? t(locale, 'gateway.running') : t(locale, 'gateway.stopped') }}
        </span>
      </div>
      <div class="min-w-0">
        <div class="text-[9.5px] uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.gateway.address') }}</div>
        <div class="truncate font-mono text-[11px] text-[var(--theme-text-secondary)]" :title="listenerAddress">{{ listenerAddress }}</div>
      </div>
      <div class="flex items-center gap-1.5">
        <Activity class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <span class="text-[11px] text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.gateway.activeConnections') }}: <b class="font-mono">{{ activeConnections }}</b></span>
      </div>
      <div class="flex items-center gap-1.5">
        <RefreshCw class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <span class="text-[11px] text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.gateway.recentRequests') }}: <b class="font-mono">{{ recentRequests }}</b></span>
      </div>
      <div class="flex items-center gap-1.5">
        <AlertTriangle class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <span class="text-[11px] text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.gateway.errorRate') }}: <b class="font-mono" :class="{ 'text-red-500': errorRate !== '—' && parseFloat(errorRate) > 5 }">{{ errorRate }}</b></span>
      </div>
      <!-- 右侧唯一主操作 -->
      <button
        type="button"
        class="theme-button-accent ml-auto inline-flex h-8 items-center gap-1.5 rounded-lg px-3.5 text-[12px] font-semibold"
        @click="startNewProfile"
      >
        <Plus class="h-4 w-4" aria-hidden="true" />
        {{ t(locale, 'gateway.newProfile') }}
      </button>
    </div>

    <!-- 页内 tabs：工具接管 / 手动接入（保持现有领域边界） -->
    <nav class="flex w-fit gap-0.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] p-0.5" :aria-label="t(locale, 'gateway.title')">
      <button
        type="button"
        class="rounded-md px-3.5 py-1 text-[11.5px] font-semibold transition-colors"
        :class="activePanel === 'takeover' ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'"
        :aria-selected="activePanel === 'takeover'"
        @click="activePanel = 'takeover'"
      >
        {{ t(locale, 'gateway.tabs.toolTakeover') }}
      </button>
      <button
        type="button"
        class="rounded-md px-3.5 py-1 text-[11.5px] font-semibold transition-colors"
        :class="activePanel === 'manual' ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'"
        :aria-selected="activePanel === 'manual'"
        @click="activePanel = 'manual'"
      >
        {{ t(locale, 'gateway.tabs.manualAccess') }}
      </button>
    </nav>

    <div v-if="feedback && !editing" :class="['rounded-xl border px-3 py-2 text-[11px] leading-snug', feedback === 'error' ? 'border-red-500/20 bg-red-500/8 text-red-600 dark:text-red-300' : 'border-emerald-500/20 bg-emerald-500/8 text-emerald-600 dark:text-emerald-300']">
      {{ feedbackMessage }}
    </div>

    <template v-if="activePanel === 'takeover'">
      <ProxyControlPanel />
      <CcSwitchCompatPanel />
    </template>

    <!-- 手动接入：两栏（左 profile 列表 320px + 右详情/编辑） -->
    <template v-else>
      <div v-if="loading" class="flex justify-center py-12">
        <div class="h-5 w-5 animate-spin rounded-full border-2 border-[var(--theme-border-strong)] border-t-[var(--theme-accent-primary)]"></div>
      </div>
      <div v-else class="flex min-h-0 items-start gap-3">
        <!-- 左：profile 列表 -->
        <aside class="w-80 shrink-0">
          <div class="theme-surface overflow-hidden rounded-xl border">
            <div class="flex items-center justify-between border-b border-[var(--theme-border-default)] px-3 py-2">
              <div>
                <h2 class="text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'gateway.profiles') }}</h2>
                <p class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.profileCount', { count: profiles.length }) }}</p>
              </div>
              <button
                type="button"
                class="theme-icon-button rounded-lg p-1.5"
                :title="t(locale, 'gateway.newProfile')"
                :aria-label="t(locale, 'gateway.newProfile')"
                @click="startNewProfile"
              >
                <Plus class="h-4 w-4" aria-hidden="true" />
              </button>
            </div>

            <div v-if="profiles.length === 0" class="px-4 py-10 text-center">
              <RadioTower class="mx-auto h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
              <p class="mt-2 text-[12px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.emptyTitle') }}</p>
              <p class="mx-auto mt-1 max-w-[220px] text-[10.5px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.emptyBody') }}</p>
            </div>

            <div v-else class="max-h-[calc(100vh-300px)] overflow-y-auto">
              <article
                v-for="profile in profiles"
                :key="profile.id"
                class="cursor-pointer border-b border-[var(--theme-border-subtle)] px-3 py-2.5 transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)]"
                :class="selectedId === profile.id ? 'bg-[var(--theme-accent-soft)]' : ''"
                @click="selectProfile(profile)"
              >
                <div class="flex items-center gap-2">
                  <div class="min-w-0 flex-1">
                    <div class="flex min-w-0 items-center gap-1.5">
                      <span class="truncate text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ profile.name }}</span>
                      <span class="shrink-0 rounded-md border border-[var(--theme-border-default)] bg-[var(--theme-bg-hover)] px-1.5 py-0.5 text-[9px] font-medium text-[var(--theme-text-tertiary)]">{{ compactProtocolLabel(profile.protocol) }}</span>
                    </div>
                    <p class="mt-1 flex items-center gap-1.5 text-[10px] text-[var(--theme-text-tertiary)]">
                      <span class="inline-flex items-center gap-1">
                        <span class="h-1.5 w-1.5 rounded-full" :class="profile.enabled ? 'bg-emerald-500' : 'bg-[var(--theme-border-strong)]'"></span>
                        {{ profile.enabled ? t(locale, 'desktop.gateway.profileEnabled') : t(locale, 'desktop.gateway.profileDisabled') }}
                      </span>
                      <span>·</span>
                      <span>{{ t(locale, 'desktop.gateway.modelCount', { count: profile.upstreamModels.length }) }}</span>
                      <span>·</span>
                      <span>{{ t(locale, 'desktop.gateway.lastActivity') }} {{ formatLastActive(lastActiveOf(profile)) }}</span>
                    </p>
                  </div>
                  <!-- 列表行操作：复制地址 / 复制本地 Key / 编辑 / 删除（均带 aria-label） -->
                  <div class="flex shrink-0 items-center gap-0.5">
                    <button type="button" class="theme-icon-button rounded-md p-1" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click.stop="copyProfileBaseUrl(profile)"><Link2 class="h-3.5 w-3.5" aria-hidden="true" /></button>
                    <button type="button" class="theme-icon-button rounded-md p-1" :title="t(locale, 'gateway.copyLocalKey')" :aria-label="t(locale, 'gateway.copyLocalKey')" :disabled="profile.localKeys.length === 0" @click.stop="copyProfileApiKey(profile)"><KeyRound class="h-3.5 w-3.5" aria-hidden="true" /></button>
                    <button type="button" class="theme-icon-button rounded-md p-1" :title="t(locale, 'gateway.editProfile')" :aria-label="t(locale, 'gateway.editProfile')" @click.stop="selectProfile(profile)"><Pencil class="h-3.5 w-3.5" aria-hidden="true" /></button>
                    <button type="button" class="theme-icon-button rounded-md p-1 text-red-500" :title="t(locale, 'gateway.delete')" :aria-label="t(locale, 'gateway.delete')" @click.stop="deleteProfile(profile)"><Trash2 class="h-3.5 w-3.5" aria-hidden="true" /></button>
                    <SettingsSwitch compact :checked="profile.enabled" :aria-label="t(locale, 'gateway.enabled')" @toggle="toggleProfile(profile)" />
                  </div>
                </div>
                <p class="mt-1.5 break-all border-t border-[var(--theme-border-default)] pt-1.5 font-mono text-[9.5px] leading-snug text-[var(--theme-text-tertiary)]">{{ profileAddress(profile) }}</p>
                <p v-if="profile.credentialRecovery.upstreamKeyRequired" class="mt-1 text-[9.5px] leading-snug text-amber-600 dark:text-amber-300">{{ t(locale, 'gateway.upstreamKeyRecoveryNotice') }}</p>
                <p v-if="profile.credentialRecovery.localKeyRotationRecommended" class="mt-1 text-[9.5px] leading-snug text-amber-600 dark:text-amber-300">{{ t(locale, 'gateway.localKeyRecoveryNotice') }}</p>
              </article>
            </div>
          </div>
        </aside>

        <!-- 右：详情 / 编辑表单（内嵌，非 modal） -->
        <div class="min-w-0 flex-1">
          <div v-if="!editing && !selectedProfile" class="theme-surface flex h-56 flex-col items-center justify-center rounded-xl border px-6 text-center">
            <ShieldCheck class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <p class="mt-2 text-[12.5px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.gateway.selectProfileHint') }}</p>
            <p class="mt-1 max-w-sm text-[11px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.gateway.selectProfileHintDesc') }}</p>
          </div>

          <form v-else class="theme-surface overflow-hidden rounded-xl border" role="dialog" aria-modal="false" @submit.prevent="saveProfile">
            <!-- 表单头部 -->
            <header class="flex items-start justify-between border-b border-[var(--theme-border-default)] px-4 py-3">
              <div class="min-w-0">
                <h2 class="text-[14px] font-bold text-[var(--theme-text-primary)]">{{ draft.id ? t(locale, 'gateway.editProfile') : t(locale, 'gateway.newProfile') }}</h2>
                <p class="mt-0.5 truncate text-[10px] text-[var(--theme-text-tertiary)]">{{ selectedProtocolLabel }}</p>
              </div>
              <button type="button" class="theme-icon-button -mr-1 -mt-1 rounded-lg p-2" :title="t(locale, 'gateway.cancel')" :aria-label="t(locale, 'gateway.cancel')" @click="cancelEditing"><X class="h-4 w-4" aria-hidden="true" /></button>
            </header>

            <div class="space-y-2.5 p-4">
              <!-- 基本信息 -->
              <section v-if="!keyPanel" class="theme-surface-muted overflow-hidden rounded-xl border">
                <label class="flex h-11 items-center gap-3 px-3">
                  <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.name') }}</span>
                  <input v-model="draft.name" class="min-w-0 flex-1 bg-transparent text-right text-[12px] font-semibold text-[var(--theme-text-primary)] outline-none placeholder:font-normal placeholder:text-[var(--theme-text-quaternary)]" :placeholder="t(locale, 'gateway.namePlaceholder')" />
                </label>
                <label class="flex h-11 items-center gap-3 border-t border-[var(--theme-border-default)] px-3">
                  <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.protocol') }}</span>
                  <select v-model="draft.protocol" class="min-w-0 flex-1 appearance-none bg-transparent py-0.5 text-right text-[11.5px] font-semibold text-[var(--theme-text-primary)] outline-none">
                    <option v-for="option in protocolOptions" :key="option.value" :value="option.value">{{ t(locale, option.labelKey) }}</option>
                  </select>
                </label>
                <label class="flex h-11 items-center gap-3 border-t border-[var(--theme-border-default)] px-3">
                  <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.baseUrl') }}</span>
                  <input v-model="draft.baseUrl" class="min-w-0 flex-1 bg-transparent text-right font-mono text-[10.5px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]" :placeholder="t(locale, 'gateway.baseUrlPlaceholder')" inputmode="url" />
                </label>
                <div v-if="baseUrlPreview" class="border-t border-[var(--theme-border-default)] px-3 py-2">
                  <p class="break-all font-mono text-[10px] leading-relaxed text-[var(--theme-text-primary)]">POST {{ baseUrlPreview.sampleRequestUrl }}</p>
                  <p v-if="!baseUrlPreview.basePathAdded" class="mt-1 text-right text-[9px] leading-none text-emerald-600/90 dark:text-emerald-400/90">
                    {{ t(locale, 'gateway.basePathAlreadyIncluded', { path: baseUrlPreview.basePath }) }}
                  </p>
                </div>
                <label class="flex h-11 items-center gap-3 border-t border-[var(--theme-border-default)] px-3">
                  <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.upstreamKey') }}</span>
                  <!-- 上游 key 密码框：创建后不再回显 -->
                  <input v-model="upstreamSecret" type="password" class="min-w-0 flex-1 bg-transparent text-right font-mono text-[10.5px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]" :placeholder="upstreamKeyRecoveryRequired ? t(locale, 'gateway.upstreamKeyRecoveryPlaceholder') : (draft.id ? t(locale, 'gateway.upstreamKeyKeepHint') : t(locale, 'gateway.upstreamKeyPlaceholder'))" autocomplete="off" />
                </label>
                <label class="flex h-11 cursor-pointer items-center justify-between border-t border-[var(--theme-border-default)] px-3">
                  <span class="text-[10.5px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.enabled') }}</span>
                  <input v-model="draft.enabled" type="checkbox" class="peer sr-only" />
                  <span class="relative h-5 w-9 rounded-full bg-[var(--theme-border-strong)] transition-colors peer-checked:bg-[var(--theme-accent-primary)] after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:shadow-sm after:transition-transform peer-checked:after:translate-x-4 peer-focus-visible:ring-2 peer-focus-visible:ring-[var(--theme-accent-primary)] peer-focus-visible:ring-offset-2"></span>
                </label>
              </section>

              <!-- 凭据 / 地址 / 模型测试 -->
              <template v-if="!keyPanel">
                <div v-if="upstreamKeyRecoveryRequired || localKeyRotationRecommended" class="rounded-xl border border-amber-500/25 bg-amber-500/8 px-3 py-2 text-[9.5px] leading-snug text-amber-700 dark:text-amber-200">
                  <p v-if="upstreamKeyRecoveryRequired">{{ t(locale, 'gateway.upstreamKeyRecoveryNotice') }}</p>
                  <p v-if="localKeyRotationRecommended" :class="{ 'mt-1': upstreamKeyRecoveryRequired }">{{ t(locale, 'gateway.localKeyRecoveryNotice') }}</p>
                </div>
                <p class="px-1 text-[9px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.managedKeyHint') }}</p>
                <div v-if="draft.id" class="theme-surface-muted rounded-xl border px-3 py-2.5">
                  <div class="flex items-center justify-between gap-2"><span class="text-[9.5px] font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.address') }}</span><div class="flex items-center gap-1"><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.localKeys')" :aria-label="t(locale, 'gateway.localKeys')" @click="keyPanel = 'local'"><KeyRound class="h-3.5 w-3.5" aria-hidden="true" /></button><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click="copyAddress()"><Copy class="h-3.5 w-3.5" aria-hidden="true" /></button></div></div>
                  <p class="mt-1 break-all font-mono text-[9px] leading-snug text-[var(--theme-text-primary)]">{{ selectedAddress }}</p>
                </div>
                <div v-if="draft.id" class="theme-surface-muted rounded-xl border px-3 py-2.5">
                  <div class="flex items-center justify-between gap-2"><span class="text-[9.5px] font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelTest') }}</span><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.refreshModels')" :aria-label="t(locale, 'gateway.refreshModels')" :disabled="modelsLoading" @click="refreshModels"><RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': modelsLoading }" aria-hidden="true" /></button></div>
                  <p v-if="modelsLoading" class="mt-1 text-[9px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsLoading') }}</p>
                  <p v-else-if="modelsResult && !modelsResult.ok" class="mt-1 text-[9px] text-red-500">{{ t(locale, 'gateway.modelsFailed') }}</p>
                  <p v-else-if="modelsResult && modelsResult.ok && modelsPersisted && modelsResult.models.length > 0" class="mt-1 text-[9px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsSaved', { count: modelsResult.models.length }) }}</p>
                  <p v-else-if="modelsResult && modelsResult.ok && !modelsPersisted && modelsResult.models.length > 0" class="mt-1 text-[9px] text-amber-500">{{ t(locale, 'gateway.modelsSaveFailed') }}</p>
                  <p v-else-if="modelsResult && modelsResult.ok && modelsResult.models.length === 0" class="mt-1 text-[9px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsEmpty') }}</p>
                  <div v-if="modelsResult && modelsResult.models.length > 0" class="mt-1.5 max-h-[120px] space-y-1 overflow-y-auto">
                    <label v-for="model in modelsResult.models" :key="model.id" class="flex cursor-pointer items-center gap-2 rounded-lg border px-2 py-1.5 text-[10px]" :class="selectedModelId === model.id ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-primary)]/5' : 'border-[var(--theme-border-default)]'"><input v-model="selectedModelId" :value="model.id" type="radio" class="sr-only" /><span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="selectedModelId === model.id ? 'bg-[var(--theme-accent-primary)]' : 'bg-[var(--theme-border-strong)]'"></span><span class="min-w-0 flex-1 truncate font-mono text-[var(--theme-text-secondary)]">{{ model.id }}</span></label>
                  </div>
                  <div v-if="modelsResult && modelsResult.models.length > 0" class="mt-2 flex items-center gap-2"><button type="button" class="rounded-lg border border-[var(--theme-border-default)] px-2.5 py-1 text-[10px] font-semibold text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)] disabled:opacity-50" :disabled="testingModel || !selectedModelId" @click="testSelectedModel">{{ testingModel ? t(locale, 'gateway.testingModel') : t(locale, 'gateway.testModel') }}</button><span v-if="testResult && testResult.ok" class="text-[9.5px] text-emerald-600 dark:text-emerald-300">{{ t(locale, 'gateway.testSuccess', { ms: testResult.latencyMs ?? 0 }) }}</span><span v-else-if="testResult && !testResult.ok" class="text-[9.5px] text-red-500">{{ testErrorMessage }}</span></div>
                </div>
                <p class="px-1 text-[9px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.singleUpstreamKeyHint') }}</p>
              </template>

              <!-- Key 子面板（上游 / 本地） -->
              <template v-else>
                <button type="button" class="inline-flex items-center gap-1 text-[10px] font-semibold text-[var(--theme-text-secondary)]" @click="keyPanel = null"><span class="text-[15px] leading-none">‹</span>{{ draft.name }}</button>
                <section class="theme-surface-muted overflow-hidden rounded-xl border">
                  <div class="border-b border-[var(--theme-border-default)] px-3 py-2.5"><h3 class="text-[11px] font-semibold text-[var(--theme-text-primary)]">{{ keyPanel === 'upstream' ? t(locale, 'gateway.upstreamKeys') : t(locale, 'gateway.localKeys') }}</h3><p class="mt-0.5 text-[9px] text-[var(--theme-text-tertiary)]">{{ keyPanel === 'upstream' ? t(locale, 'gateway.upstreamKeyHint') : t(locale, 'gateway.localKeyHint') }}</p></div>
                  <div class="p-2">
                    <div v-if="keyPanel === 'upstream' && draft.upstreamKeys.length === 0" class="flex gap-1.5"><input v-model="upstreamRemark" class="min-w-0 w-20 flex-1 rounded-lg border border-[var(--theme-border-default)] bg-transparent px-2 py-2 text-[10px] text-[var(--theme-text-primary)] outline-none" :placeholder="t(locale, 'gateway.keyRemark')" /><input v-model="upstreamSecret" type="password" class="min-w-0 flex-[1.6] rounded-lg border border-[var(--theme-border-default)] bg-transparent px-2 py-2 font-mono text-[10px] text-[var(--theme-text-primary)] outline-none" :placeholder="t(locale, 'gateway.upstreamKeyPlaceholder')" autocomplete="off" /><button type="button" class="theme-icon-button rounded-lg p-1.5" :title="t(locale, 'gateway.addKey')" :aria-label="t(locale, 'gateway.addKey')" :disabled="saving || !upstreamSecret.trim()" @click="addUpstreamKey"><Plus class="h-3.5 w-3.5" aria-hidden="true" /></button></div>
                    <div v-else-if="keyPanel === 'upstream'" class="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2 text-[10px] text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.singleUpstreamKeyHint') }}</div>
                    <div v-else-if="draft.localKeys.length === 0" class="flex gap-1.5"><input v-model="localRemark" class="min-w-0 flex-1 rounded-lg border border-[var(--theme-border-default)] bg-transparent px-2 py-2 text-[10px] text-[var(--theme-text-primary)] outline-none" :placeholder="t(locale, 'gateway.keyRemark')" /><button type="button" class="theme-icon-button rounded-lg p-1.5" :title="t(locale, 'gateway.createLocalKey')" :aria-label="t(locale, 'gateway.createLocalKey')" :disabled="saving" @click="createReplacementLocalKey"><Plus class="h-3.5 w-3.5" aria-hidden="true" /></button></div>
                    <div v-else class="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2 text-[10px] text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.singleLocalKeyHint') }}</div>
                    <div v-for="key in keyPanel === 'upstream' ? draft.upstreamKeys : draft.localKeys" :key="key.id" class="mt-1.5 rounded-lg border border-[var(--theme-border-default)] px-2 py-1.5 text-[10px]"><div class="flex h-6 items-center gap-2"><span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="key.enabled ? 'bg-emerald-500' : 'bg-gray-400'"></span><span class="min-w-0 flex-1 truncate text-[var(--theme-text-secondary)]">{{ key.remark || t(locale, 'gateway.unnamedKey') }}</span><template v-if="keyPanel === 'local'"><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.showLocalKey')" :aria-label="t(locale, 'gateway.showLocalKey')" @click="revealLocalKey(key.id)"><Eye class="h-3 w-3" aria-hidden="true" /></button><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.copyLocalKey')" :aria-label="t(locale, 'gateway.copyLocalKey')" @click="copyStoredLocalKey(key.id)"><Copy class="h-3 w-3" aria-hidden="true" /></button></template><button type="button" class="theme-icon-button rounded-lg p-1" :title="keyPanel === 'upstream' ? t(locale, 'gateway.deleteKey') : t(locale, 'gateway.revokeKey')" @click="keyPanel === 'upstream' ? deleteUpstreamKey(key.id) : revokeLocalKey(key.id)"><Trash2 class="h-3 w-3 text-red-500" aria-hidden="true" /></button></div><div v-if="keyPanel === 'upstream'" class="mt-1 flex items-center gap-2 border-t border-[var(--theme-border-default)] pt-1"><label class="flex items-center gap-1 text-[9px] text-[var(--theme-text-tertiary)]"><input v-model="key.enabled" type="checkbox" class="h-3 w-3 accent-[var(--theme-accent-primary)]" :aria-label="t(locale, 'gateway.enabled')" @change="updateUpstreamKey(key as GatewayUpstreamKey)" />{{ t(locale, 'gateway.enabled') }}</label></div><p v-if="keyPanel === 'local' && revealedLocalKeys[key.id]" class="mt-1 break-all border-t border-[var(--theme-border-default)] pt-1 font-mono text-[9px] text-[var(--theme-text-primary)]">{{ revealedLocalKeys[key.id] }}</p></div>
                  </div>
                </section>
              </template>

              <p v-if="feedback === 'error'" class="text-[10px] leading-snug text-red-500">{{ feedbackMessage }}</p>
              <p v-else-if="feedback === 'saved'" class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" aria-hidden="true" />{{ t(locale, 'gateway.saveSuccess') }}</p>
              <p v-else-if="feedback === 'copied'" class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" aria-hidden="true" />{{ t(locale, 'gateway.copied') }}</p>
            </div>

            <!-- 表单底部 -->
            <footer class="flex gap-2 border-t border-[var(--theme-border-default)] px-4 py-3">
              <button v-if="keyPanel" type="button" class="inline-flex flex-1 items-center justify-center rounded-lg border border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="keyPanel = null">{{ t(locale, 'gateway.cancel') }}</button>
              <template v-else><button type="button" class="rounded-lg border border-[var(--theme-border-default)] px-3 text-[10.5px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="cancelEditing">{{ t(locale, 'gateway.cancel') }}</button><button type="submit" class="inline-flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3 py-2 text-[10.5px] font-semibold text-[var(--theme-accent-contrast)] shadow-sm transition-opacity disabled:opacity-50" :disabled="saving"><Save class="h-3.5 w-3.5" aria-hidden="true" />{{ draft.id ? t(locale, 'gateway.update') : t(locale, 'gateway.save') }}</button></template>
            </footer>
          </form>
        </div>
      </div>
    </template>
  </section>
</template>
