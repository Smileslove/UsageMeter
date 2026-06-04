<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import SettingsSwitch from './SettingsSwitch.vue'

type NetworkProxyScheme = 'http' | 'https' | 'socks5'

interface NetworkProxyTestResult {
  ok: boolean
  reachable: boolean
  latencyMs?: number
  status?: number
  errorKind?: string
  errorDetail?: string
}

type TestState = {
  status: 'idle' | 'testing' | 'success' | 'error'
  latency?: number
  statusCode?: number
  errorKey?: string
  errorDetail?: string
}

const store = useMonitorStore()

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
const npTests = ref<Record<string, TestState>>({
  github: { status: 'idle' },
  anthropic: { status: 'idle' },
  openai: { status: 'idle' },
})

const proxyTargetIcons: Record<string, string> = {
  github: 'M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12',
  anthropic: 'M12.4553 3H11.0947L4.5 21h2.8026l1.4702-4.1082h6.4538L16.6974 21H19.5L12.4553 3ZM9.4614 14.4695 11.775 7.9185l2.3136 6.551H9.4614Z',
  openai: 'M22.282 9.821a5.985 5.985 0 0 0-.516-4.911 6.046 6.046 0 0 0-6.51-2.9 6.065 6.065 0 0 0-4.981-2.529 6.046 6.046 0 0 0-5.777 4.196 6.065 6.065 0 0 0-3.998 2.9 6.046 6.046 0 0 0 .743 7.097 5.98 5.98 0 0 0 .511 4.911 6.051 6.051 0 0 0 6.515 2.9A5.985 5.985 0 0 0 13.26 24a6.056 6.056 0 0 0 5.772-4.206 5.99 5.99 0 0 0 3.997-2.9 6.056 6.056 0 0 0-.747-7.073zM13.26 22.43a4.476 4.476 0 0 1-2.876-1.041l.141-.081 4.779-2.758a.775.775 0 0 0 .392-.681v-6.737l2.02 1.169a.071.071 0 0 1 .038.052v5.583a4.504 4.504 0 0 1-4.494 4.494zM3.6 18.304a4.47 4.47 0 0 1-.535-3.014l.142.085 4.783 2.759a.771.771 0 0 0 .78 0l5.843-3.369v2.332a.08.08 0 0 1-.033.062L9.74 19.95a4.5 4.5 0 0 1-6.14-1.646zM2.34 7.896a4.485 4.485 0 0 1 2.366-1.973V11.6a.766.766 0 0 0 .388.676l5.815 3.355-2.02 1.168a.076.076 0 0 1-.071 0l-4.83-2.786A4.504 4.504 0 0 1 2.34 7.872zm16.597 3.855-5.803-3.358L15.154 7.2a.076.076 0 0 1 .071 0l4.83 2.791a4.494 4.494 0 0 1-.676 8.104v-5.678a.79.79 0 0 0-.407-.666zm2.01-3.023-.141-.085-4.774-2.782a.776.776 0 0 0-.785 0L9.409 9.23V6.897a.066.066 0 0 1 .028-.061l4.83-2.787a4.5 4.5 0 0 1 6.68 4.66zm-12.64 4.135-2.02-1.167a.08.08 0 0 1-.038-.057V6.075a4.5 4.5 0 0 1 7.375-3.453l-.142.08-4.778 2.758a.775.775 0 0 0-.392.681zm1.097-2.365 2.602-1.5 2.607 1.5v2.999l-2.597 1.5-2.607-1.5z',
}

watch(() => store.settings.networkProxy, (value) => {
  const newEnabled = value?.enabled ?? false
  const newUrl = value?.host ? buildProxyUrl(value.scheme ?? 'http', value.host, value.port ?? 7890) : ''
  const enabledChanged = newEnabled !== npEnabled.value
  const urlChanged = newUrl !== npUrl.value
  if (enabledChanged) npEnabled.value = newEnabled
  if (urlChanged) npUrl.value = newUrl
  if (enabledChanged || urlChanged) {
    resetTestStates()
  }
}, { deep: true })

const networkProxyChipText = computed(() => {
  if (!npEnabled.value) {
    return t(store.settings.locale, 'settings.networkProxyChipFollowSystem')
  }
  return npUrl.value || t(store.settings.locale, 'settings.networkProxyChipFollowSystem')
})

const networkProxyDirty = computed(() => {
  const current = store.settings.networkProxy
  const currentEnabled = current?.enabled ?? false
  const currentUrl = current?.host ? buildProxyUrl(current.scheme ?? 'http', current.host, current.port ?? 7890) : ''
  return currentEnabled !== npEnabled.value || currentUrl !== npUrl.value
})

function buildProxyUrl(scheme: string, host: string, port: number): string {
  if (!host) return ''
  return `${scheme}://${host}:${port}`
}

function parseProxyUrl(url: string): { scheme: NetworkProxyScheme; host: string; port: number } | null {
  try {
    const parsed = new URL(url.trim())
    const scheme = parsed.protocol.replace(':', '') as NetworkProxyScheme
    if (!['http', 'https', 'socks5'].includes(scheme)) return null
    const port = parsed.port ? parseInt(parsed.port) : (scheme === 'https' ? 443 : 1080)
    if (!parsed.hostname || Number.isNaN(port) || port < 1 || port > 65535) return null
    return { scheme, host: parsed.hostname, port }
  } catch {
    return null
  }
}

function validateNetworkProxy(): string {
  if (!npEnabled.value) return ''
  if (!npUrl.value.trim()) return t(store.settings.locale, 'settings.networkProxyUrlRequired')
  if (!parseProxyUrl(npUrl.value)) return t(store.settings.locale, 'settings.networkProxyUrlInvalid')
  return ''
}

