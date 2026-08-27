<script setup lang="ts">
/**
 * 桌面主窗口「网关」页（设计文档第 10 章）。
 * 业务逻辑整体复用 src/views/Gateway.vue（同一领域合同），布局按桌面密度重排：
 * 顶部状态带（监听状态/地址/活动连接/近期请求/错误率 + 唯一主操作「新增上游」），
 * 主体两栏（左 profile 列表 320px，右详情/编辑表单内嵌）。
 * 安全行为全部保留：key 默认遮罩、显式点击查看本地 key、创建后不再回显、
 * 复制 toast 不含 key、删除/撤销/停止接管需确认、敏感字段不进日志。
 *
 * 本文件仅保留状态持有与编排：状态带 / profile 列表 / 编辑表单分别拆至
 * gateway/GatewayStatusBar.vue、gateway/GatewayProfileList.vue、gateway/GatewayProfileEditor.vue。
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { ShieldCheck } from 'lucide-vue-next'
import {
  createGatewayProfile,
  deleteGatewayProfile,
  getGatewayStatus,
  listGatewayProfiles,
  revealGatewayLocalKey,
  updateGatewayProfile
} from '../../api/gatewayApi'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import { gatewayProtocolOptions } from './gateway/protocolOptions'
import type { GatewayCredentialRecovery, GatewayDispatchStrategy, GatewayLocalKey, GatewayProfile, GatewayProtocol, GatewayStatus, GatewayUpstreamKey, GatewayUpstreamModel } from '../../types'
import ProxyControlPanel from '../../components/settings/ProxyControlPanel.vue'
import CcSwitchCompatPanel from '../../components/settings/CcSwitchCompatPanel.vue'
import GatewayStatusBar from './gateway/GatewayStatusBar.vue'
import GatewayProfileList from './gateway/GatewayProfileList.vue'
import GatewayProfileEditor from './gateway/GatewayProfileEditor.vue'

interface DraftProfile {
  id?: string
  name: string
  protocol: GatewayProtocol
  baseUrl: string
  enabled: boolean
  clientLabel: string
  dispatchStrategy: GatewayDispatchStrategy
  upstreamKeys: GatewayUpstreamKey[]
  localKeys: GatewayLocalKey[]
  upstreamModels: GatewayUpstreamModel[]
  credentialRecovery: GatewayCredentialRecovery
}

interface GatewayDraftInput {
  name: string
  protocol: GatewayProtocol
  baseUrl: string
  enabled: boolean
  clientLabel: string
  dispatchStrategy: GatewayDispatchStrategy
  upstreamSecret: string
}

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
const resetKey = ref(0)
const draft = ref<DraftProfile>({
  id: undefined,
  name: '',
  protocol: 'open_ai_chat_completions',
  baseUrl: '',
  enabled: true,
  clientLabel: '',
  dispatchStrategy: 'round_robin' as GatewayDispatchStrategy,
  upstreamKeys: [],
  localKeys: [],
  upstreamModels: [],
  credentialRecovery: emptyCredentialRecovery()
})

function emptyCredentialRecovery(): GatewayCredentialRecovery {
  return { upstreamKeyRequired: false, localKeyRotationRecommended: false }
}

const locale = computed(() => store.settings.locale)
const selectedProfile = computed(() => profiles.value.find(profile => profile.id === selectedId.value) ?? null)
const listenerAddress = computed(() => status.value?.listenerAddress || `http://127.0.0.1:${store.settings.proxy.port}`)
const selectedAddress = computed(() => {
  const option = gatewayProtocolOptions.find(item => item.value === selectedProfile.value?.protocol)
  return selectedProfile.value ? `${listenerAddress.value}/gateway/${selectedProfile.value.id}${option?.basePath ?? '/v1'}` : ''
})
const selectedProtocolLabel = computed(() => {
  const option = gatewayProtocolOptions.find(item => item.value === (selectedProfile.value?.protocol ?? draft.value.protocol))
  return option ? t(locale.value, option.labelKey) : ''
})
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

// —— 状态带（设计文档 10.2）：监听状态 / 地址 / 活动连接 / 近期请求 / 错误率 ——
const listenerStatus = computed(() => status.value?.proxyRunning ?? store.isProxyRunning)
const activeConnections = computed(() => store.proxyStatus?.activeConnections ?? 0)
const recentRequests = computed(() => store.proxyStatus?.totalRequests ?? 0)
const errorRate = computed(() => {
  const proxy = store.proxyStatus
  if (!proxy || proxy.totalRequests <= 0) return '—'
  return `${((proxy.failedRequests / proxy.totalRequests) * 100).toFixed(1)}%`
})

function resetDraft() {
  draft.value = { id: undefined, name: '', protocol: 'open_ai_chat_completions', baseUrl: '', enabled: true, clientLabel: '', dispatchStrategy: 'round_robin', upstreamKeys: [], localKeys: [], upstreamModels: [], credentialRecovery: emptyCredentialRecovery() }
}

function selectProfile(profile: GatewayProfile) {
  selectedId.value = profile.id
  editing.value = true
  draft.value = { ...profile }
  feedback.value = null
  errorCode.value = ''
}

function startNewProfile() {
  activePanel.value = 'manual'
  selectedId.value = null
  editing.value = true
  resetDraft()
  resetKey.value++
  feedback.value = null
  errorCode.value = ''
}

function cancelEditing() {
  editing.value = false
  selectedId.value = null
  resetDraft()
  resetKey.value++
  feedback.value = null
  errorCode.value = ''
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

function validateDraft(input: GatewayDraftInput) {
  if (!input.name.trim()) return 'gateway.validationName'
  if (!draft.value.id && !input.upstreamSecret.trim()) return 'gateway.validationUpstreamKey'
  try {
    const parsed = new URL(input.baseUrl.trim())
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

async function saveProfile(input: GatewayDraftInput) {
  const validationKey = validateDraft(input)
  if (validationKey) {
    errorCode.value = validationKey
    feedback.value = 'error'
    return
  }
  saving.value = true
  feedback.value = null
  errorCode.value = ''
  const payload = {
    name: input.name.trim(),
    protocol: input.protocol,
    baseUrl: input.baseUrl.trim().replace(/\/$/, ''),
    enabled: input.enabled,
    clientLabel: input.clientLabel.trim(),
    dispatchStrategy: input.dispatchStrategy,
    upstreamSecret: input.upstreamSecret.trim() || undefined
  }
  try {
    const saved = draft.value.id
      ? await updateGatewayProfile(draft.value.id, payload)
      : await createGatewayProfile(payload)
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

function updateDraft(partial: Partial<DraftProfile>) {
  draft.value = { ...draft.value, ...partial }
}

function handleNotice(payload: { kind: 'saved' | 'copied' | 'error'; errorCode?: string }) {
  feedback.value = payload.kind
  errorCode.value = payload.errorCode ?? ''
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
  const option = gatewayProtocolOptions.find(item => item.value === profile.protocol)
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

function profileAddress(profile: GatewayProfile) {
  const option = gatewayProtocolOptions.find(item => item.value === profile.protocol)
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
})
</script>

<template>
  <section class="flex h-full flex-col gap-3">
    <!-- 顶部状态带：监听状态 / 地址 / 活动连接 / 近期请求 / 错误率；右侧唯一主操作「新增上游」 -->
    <GatewayStatusBar
      :listener-status="listenerStatus"
      :listener-address="listenerAddress"
      :active-connections="activeConnections"
      :recent-requests="recentRequests"
      :error-rate="errorRate"
      @create="startNewProfile"
    />

    <!-- 页内 tabs：工具接管 / 手动接入（保持现有领域边界） -->
    <nav class="flex w-fit shrink-0 gap-0.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] p-0.5" :aria-label="t(locale, 'gateway.title')">
      <button
        type="button"
        class="rounded-md px-3.5 py-1 text-xs font-semibold transition-colors"
        :class="activePanel === 'takeover' ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'"
        :aria-selected="activePanel === 'takeover'"
        @click="activePanel = 'takeover'"
      >
        {{ t(locale, 'gateway.tabs.toolTakeover') }}
      </button>
      <button
        type="button"
        class="rounded-md px-3.5 py-1 text-xs font-semibold transition-colors"
        :class="activePanel === 'manual' ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'"
        :aria-selected="activePanel === 'manual'"
        @click="activePanel = 'manual'"
      >
        {{ t(locale, 'gateway.tabs.manualAccess') }}
      </button>
    </nav>

    <div v-if="feedback && !editing" :class="['shrink-0 rounded-lg border px-3 py-2 text-xs leading-snug', feedback === 'error' ? 'border-red-500/20 bg-red-500/8 text-red-600 dark:text-red-300' : 'border-emerald-500/20 bg-emerald-500/8 text-emerald-600 dark:text-emerald-300']">
      {{ feedbackMessage }}
    </div>

    <template v-if="activePanel === 'takeover'">
      <div class="min-h-0 flex-1 overflow-y-auto">
        <ProxyControlPanel />
        <CcSwitchCompatPanel />
      </div>
    </template>

    <!-- 手动接入：两栏（左 profile 列表 + 右详情/编辑） -->
    <template v-else>
      <div v-if="loading" class="flex flex-1 items-center justify-center">
        <div class="h-5 w-5 animate-spin rounded-full border-2 border-[var(--theme-border-strong)] border-t-[var(--theme-accent-primary)]"></div>
      </div>
      <div v-else class="flex min-h-0 flex-1 gap-3">
        <!-- 左：profile 列表 -->
        <GatewayProfileList
          :profiles="profiles"
          :selected-id="selectedId"
          @select="selectProfile"
          @create="startNewProfile"
          @toggle="toggleProfile"
          @delete="deleteProfile"
          @copy-address="copyProfileBaseUrl"
          @copy-api-key="copyProfileApiKey"
        />

        <!-- 右：详情 / 编辑表单 -->
        <div class="min-w-0 flex-1 overflow-y-auto">
          <div v-if="!editing && !selectedProfile" class="flex h-full flex-col items-center justify-center rounded-lg border text-center" style="background: var(--theme-surface-gradient)">
            <ShieldCheck class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <p class="mt-2 text-xs font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.gateway.selectProfileHint') }}</p>
            <p class="mt-1 max-w-sm text-xs leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.gateway.selectProfileHintDesc') }}</p>
          </div>

          <GatewayProfileEditor
            v-else
            :draft="draft"
            :selected-address="selectedAddress"
            :selected-protocol-label="selectedProtocolLabel"
            :saving="saving"
            :feedback="feedback"
            :feedback-message="feedbackMessage"
            :reset-key="resetKey"
            @submit="saveProfile"
            @cancel="cancelEditing"
            @apply-profile="applyProfile"
            @update-draft="updateDraft"
            @notice="handleNotice"
            @copy-address="copyAddress"
          />
        </div>
      </div>
    </template>
  </section>
</template>
