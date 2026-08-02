<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { Check, Copy, Link2, Pencil, Plus, RadioTower, Save, Trash2, X } from 'lucide-vue-next'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import type { GatewayProfile, GatewayProtocol, GatewayStatus } from '../types'
import ProxyControlPanel from '../components/settings/ProxyControlPanel.vue'
import CcSwitchCompatPanel from '../components/settings/CcSwitchCompatPanel.vue'

const store = useMonitorStore()
const profiles = ref<GatewayProfile[]>([])
const status = ref<GatewayStatus | null>(null)
const selectedId = ref<string | null>(null)
const editing = ref(false)
const saving = ref(false)
const loading = ref(false)
const feedback = ref<'copied' | 'saved' | 'error' | null>(null)
const errorCode = ref('')

const protocolOptions: Array<{ value: GatewayProtocol; labelKey: string; basePath: string }> = [
  { value: 'open_ai_chat_completions', labelKey: 'gateway.protocolOpenAiChat', basePath: '/v1' },
  { value: 'open_ai_responses', labelKey: 'gateway.protocolOpenAiResponses', basePath: '/v1' },
  { value: 'anthropic_messages', labelKey: 'gateway.protocolAnthropic', basePath: '/v1' },
  { value: 'gemini_generate_content', labelKey: 'gateway.protocolGemini', basePath: '/v1beta' }
]

const draft = ref({
  id: undefined as string | undefined,
  name: '',
  protocol: 'open_ai_chat_completions' as GatewayProtocol,
  baseUrl: '',
  enabled: true,
  clientLabel: ''
})

const locale = computed(() => store.settings.locale)
const selectedProfile = computed(() => profiles.value.find(profile => profile.id === selectedId.value) ?? null)
const listenerAddress = computed(() => status.value?.listenerAddress || `http://127.0.0.1:${store.settings.proxy.port}`)
const selectedBasePath = computed(() => protocolOptions.find(option => option.value === (selectedProfile.value?.protocol ?? draft.value.protocol))?.basePath ?? '/v1')
const selectedAddress = computed(() => selectedProfile.value ? `${listenerAddress.value}/gateway/${selectedProfile.value.id}${selectedBasePath.value}` : '')
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
    : errorCode.value.startsWith('ERR_GATEWAY_PROFILE_NAME')
      ? 'gateway.validationName'
      : errorCode.value.startsWith('ERR_GATEWAY_BASE_URL')
        ? 'gateway.validationUrl'
        : 'gateway.operationError'
  return t(locale.value, errorKey)
})

function resetDraft() {
  draft.value = { id: undefined, name: '', protocol: 'open_ai_chat_completions', baseUrl: '', enabled: true, clientLabel: '' }
}

function selectProfile(profile: GatewayProfile) {
  selectedId.value = profile.id
  editing.value = true
  draft.value = { ...profile }
  feedback.value = null
  errorCode.value = ''
}

function startNewProfile() {
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
    clientLabel: draft.value.clientLabel.trim()
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

function protocolLabel(protocol: GatewayProtocol) {
  const option = protocolOptions.find(item => item.value === protocol)
  return option ? t(locale.value, option.labelKey) : protocol
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && editing.value && !saving.value) cancelEditing()
}

onMounted(() => {
  load()
  window.addEventListener('keydown', handleKeydown)
})

onUnmounted(() => window.removeEventListener('keydown', handleKeydown))
</script>

