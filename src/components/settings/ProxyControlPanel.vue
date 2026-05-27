<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import { type ToolTakeoverStatus } from '../../types'
import LobeIcon from '../LobeIcon.vue'
import SettingsSwitch from './SettingsSwitch.vue'
import ConfirmDialog from './ConfirmDialog.vue'
import { TOOL_LOBE_ICONS } from '../../iconConfig'

const store = useMonitorStore()

const localIncludeErrorRequests = ref(store.settings.proxy.includeErrorRequests ?? true)
const takeoverStatuses = ref<ToolTakeoverStatus[]>([])
const takeoverLoading = ref<Record<string, boolean>>({})
const showOfficialApiWarning = ref(false)
const pendingTakeoverTool = ref<string | null>(null)

watch(() => store.settings.proxy.includeErrorRequests, (value) => {
  localIncludeErrorRequests.value = value ?? true
})

const proxyEnabled = computed(() => store.isProxyRunning)

const proxyStatusInfo = computed(() => {
  if (!store.proxyStatus) {
    return null
  }

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

const managedToolProfiles = computed(() => {
  return store.settings.clientTools.profiles.filter(profile => ['claude_code', 'codex'].includes(profile.tool))
})

const handleIncludeErrorRequestsChange = async () => {
  store.settings.proxy.includeErrorRequests = localIncludeErrorRequests.value
  await store.saveSettings()
}

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
  if (status) {
    return status
  }
  await loadTakeoverStatuses()
  status = takeoverStatusFor(tool)
  return status
}

const getToolIcon = (tool: string, icon?: string) => {
  return icon || TOOL_LOBE_ICONS[tool] || null
}

const openOfficialApiWarning = (tool: string) => {
  pendingTakeoverTool.value = tool
  showOfficialApiWarning.value = true
}

const closeOfficialApiWarning = () => {
  showOfficialApiWarning.value = false
  pendingTakeoverTool.value = null
}

const applyToolTakeover = async (tool: string, nextEnabled: boolean) => {
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

const toggleToolTakeover = async (tool: string) => {
  const nextEnabled = !takeoverEnabledFor(tool)
  const status = nextEnabled ? await ensureTakeoverStatusFor(tool) : takeoverStatusFor(tool)
  if (nextEnabled && (!status || status.officialProvider)) {
    openOfficialApiWarning(tool)
    return
  }
  await applyToolTakeover(tool, nextEnabled)
}

const confirmOfficialApiWarning = async () => {
  const tool = pendingTakeoverTool.value
  closeOfficialApiWarning()
  if (!tool) {
    return
  }
  await applyToolTakeover(tool, true)
}

const handleGlobalKeydown = (event: KeyboardEvent) => {
  if (event.key === 'Escape' && showOfficialApiWarning.value) {
    closeOfficialApiWarning()
  }
}

onMounted(async () => {
  await loadTakeoverStatuses()
  window.addEventListener('keydown', handleGlobalKeydown)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleGlobalKeydown)
})

function formatUptime(seconds: number): string {
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`
}
</script>

<template>
  <div class="space-y-2">
    <div class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm divide-y divide-gray-50 dark:border-neutral-800 dark:bg-[#1C1C1E] dark:divide-neutral-800/50">
      <div class="flex items-center justify-between p-3 px-4 text-[13px]">
        <div class="flex flex-col">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.interceptRequests') }}</span>
          <span v-if="proxyEnabled && proxyStatusInfo" class="mt-0.5 text-[10px] text-gray-400">
            {{ t(store.settings.locale, 'settings.proxyRunning') }} · {{ t(store.settings.locale, 'settings.port') }} {{ proxyStatusInfo.port }} · {{ proxyStatusInfo.uptime }}
          </span>
        </div>
        <SettingsSwitch :checked="proxyEnabled" @toggle="toggleProxy" />
      </div>

      <div v-if="proxyEnabled && proxyStatusInfo" class="bg-gray-50 p-3 px-4 dark:bg-neutral-800/50">
        <div class="grid grid-cols-2 gap-2 text-[11px]">
          <div class="flex items-center gap-1.5">
            <span :class="['h-2 w-2 rounded-full', proxyStatusInfo.configTakenOver ? 'bg-green-500' : 'bg-amber-500']"></span>
            <span class="text-gray-500 dark:text-gray-400">
              {{ proxyStatusInfo.configTakenOver ? t(store.settings.locale, 'settings.configTakenOver') : t(store.settings.locale, 'settings.configNotTakenOver') }}
            </span>
          </div>
          <div class="flex items-center gap-1.5">
            <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.requestCount') }}:</span>
            <span class="font-mono text-gray-700 dark:text-gray-300">{{ proxyStatusInfo.totalRequests }}</span>
          </div>
          <div class="flex items-center gap-1.5">
            <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.recordCount') }}:</span>
            <span class="font-mono text-gray-700 dark:text-gray-300">{{ proxyStatusInfo.recordCount }}</span>
          </div>
          <div class="flex items-center gap-1.5">
            <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.activeConnections') }}:</span>
            <span class="font-mono text-gray-700 dark:text-gray-300">{{ proxyStatusInfo.activeConnections }}</span>
          </div>
        </div>
      </div>

      <div class="p-3 px-4">
        <div class="mb-2 flex items-center justify-between">
          <div>
            <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.proxyTools') }}</div>
            <div class="mt-0.5 text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.proxyToolsDesc') }}</div>
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
              takeoverLoading[profile.tool] ? 'pointer-events-none opacity-60' : 'hover:border-blue-200 dark:hover:border-blue-500/40'
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

      <div class="flex items-center justify-between p-3 px-4 text-[13px]">
        <div class="flex flex-col">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.includeErrorRequests') }}</span>
          <span class="mt-0.5 text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.includeErrorRequestsDesc') }}</span>
        </div>
        <SettingsSwitch :checked="localIncludeErrorRequests" @toggle="localIncludeErrorRequests = !localIncludeErrorRequests; handleIncludeErrorRequestsChange()" />
      </div>
    </div>

    <ConfirmDialog
      :open="showOfficialApiWarning"
      :title="t(store.settings.locale, 'settings.officialApiRiskTitle')"
      :body="`${t(store.settings.locale, 'settings.officialApiRiskBody')} ${t(store.settings.locale, 'settings.officialApiRiskAccount')} ${t(store.settings.locale, 'settings.officialApiRiskAccept')}`"
      :confirm-label="t(store.settings.locale, 'settings.officialApiRiskContinue')"
      :cancel-label="t(store.settings.locale, 'common.cancel')"
      tone="warning"
      @cancel="closeOfficialApiWarning"
      @confirm="confirmOfficialApiWarning"
    />
  </div>
</template>
