<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import { type ClientToolProfile, type ToolTakeoverStatus } from '../../types'
import LobeIcon from '../LobeIcon.vue'
import SettingsSwitch from './SettingsSwitch.vue'
import ConfirmDialog from './ConfirmDialog.vue'
import { TOOL_LOBE_ICONS } from '../../iconConfig'

const store = useMonitorStore()

const localIncludeErrorRequests = ref(store.settings.proxy.includeErrorRequests ?? true)
const takeoverStatuses = ref<ToolTakeoverStatus[]>([])
const takeoverLoading = ref<Record<string, boolean>>({})
const takeoverActionErrors = ref<Record<string, string>>({})
const showOfficialApiWarning = ref(false)
const pendingTakeoverTool = ref<string | null>(null)
const expandedTool = ref<string | null>(null)
let unlistenTakeoverConflict: UnlistenFn | null = null

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
  return store.settings.clientTools.profiles.filter(profile => ['claude_code', 'codex', 'opencode', 'reasonix'].includes(profile.tool))
})

const toolAlerts = computed(() => {
  return managedToolProfiles.value.flatMap((profile) => {
    const status = takeoverStatusFor(profile.tool)
    const alerts: Array<{ toolId: string; tool: string; tone: 'warning' | 'error'; message: string; actions?: Array<'force_reclaim' | 'disable_takeover'> }> = []

    if (status?.conflictPaused) {
      alerts.push({
        toolId: profile.tool,
        tool: profile.displayName || profile.tool,
        tone: 'warning',
        message: t(store.settings.locale, 'settings.takeoverConflictDetectedDesc'),
        actions: ['force_reclaim', 'disable_takeover']
      })
    }

    if (status?.lastError) {
      alerts.push({
        toolId: profile.tool,
        tool: profile.displayName || profile.tool,
        tone: 'error',
        message: status.lastError
      })
    }

    const actionError = takeoverActionErrors.value[profile.tool]
    if (actionError) {
      alerts.push({
        toolId: profile.tool,
        tool: profile.displayName || profile.tool,
        tone: 'error',
        message: actionError
      })
    }

    return alerts
  })
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
    takeoverActionErrors.value = { ...takeoverActionErrors.value, [tool]: '' }
    await store.loadSettings()
    await store.getProxyStatus()
    await loadTakeoverStatuses()
  } catch (error) {
    const message = error instanceof Error
      ? error.message
      : typeof error === 'string'
        ? error
        : JSON.stringify(error)
    takeoverActionErrors.value = { ...takeoverActionErrors.value, [tool]: message }
    await loadTakeoverStatuses()
  } finally {
    takeoverLoading.value = { ...takeoverLoading.value, [tool]: false }
  }
}

const resolveTakeoverConflict = async (tool: string, action: 'force_reclaim' | 'pause' | 'disable_takeover') => {
  takeoverLoading.value = { ...takeoverLoading.value, [tool]: true }
  try {
    await invoke('resolve_takeover_conflict', { tool, action })
    takeoverActionErrors.value = { ...takeoverActionErrors.value, [tool]: '' }
    await store.loadSettings()
    await store.getProxyStatus()
    await loadTakeoverStatuses()
  } catch (error) {
    const message = error instanceof Error
      ? error.message
      : typeof error === 'string'
        ? error
        : JSON.stringify(error)
    takeoverActionErrors.value = { ...takeoverActionErrors.value, [tool]: message }
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
  unlistenTakeoverConflict = await listen('takeover_conflict_detected', loadTakeoverStatuses)
  window.addEventListener('keydown', handleGlobalKeydown)
})

onUnmounted(() => {
  if (unlistenTakeoverConflict) unlistenTakeoverConflict()
  window.removeEventListener('keydown', handleGlobalKeydown)
})

