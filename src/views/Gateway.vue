<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { Check, Copy, Eye, KeyRound, Link2, Pencil, Plus, RadioTower, Save, Trash2, X } from 'lucide-vue-next'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import type { GatewayDispatchStrategy, GatewayProfile, GatewayProtocol, GatewayStatus, GatewayUpstreamKey } from '../types'
import ProxyControlPanel from '../components/settings/ProxyControlPanel.vue'
import CcSwitchCompatPanel from '../components/settings/CcSwitchCompatPanel.vue'
import SettingsSwitch from '../components/settings/SettingsSwitch.vue'

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
  localKeys: [] as GatewayProfile['localKeys']
})

const locale = computed(() => store.settings.locale)
const selectedProfile = computed(() => profiles.value.find(profile => profile.id === selectedId.value) ?? null)
const listenerAddress = computed(() => status.value?.listenerAddress || `http://127.0.0.1:${store.settings.proxy.port}`)
const selectedAddress = computed(() => selectedProfile.value ? profileAddress(selectedProfile.value) : '')
const selectedProtocolLabel = computed(() => {
  const option = protocolOptions.find(item => item.value === (selectedProfile.value?.protocol ?? draft.value.protocol))
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

function resetDraft() {
  draft.value = { id: undefined, name: '', protocol: 'open_ai_chat_completions', baseUrl: '', enabled: true, clientLabel: '', dispatchStrategy: 'round_robin', upstreamKeys: [], localKeys: [] }
  upstreamRemark.value = ''
  upstreamSecret.value = ''
  localRemark.value = ''
  generatedLocalKey.value = ''
  revealedLocalKeys.value = {}
  keyPanel.value = null
}

function selectProfile(profile: GatewayProfile) {
  selectedId.value = profile.id
  editing.value = true
  draft.value = { ...profile }
  upstreamSecret.value = ''
  feedback.value = null
  errorCode.value = ''
  keyPanel.value = null
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
    profiles.value = await invoke<GatewayProfile[]>('list_gateway_profiles')
    status.value = await invoke<GatewayStatus>('get_gateway_status')
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
      ? await invoke<GatewayProfile>('update_gateway_profile', { id: draft.value.id, input })
      : await invoke<GatewayProfile>('create_gateway_profile', { input })
    profiles.value = draft.value.id
      ? profiles.value.map(profile => profile.id === saved.id ? saved : profile)
      : [...profiles.value, saved]
    selectedId.value = saved.id
    draft.value = { ...saved }
    editing.value = false
    feedback.value = 'saved'
    await store.loadSettings()
    status.value = await invoke<GatewayStatus>('get_gateway_status')
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
    const profile = await invoke<GatewayProfile>('create_gateway_upstream_key', {
      profileId: draft.value.id,
      input: { remark: upstreamRemark.value.trim(), secret: upstreamSecret.value.trim(), enabled: true, weight: 1, priority: 0 }
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
    applyProfile(await invoke<GatewayProfile>('delete_gateway_upstream_key', { profileId: draft.value.id, keyId }))
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
    applyProfile(await invoke<GatewayProfile>('update_gateway_upstream_key', {
      profileId: draft.value.id,
      keyId: key.id,
      input: { enabled: key.enabled, weight: key.weight, priority: key.priority }
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
    applyProfile(await invoke<GatewayProfile>('revoke_gateway_local_key', { profileId: draft.value.id, keyId }))
  } catch (error) { errorCode.value = String(error); feedback.value = 'error' }
}

async function revealLocalKey(keyId: string) {
  if (!draft.value.id) return
  try {
    revealedLocalKeys.value[keyId] = await invoke<string>('reveal_gateway_local_key', { profileId: draft.value.id, keyId })
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
    const updated = await invoke<GatewayProfile>('update_gateway_profile', {
      id: profile.id,
      input: { ...profile, enabled: !profile.enabled }
    })
    profiles.value = profiles.value.map(item => item.id === updated.id ? updated : item)
    if (selectedId.value === updated.id) draft.value = { ...updated }
    status.value = await invoke<GatewayStatus>('get_gateway_status')
  } catch (error) {
    errorCode.value = String(error)
    feedback.value = 'error'
  }
}

async function deleteProfile(profile: GatewayProfile) {
  if (!window.confirm(t(locale.value, 'gateway.deleteConfirm'))) return
  try {
    await invoke('delete_gateway_profile', { id: profile.id })
    profiles.value = profiles.value.filter(item => item.id !== profile.id)
    if (selectedId.value === profile.id) cancelEditing()
    status.value = await invoke<GatewayStatus>('get_gateway_status')
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
  const key = await invoke<string>('reveal_gateway_local_key', { profileId: profile.id, keyId: keyMeta.id })
  await navigator.clipboard.writeText(key)
  feedback.value = 'copied'
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
})
</script>

<template>
  <section class="space-y-3 pb-3">
    <nav class="gateway-tabs px-1 pt-1" :aria-label="t(locale, 'gateway.title')">
      <button
        type="button"
        class="gateway-tabs__item"
        :class="{ 'gateway-tabs__item--on': activePanel === 'takeover' }"
        :aria-selected="activePanel === 'takeover'"
        @click="activePanel = 'takeover'"
      >
        {{ t(locale, 'gateway.tabs.toolTakeover') }}
      </button>
      <button
        type="button"
        class="gateway-tabs__item"
        :class="{ 'gateway-tabs__item--on': activePanel === 'manual' }"
        :aria-selected="activePanel === 'manual'"
        @click="activePanel = 'manual'"
      >
        {{ t(locale, 'gateway.tabs.manualAccess') }}
      </button>
    </nav>

    <div v-if="feedback && !editing" :class="['rounded-xl border px-3 py-2 text-[10.5px] leading-snug', feedback === 'error' ? 'border-red-500/20 bg-red-500/8 text-red-600 dark:text-red-300' : 'border-emerald-500/20 bg-emerald-500/8 text-emerald-600 dark:text-emerald-300']">
      {{ feedbackMessage }}
    </div>

    <template v-if="activePanel === 'takeover'">
      <ProxyControlPanel />
      <CcSwitchCompatPanel />
    </template>

    <template v-else>
      <section class="theme-surface overflow-hidden rounded-2xl border">
        <div class="flex items-center justify-between gap-3 px-3 py-2.5">
          <div class="min-w-0">
            <h2 class="text-[11.5px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'gateway.localAccess') }}</h2>
            <p class="mt-0.5 truncate font-mono text-[9.5px] text-[var(--theme-text-tertiary)]">{{ listenerAddress }}</p>
            <p class="mt-0.5 text-[9px] leading-snug text-[var(--theme-text-quaternary)]">{{ t(locale, 'gateway.manualAccessDescription') }}</p>
          </div>
          <span :class="['shrink-0 rounded-full px-2 py-0.5 text-[9px] font-semibold', status?.routingActive ? 'bg-emerald-500/12 text-emerald-600 dark:text-emerald-300' : 'bg-gray-500/10 text-[var(--theme-text-tertiary)]']">{{ status?.routingActive ? t(locale, 'gateway.routingReady') : t(locale, 'gateway.routingPending') }}</span>
        </div>
      </section>

    <div class="flex items-center justify-between px-1 pt-1">
      <div>
        <h2 class="text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'gateway.profiles') }}</h2>
        <p class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.profileCount', { count: profiles.length }) }}</p>
      </div>
      <button class="inline-flex items-center gap-1.5 rounded-xl bg-[var(--theme-accent-primary)] px-2.5 py-1.5 text-[10.5px] font-semibold text-[var(--theme-accent-contrast)] shadow-sm" @click="startNewProfile">
        <Plus class="h-3.5 w-3.5" />
        {{ t(locale, 'gateway.newProfile') }}
      </button>
    </div>

    <div v-if="profiles.length === 0" class="theme-surface rounded-2xl border px-4 py-8 text-center">
      <RadioTower class="mx-auto h-7 w-7 text-[var(--theme-text-quaternary)]" />
      <p class="mt-2 text-[12px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.emptyTitle') }}</p>
      <p class="mx-auto mt-1 max-w-[260px] text-[10.5px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.emptyBody') }}</p>
      <button class="mt-3 rounded-xl border border-[var(--theme-border-default)] px-3 py-1.5 text-[10.5px] font-semibold text-[var(--theme-text-primary)]" @click="startNewProfile">
        {{ t(locale, 'gateway.newProfile') }}
      </button>
    </div>

    <div v-else class="space-y-2">
      <article v-for="profile in profiles" :key="profile.id" class="theme-surface overflow-visible rounded-xl border px-3 py-2.5 transition-colors hover:border-[var(--theme-border-strong)]">
        <div class="flex min-w-0 items-center gap-2">
          <button type="button" class="min-w-0 flex-1 text-left" :title="t(locale, 'gateway.editProfile')" @click="selectProfile(profile)">
            <div class="flex min-w-0 items-center gap-2">
              <span class="truncate text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ profile.name }}</span>
              <span class="max-w-[88px] shrink truncate rounded-md border border-[var(--theme-border-default)] bg-[var(--theme-bg-hover)] px-1.5 py-0.5 text-[9px] font-medium text-[var(--theme-text-tertiary)]">{{ compactProtocolLabel(profile.protocol) }}</span>
            </div>
          </button>
          <button type="button" class="gateway-profile-icon-action" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click="copyProfileBaseUrl(profile)"><Link2 class="h-3.5 w-3.5" /></button>
          <button type="button" class="gateway-profile-icon-action" :title="t(locale, 'gateway.copyLocalKey')" :aria-label="t(locale, 'gateway.copyLocalKey')" :disabled="profile.localKeys.length === 0" @click="copyProfileApiKey(profile)"><KeyRound class="h-3.5 w-3.5" /></button>
          <button type="button" class="gateway-profile-icon-action" :title="t(locale, 'gateway.editProfile')" :aria-label="t(locale, 'gateway.editProfile')" @click="selectProfile(profile)"><Pencil class="h-3.5 w-3.5" /></button>
          <button type="button" class="gateway-profile-icon-action gateway-profile-icon-action--danger" :title="t(locale, 'gateway.delete')" :aria-label="t(locale, 'gateway.delete')" @click="deleteProfile(profile)"><Trash2 class="h-3.5 w-3.5" /></button>
          <SettingsSwitch compact :checked="profile.enabled" :aria-label="t(locale, 'gateway.enabled')" @toggle="toggleProfile(profile)" />
        </div>
        <p class="mt-1.5 break-all border-t border-[var(--theme-border-default)] pt-1.5 font-mono text-[9.5px] leading-snug text-[var(--theme-text-tertiary)]">{{ profileAddress(profile) }}</p>
      </article>
    </div>
    </template>

    <Teleport to="body">
      <div v-if="editing" class="theme-backdrop fixed inset-0 z-[80] flex items-center justify-center p-4" @click.self="cancelEditing">
        <form class="max-h-[calc(100vh-2rem)] w-full max-w-[380px] overflow-y-auto rounded-2xl border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] shadow-2xl" role="dialog" aria-modal="true" @submit.prevent="saveProfile">
          <header class="flex items-start justify-between border-b border-[var(--theme-border-default)] px-4 py-3.5">
            <div class="min-w-0">
              <h2 class="text-[14px] font-bold text-[var(--theme-text-primary)]">{{ draft.id ? t(locale, 'gateway.editProfile') : t(locale, 'gateway.newProfile') }}</h2>
              <p class="mt-1 truncate text-[10px] text-[var(--theme-text-tertiary)]">{{ selectedProtocolLabel }}</p>
            </div>
            <button type="button" class="theme-icon-button -mr-1 -mt-1 rounded-lg p-2" :title="t(locale, 'gateway.cancel')" :aria-label="t(locale, 'gateway.cancel')" @click="cancelEditing"><X class="h-4 w-4" /></button>
          </header>

          <div class="space-y-2.5 p-4">
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
              <label class="flex h-11 items-center gap-3 border-t border-[var(--theme-border-default)] px-3">
                <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.upstreamKey') }}</span>
                <input v-model="upstreamSecret" type="password" class="min-w-0 flex-1 bg-transparent text-right font-mono text-[10.5px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]" :placeholder="draft.id ? t(locale, 'gateway.upstreamKeyKeepHint') : t(locale, 'gateway.upstreamKeyPlaceholder')" autocomplete="off" />
              </label>
              <label class="flex h-11 cursor-pointer items-center justify-between border-t border-[var(--theme-border-default)] px-3">
                <span class="text-[10.5px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.enabled') }}</span>
                <input v-model="draft.enabled" type="checkbox" class="peer sr-only" />
                <span class="relative h-5 w-9 rounded-full bg-[var(--theme-border-strong)] transition-colors peer-checked:bg-[var(--theme-accent-primary)] after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:shadow-sm after:transition-transform peer-checked:after:translate-x-4 peer-focus-visible:ring-2 peer-focus-visible:ring-[var(--theme-accent-primary)] peer-focus-visible:ring-offset-2"></span>
              </label>
            </section>

            <template v-if="!keyPanel">
              <p class="px-1 text-[9px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.managedKeyHint') }}</p>
              <div v-if="draft.id" class="theme-surface-muted rounded-xl border px-3 py-2.5">
                <div class="flex items-center justify-between gap-2"><span class="text-[9.5px] font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.address') }}</span><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click="copyAddress()"><Copy class="h-3.5 w-3.5" /></button></div>
                <p class="mt-1 break-all font-mono text-[9px] leading-snug text-[var(--theme-text-primary)]">{{ selectedAddress }}</p>
              </div>
              <p class="px-1 text-[9px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.singleUpstreamKeyHint') }}</p>
            </template>

            <template v-else>
              <button type="button" class="inline-flex items-center gap-1 text-[10px] font-semibold text-[var(--theme-text-secondary)]" @click="keyPanel = null"><span class="text-[15px] leading-none">‹</span>{{ draft.name }}</button>
              <section class="theme-surface-muted overflow-hidden rounded-xl border">
                <div class="border-b border-[var(--theme-border-default)] px-3 py-2.5"><h3 class="text-[11px] font-semibold text-[var(--theme-text-primary)]">{{ keyPanel === 'upstream' ? t(locale, 'gateway.upstreamKeys') : t(locale, 'gateway.localKeys') }}</h3><p class="mt-0.5 text-[9px] text-[var(--theme-text-tertiary)]">{{ keyPanel === 'upstream' ? t(locale, 'gateway.upstreamKeyHint') : t(locale, 'gateway.localKeyHint') }}</p></div>
                <div class="p-2">
                  <div v-if="keyPanel === 'upstream' && draft.upstreamKeys.length === 0" class="flex gap-1.5"><input v-model="upstreamRemark" class="min-w-0 w-20 flex-1 rounded-lg border border-[var(--theme-border-default)] bg-transparent px-2 py-2 text-[10px] text-[var(--theme-text-primary)] outline-none" :placeholder="t(locale, 'gateway.keyRemark')" /><input v-model="upstreamSecret" type="password" class="min-w-0 flex-[1.6] rounded-lg border border-[var(--theme-border-default)] bg-transparent px-2 py-2 font-mono text-[10px] text-[var(--theme-text-primary)] outline-none" :placeholder="t(locale, 'gateway.upstreamKeyPlaceholder')" autocomplete="off" /><button type="button" class="theme-icon-button rounded-lg p-1.5" :title="t(locale, 'gateway.addKey')" :aria-label="t(locale, 'gateway.addKey')" :disabled="saving || !upstreamSecret.trim()" @click="addUpstreamKey"><Plus class="h-3.5 w-3.5" /></button></div>
                  <div v-else-if="keyPanel === 'upstream'" class="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2 text-[10px] text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.singleUpstreamKeyHint') }}</div>
                  <div v-else class="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2 text-[10px] text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.singleLocalKeyHint') }}</div>
                  <div v-for="key in keyPanel === 'upstream' ? draft.upstreamKeys : draft.localKeys" :key="key.id" class="mt-1.5 rounded-lg border border-[var(--theme-border-default)] px-2 py-1.5 text-[10px]"><div class="flex h-6 items-center gap-2"><span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="key.enabled ? 'bg-emerald-500' : 'bg-gray-400'"></span><span class="min-w-0 flex-1 truncate text-[var(--theme-text-secondary)]">{{ key.remark || t(locale, 'gateway.unnamedKey') }}</span><template v-if="keyPanel === 'local'"><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.showLocalKey')" :aria-label="t(locale, 'gateway.showLocalKey')" @click="revealLocalKey(key.id)"><Eye class="h-3 w-3" /></button><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.copyLocalKey')" :aria-label="t(locale, 'gateway.copyLocalKey')" @click="copyStoredLocalKey(key.id)"><Copy class="h-3 w-3" /></button></template><button type="button" class="theme-icon-button rounded-lg p-1" :title="keyPanel === 'upstream' ? t(locale, 'gateway.deleteKey') : t(locale, 'gateway.revokeKey')" @click="keyPanel === 'upstream' ? deleteUpstreamKey(key.id) : revokeLocalKey(key.id)"><Trash2 class="h-3 w-3 text-red-500" /></button></div><div v-if="keyPanel === 'upstream'" class="mt-1 flex items-center gap-2 border-t border-[var(--theme-border-default)] pt-1"><label class="flex items-center gap-1 text-[9px] text-[var(--theme-text-tertiary)]"><input v-model="key.enabled" type="checkbox" class="h-3 w-3 accent-[var(--theme-accent-primary)]" :aria-label="t(locale, 'gateway.enabled')" @change="updateUpstreamKey(key as GatewayUpstreamKey)" />{{ t(locale, 'gateway.enabled') }}</label></div><p v-if="keyPanel === 'local' && revealedLocalKeys[key.id]" class="mt-1 break-all border-t border-[var(--theme-border-default)] pt-1 font-mono text-[9px] text-[var(--theme-text-primary)]">{{ revealedLocalKeys[key.id] }}</p></div>
                </div>
              </section>
            </template>

            <p v-if="feedback === 'error'" class="text-[10px] leading-snug text-red-500">{{ feedbackMessage }}</p>
            <p v-else-if="feedback === 'saved'" class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" />{{ t(locale, 'gateway.saveSuccess') }}</p>
            <p v-else-if="feedback === 'copied'" class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" />{{ t(locale, 'gateway.copied') }}</p>
          </div>

          <footer class="flex gap-2 border-t border-[var(--theme-border-default)] px-4 py-3">
            <button v-if="keyPanel" type="button" class="inline-flex flex-1 items-center justify-center rounded-lg border border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="keyPanel = null">{{ t(locale, 'gateway.cancel') }}</button>
            <template v-else><button type="button" class="rounded-lg border border-[var(--theme-border-default)] px-3 text-[10.5px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="cancelEditing">{{ t(locale, 'gateway.cancel') }}</button><button type="submit" class="inline-flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3 py-2 text-[10.5px] font-semibold text-[var(--theme-accent-contrast)] shadow-sm transition-opacity disabled:opacity-50" :disabled="saving"><Save class="h-3.5 w-3.5" />{{ draft.id ? t(locale, 'gateway.update') : t(locale, 'gateway.save') }}</button></template>
          </footer>
        </form>
      </div>
    </Teleport>
  </section>
</template>

<style scoped>
.gateway-tabs {
  display: flex;
  gap: 2px;
  padding: 2px;
  border: 1px solid var(--theme-border-subtle);
  border-radius: 10px;
  background: var(--theme-bg-surface);
}

.gateway-tabs__item {
  flex: 1;
  min-width: 0;
  padding: 6px 8px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: var(--theme-text-tertiary);
  cursor: pointer;
  font-size: 11px;
  font-weight: 600;
  transition: color 0.18s ease, background 0.18s ease, box-shadow 0.18s ease;
}

.gateway-tabs__item:hover:not(.gateway-tabs__item--on) {
  color: var(--theme-text-primary);
}

.gateway-tabs__item--on {
  background: var(--theme-accent-primary);
  box-shadow: 0 2px 6px color-mix(in srgb, var(--theme-accent-primary) 30%, transparent);
  color: var(--theme-accent-contrast);
}

.gateway-profile-icon-action {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 7px;
  color: var(--theme-text-secondary);
  font-size: 10px;
  font-weight: 600;
  transition: background-color 0.18s ease, color 0.18s ease;
}

.gateway-profile-icon-action {
  height: 26px;
  width: 26px;
  flex: 0 0 26px;
}

.gateway-profile-icon-action:hover {
  background: var(--theme-bg-hover);
  color: var(--theme-text-primary);
}

.gateway-profile-icon-action:disabled {
  cursor: not-allowed;
  opacity: 0.45;
}

.gateway-profile-icon-action--danger {
  color: rgb(239 68 68 / 0.85);
}

.gateway-profile-icon-action--danger:hover {
  background: rgb(239 68 68 / 0.08);
  color: rgb(220 38 38);
}
</style>