<template>
  <section class="space-y-3 pb-3">
    <header class="flex items-start gap-3 px-1 pt-1">
      <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-2xl bg-emerald-500/12 text-emerald-600 dark:text-emerald-300">
        <RadioTower class="h-4.5 w-4.5" />
      </div>
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <h1 class="truncate text-[15px] font-bold text-[var(--theme-text-primary)]">{{ t(locale, 'gateway.title') }}</h1>
          <span :class="['rounded-full px-2 py-0.5 text-[10px] font-semibold', status?.proxyRunning ? 'bg-emerald-500/12 text-emerald-600 dark:text-emerald-300' : 'bg-gray-500/10 text-[var(--theme-text-tertiary)]']">
            {{ status?.proxyRunning ? t(locale, 'gateway.running') : t(locale, 'gateway.stopped') }}
          </span>
        </div>
        <p class="mt-0.5 text-[10.5px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.subtitle') }}</p>
      </div>
      <button
        class="theme-icon-button rounded-xl p-2"
        :title="t(locale, 'common.refresh')"
        :aria-label="t(locale, 'common.refresh')"
        @click="load"
      >
        <Link2 class="h-3.5 w-3.5" :class="{ 'animate-pulse': loading }" />
      </button>
    </header>

    <div v-if="feedback && !editing" :class="['rounded-xl border px-3 py-2 text-[10.5px] leading-snug', feedback === 'error' ? 'border-red-500/20 bg-red-500/8 text-red-600 dark:text-red-300' : 'border-emerald-500/20 bg-emerald-500/8 text-emerald-600 dark:text-emerald-300']">
      {{ feedbackMessage }}
    </div>

    <section class="theme-surface overflow-hidden rounded-2xl border">
      <div class="flex items-center justify-between border-b border-[var(--theme-border-default)] px-3 py-2.5">
        <div>
          <h2 class="text-[11.5px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'gateway.localAccess') }}</h2>
          <p class="mt-0.5 text-[9.5px] text-[var(--theme-text-tertiary)]">{{ listenerAddress }}</p>
          <p class="mt-0.5 text-[9px] leading-snug text-[var(--theme-text-quaternary)]">{{ t(locale, 'gateway.autoStartHint') }}</p>
        </div>
        <span :class="['rounded-full px-2 py-0.5 text-[9px] font-semibold', status?.routingActive ? 'bg-emerald-500/12 text-emerald-600 dark:text-emerald-300' : 'bg-gray-500/10 text-[var(--theme-text-tertiary)]']">{{ status?.routingActive ? t(locale, 'gateway.routingReady') : t(locale, 'gateway.routingPending') }}</span>
      </div>
      <div class="space-y-2 p-2">
        <ProxyControlPanel />
        <CcSwitchCompatPanel />
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

    <div v-else class="space-y-1.5">
      <div v-for="profile in profiles" :key="profile.id" :class="['theme-surface rounded-2xl border p-2.5 transition-colors', selectedId === profile.id ? 'border-emerald-500/35 bg-emerald-500/5' : '']">
        <button class="flex w-full min-w-0 items-start gap-2 text-left" @click="selectProfile(profile)">
          <span :class="['mt-1 h-2 w-2 shrink-0 rounded-full', profile.enabled ? 'bg-emerald-500' : 'bg-gray-400']"></span>
          <span class="min-w-0 flex-1">
            <span class="block truncate text-[11.5px] font-semibold text-[var(--theme-text-primary)]">{{ profile.name }}</span>
            <span class="mt-0.5 block truncate text-[9.5px] text-[var(--theme-text-tertiary)]">{{ protocolLabel(profile.protocol) }}</span>
          </span>
          <span class="shrink-0 text-[9px] text-[var(--theme-text-quaternary)]">{{ profile.enabled ? t(locale, 'common.enabled') : t(locale, 'common.disabled') }}</span>
        </button>
        <div class="mt-2 flex items-center justify-end gap-1">
          <button class="rounded-lg p-1.5 text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click="copyAddress(profile)"><Copy class="h-3.5 w-3.5" /></button>
          <button class="rounded-lg p-1.5 text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]" :title="profile.enabled ? t(locale, 'common.hide') : t(locale, 'common.enabled')" @click="toggleProfile(profile)"><Check v-if="profile.enabled" class="h-3.5 w-3.5 text-emerald-500" /><span v-else class="block h-3.5 w-3.5 rounded-full border border-current" /></button>
          <button class="rounded-lg p-1.5 text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]" :title="t(locale, 'gateway.editProfile')" :aria-label="t(locale, 'gateway.editProfile')" @click="selectProfile(profile)"><Pencil class="h-3.5 w-3.5" /></button>
          <button class="rounded-lg p-1.5 text-red-500/75 hover:bg-red-500/10" :title="t(locale, 'gateway.delete')" :aria-label="t(locale, 'gateway.delete')" @click="deleteProfile(profile)"><Trash2 class="h-3.5 w-3.5" /></button>
        </div>
      </div>
    </div>

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
            <section class="theme-surface-muted overflow-hidden rounded-xl border">
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
                <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.clientLabel') }}</span>
                <input v-model="draft.clientLabel" class="min-w-0 flex-1 bg-transparent text-right text-[11.5px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]" :placeholder="t(locale, 'gateway.clientLabelPlaceholder')" />
              </label>
              <label class="flex h-11 cursor-pointer items-center justify-between border-t border-[var(--theme-border-default)] px-3">
                <span class="text-[10.5px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.enabled') }}</span>
                <input v-model="draft.enabled" type="checkbox" class="peer sr-only" />
                <span class="relative h-5 w-9 rounded-full bg-[var(--theme-border-strong)] transition-colors peer-checked:bg-[var(--theme-accent-primary)] after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:shadow-sm after:transition-transform peer-checked:after:translate-x-4 peer-focus-visible:ring-2 peer-focus-visible:ring-[var(--theme-accent-primary)] peer-focus-visible:ring-offset-2"></span>
              </label>
            </section>

            <p class="px-1 text-[9px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.clientLabelHint') }}</p>

            <div v-if="draft.id" class="theme-surface-muted rounded-xl border p-3">
              <div class="flex items-center justify-between gap-2">
                <span class="text-[9.5px] font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.address') }}</span>
                <button type="button" class="theme-icon-button rounded-lg p-1.5" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click="copyAddress()"><Copy class="h-3.5 w-3.5" /></button>
              </div>
              <p class="mt-1.5 break-all font-mono text-[9.5px] leading-snug text-[var(--theme-text-primary)]">{{ selectedAddress }}</p>
              <p class="mt-1.5 text-[9px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.addressHint') }}</p>
            </div>

            <p v-if="feedback === 'error'" class="text-[10px] leading-snug text-red-500">{{ feedbackMessage }}</p>
            <p v-else-if="feedback === 'saved'" class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" />{{ t(locale, 'gateway.saveSuccess') }}</p>
            <p v-else-if="feedback === 'copied'" class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" />{{ t(locale, 'gateway.copied') }}</p>
          </div>

          <footer class="flex gap-2 border-t border-[var(--theme-border-default)] px-4 py-3">
            <button type="button" class="rounded-lg border border-[var(--theme-border-default)] px-3 text-[10.5px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="cancelEditing">{{ t(locale, 'gateway.cancel') }}</button>
            <button type="submit" class="inline-flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3 py-2 text-[10.5px] font-semibold text-[var(--theme-accent-contrast)] shadow-sm transition-opacity disabled:opacity-50" :disabled="saving"><Save class="h-3.5 w-3.5" />{{ draft.id ? t(locale, 'gateway.update') : t(locale, 'gateway.save') }}</button>
          </footer>
        </form>
      </div>
    </Teleport>
  </section>
</template>