function formatUptime(seconds: number): string {
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`
}

function toolLabel(profile: ClientToolProfile): string {
  return profile.displayName || profile.tool
}

function compactPath(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, '~')
}

function toolConfigPaths(profile: ClientToolProfile): string[] {
  const status = takeoverStatusFor(profile.tool)
  const rawPaths = [status?.configPath, status?.authPath].filter((value): value is string => Boolean(value))

  if (rawPaths.length > 0) {
    return rawPaths.map(compactPath)
  }

  switch (profile.tool) {
    case 'claude_code':
      return ['~/.claude/settings.json']
    case 'codex':
      return ['~/.codex/config.toml', '~/.codex/auth.json']
    case 'opencode':
      return ['~/.config/opencode/opencode.json']
    case 'reasonix':
      return ['~/Library/Application Support/reasonix/config.toml']
    default:
      return []
  }
}

function toolConfigSummary(profile: ClientToolProfile): string {
  const paths = toolConfigPaths(profile)
  if (paths.length === 0) {
    return t(store.settings.locale, 'settings.proxyToolConfigUnknown')
  }

  const fileNames = paths.map((path) => path.split('/').filter(Boolean).pop() || path)
  return t(store.settings.locale, 'settings.proxyToolConfigSummary', {
    files: fileNames.join(' · ')
  })
}

function inlineScopeWarning(profile: ClientToolProfile): string | null {
  const status = takeoverStatusFor(profile.tool)
  if (!status?.enabled || !status.scopeWarningKey) {
    return null
  }
  return t(store.settings.locale, status.scopeWarningKey)
}

function showScopeBadge(profile: ClientToolProfile): boolean {
  const status = takeoverStatusFor(profile.tool)
  return Boolean(status?.enabled && status?.scopeWarningKey)
}

function toggleExpandedTool(tool: string): void {
  expandedTool.value = expandedTool.value === tool ? null : tool
}
</script>

<template>
  <div class="space-y-2">
    <div class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm divide-y divide-gray-50 dark:border-neutral-800 dark:bg-[#1C1C1E] dark:divide-neutral-800/50">
      <div class="flex items-center justify-between py-2 px-4 text-[13px]">
        <div class="flex flex-col">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.interceptRequests') }}</span>
          <span v-if="proxyEnabled && proxyStatusInfo" class="text-[10px] text-gray-400">
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

      <div class="p-2 px-4">
        <div class="mb-2 flex items-center justify-between">
          <div>
            <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.proxyTools') }}</div>
            <div class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.proxyToolsDescCompact') }}</div>
          </div>
          <span class="text-[10px] text-gray-400">
            {{ managedToolProfiles.filter(profile => takeoverEnabledFor(profile.tool)).length }}/{{ managedToolProfiles.length }}
          </span>
        </div>

        <div class="space-y-1.5">
          <div
            v-for="profile in managedToolProfiles"
            :key="profile.id"
            :class="[
              'rounded-lg border border-gray-100 bg-white px-2.5 py-1.5 dark:border-neutral-800 dark:bg-neutral-950',
              takeoverLoading[profile.tool] ? 'opacity-60' : ''
            ]"
          >
            <div class="flex items-start justify-between gap-2">
              <div class="flex min-w-0 items-start gap-2">
                <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-gray-50 dark:bg-neutral-800">
                  <LobeIcon
                    v-if="getToolIcon(profile.tool, profile.icon)"
                    :slug="getToolIcon(profile.tool, profile.icon)!"
                    :size="15"
                    @error="() => {}"
                  />
                  <span v-else class="h-1.5 w-1.5 rounded-full bg-gray-400"></span>
                </div>
                <div class="min-w-0">
                  <div class="flex items-center gap-1.5">
                    <div class="text-[10.5px] font-medium leading-none text-gray-700 dark:text-gray-200">{{ toolLabel(profile) }}</div>
                    <span
                      v-if="showScopeBadge(profile)"
                      class="rounded px-1 py-px text-[9px] font-medium bg-amber-100 text-amber-700 dark:bg-amber-900/50 dark:text-amber-300"
                    >{{ t(store.settings.locale, 'settings.globalConfigOnlyBadge') }}</span>
                  </div>
                  <div class="mt-0.5 flex items-center gap-1.5">
                    <div class="min-w-0 truncate font-mono text-[9px] leading-tight text-gray-400 dark:text-gray-500">
                      {{ toolConfigSummary(profile) }}
                    </div>
                    <button
                      type="button"
                      class="shrink-0 text-[9px] font-medium text-[var(--theme-accent-primary)]"
                      @click="toggleExpandedTool(profile.tool)"
                    >
                      {{ expandedTool === profile.tool ? t(store.settings.locale, 'common.hide') : t(store.settings.locale, 'common.view') }}
                    </button>
                  </div>
                </div>
              </div>
              <SettingsSwitch
                :checked="takeoverEnabledFor(profile.tool)"
                :disabled="takeoverLoading[profile.tool]"
                @toggle="toggleToolTakeover(profile.tool)"
              />
            </div>
            <div
              v-if="expandedTool === profile.tool"
              class="mt-1.5 border-t border-gray-100 pt-1.5 dark:border-neutral-800/60"
            >
              <div
                v-for="path in toolConfigPaths(profile)"
                :key="path"
                class="break-all font-mono text-[9px] leading-relaxed text-gray-400 dark:text-gray-500"
              >
                {{ path }}
              </div>
              <div
                v-if="inlineScopeWarning(profile)"
                class="mt-1 text-[9px] leading-relaxed text-amber-600 dark:text-amber-300"
              >
                {{ inlineScopeWarning(profile) }}
              </div>
            </div>
          </div>
        </div>

        <div v-if="toolAlerts.length" class="mt-2 space-y-2">
          <div
            v-for="alert in toolAlerts"
            :key="`${alert.toolId}-${alert.message}`"
            :class="[
              'rounded-xl border px-3 py-2 text-[10px] leading-snug',
              alert.tone === 'error'
                ? 'border-red-100 bg-red-50 text-red-600 dark:border-red-500/20 dark:bg-red-500/10 dark:text-red-300'
                : 'border-amber-100 bg-amber-50 text-amber-700 dark:border-amber-500/20 dark:bg-amber-500/10 dark:text-amber-300'
            ]"
          >
            <div class="font-medium">{{ alert.tool }}</div>
            <div class="mt-0.5">{{ alert.message }}</div>
            <div v-if="alert.actions?.length" class="mt-2 flex gap-1.5">
              <button
                class="rounded-lg bg-amber-500/10 px-2 py-1 text-[10px] font-medium text-amber-700 transition-colors hover:bg-amber-500/15 dark:text-amber-300"
                @click.stop="resolveTakeoverConflict(alert.toolId, 'force_reclaim')"
              >
                {{ t(store.settings.locale, 'settings.takeoverConflictForce') }}
              </button>
              <button
                class="rounded-lg bg-gray-200/70 px-2 py-1 text-[10px] font-medium text-gray-600 transition-colors hover:bg-gray-200 dark:bg-neutral-800 dark:text-gray-300"
                @click.stop="resolveTakeoverConflict(alert.toolId, 'disable_takeover')"
              >
                {{ t(store.settings.locale, 'settings.takeoverConflictDisable') }}
              </button>
            </div>
          </div>
        </div>
      </div>

      <div class="flex items-center justify-between py-2 px-4 text-[13px]">
        <div class="flex flex-col">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.includeErrorRequests') }}</span>
          <span class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.includeErrorRequestsDesc') }}</span>
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