function currentProxyPayload() {
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

function resetTestStates() {
  npTests.value = {
    github: { status: 'idle' },
    anthropic: { status: 'idle' },
    openai: { status: 'idle' },
  }
}

const toggleNetworkProxy = async () => {
  npEnabled.value = !npEnabled.value
  npError.value = ''
  resetTestStates()

  if (!npEnabled.value) {
    const previous = store.settings.networkProxy
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
    } catch {
      store.settings.networkProxy = previous
      npEnabled.value = true
    }
  }
}

const saveNetworkProxy = async () => {
  const error = validateNetworkProxy()
  if (error) {
    npError.value = error
    return
  }
  npError.value = ''
  const previous = store.settings.networkProxy
  store.settings.networkProxy = currentProxyPayload()
  try {
    await store.saveSettings()
    npSavedFlash.value = true
    setTimeout(() => {
      npSavedFlash.value = false
    }, 1500)
    if (npEnabled.value) {
      testAllTargets()
    }
  } catch (error) {
    store.settings.networkProxy = previous
    npError.value = String(error)
  }
}

async function testTarget(target: string) {
  npTests.value[target] = { status: 'testing' }
  try {
    const result = await invoke<NetworkProxyTestResult>('test_network_proxy', {
      config: currentProxyPayload(),
      target,
    })
    if (result.reachable) {
      npTests.value[target] = {
        status: 'success',
        latency: result.latencyMs,
        statusCode: result.status,
        errorKey: result.errorKind,
        errorDetail: result.errorDetail,
      }
      return
    }

    npTests.value[target] = {
      status: 'error',
      statusCode: result.status,
      errorKey: result.errorKind ?? 'testUnknownError',
      errorDetail: result.errorDetail,
      latency: result.latencyMs,
    }
  } catch (error) {
    npTests.value[target] = { status: 'error', errorKey: 'testUnknownError', errorDetail: String(error) }
  }
}

function testAllTargets() {
  const error = validateNetworkProxy()
  if (error) {
    npError.value = error
    return
  }
  npError.value = ''
  testTarget('github')
  testTarget('anthropic')
  testTarget('openai')
}

function networkProxyTestLabel(state: TestState) {
  if (state.status === 'idle') return '—'
  if (state.status === 'testing') return '…'
  if (state.status === 'success' && state.latency != null) {
    return `${state.latency}ms`
  }
  return t(store.settings.locale, `settings.networkProxyErr_${state.errorKey ?? 'testUnknownError'}`)
}
</script>

<template>
  <div class="py-2 px-4">
    <div class="flex items-center justify-between gap-3">
      <div class="min-w-0 flex-1">
        <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.networkProxyTitle') }}</div>
        <div class="mt-0.5 truncate text-[10px] text-gray-400">{{ networkProxyChipText }}</div>
      </div>
      <SettingsSwitch :checked="npEnabled" @toggle="toggleNetworkProxy" />
    </div>

    <div v-if="npEnabled" class="mt-3 space-y-3">
      <div class="flex items-center gap-2">
        <input
          v-model="npUrl"
          type="text"
          :placeholder="t(store.settings.locale, 'settings.networkProxyUrlPlaceholder')"
          class="h-8 min-w-0 flex-1 rounded-lg border border-gray-200 bg-white px-3 text-[12px] text-gray-700 placeholder:text-gray-300 focus:outline-none focus:ring-1 focus:ring-[var(--theme-ring-focus)] dark:border-neutral-700 dark:bg-neutral-800 dark:text-gray-200 dark:placeholder:text-neutral-600"
          @input="npError = ''"
        />
        <button
          type="button"
          class="h-8 shrink-0 rounded-lg bg-[var(--theme-accent-primary)] px-3 text-[12px] text-[var(--theme-accent-contrast)] transition-opacity hover:opacity-90 disabled:opacity-40 disabled:hover:opacity-40"
          :disabled="!networkProxyDirty"
          @click="saveNetworkProxy"
        >
          <span v-if="npSavedFlash">✓</span>
          <span v-else>{{ t(store.settings.locale, 'settings.networkProxySave') }}</span>
        </button>
      </div>

      <div v-if="npError" class="text-[11px] text-red-500">{{ npError }}</div>

      <div class="overflow-hidden rounded-xl border border-gray-100 dark:border-neutral-700/60">
        <button
          v-for="(target, index) in ['github', 'anthropic', 'openai']"
          :key="target"
          type="button"
          class="flex w-full select-none items-center justify-between px-3 py-2 text-[12px] transition-colors"
          :class="index < 2 ? 'border-b border-gray-100 dark:border-neutral-700/60' : ''"
          @click="testTarget(target)"
        >
          <span class="flex items-center gap-2">
            <svg
              viewBox="0 0 24 24"
              class="h-3.5 w-3.5 shrink-0 transition-colors"
              :class="[
                npTests[target].status === 'testing' ? 'animate-pulse text-amber-400' :
                npTests[target].status === 'success' ? 'text-emerald-500' :
                npTests[target].status === 'error' ? 'text-red-400' :
                'text-gray-400 dark:text-neutral-500'
              ]"
            >
              <path fill="currentColor" :d="proxyTargetIcons[target]" />
            </svg>
            <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, `settings.networkProxyTarget_${target}`) }}</span>
          </span>
          <span
            class="tabular-nums text-[11px]"
            :class="[
              npTests[target].status === 'success' ? 'text-emerald-500' :
              npTests[target].status === 'error' ? 'text-red-400' :
              'text-gray-300 dark:text-neutral-600'
            ]"
          >
            {{ networkProxyTestLabel(npTests[target]) }}
          </span>
        </button>
      </div>
    </div>
  </div>
</template>
