<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import LobeIcon from '../LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../../iconConfig'

interface OpenCodeSchemaStatus {
  dbFound: boolean
  dbPath: string | null
  schemaCompatible: boolean
  compatibilityMode?: 'full' | 'message_only' | 'incompatible'
  persistedCompatibilityMode?: 'full' | 'message_only' | 'incompatible' | 'unknown' | null
  incompatibilityReason: string | null
  messageIdConflict?: {
    hasConflict: boolean
    conflictCount: number
    sampleIds: string[]
  }
}

const store = useMonitorStore()
const opencodeSchema = ref<OpenCodeSchemaStatus | null>(null)

const loadOpenCodeSchemaStatus = async () => {
  try {
    opencodeSchema.value = await invoke<OpenCodeSchemaStatus>('get_opencode_schema_status')
  } catch {
    // 静默失败
  }
}

onMounted(() => {
  loadOpenCodeSchemaStatus()
})

function compactPath(path: string | null | undefined): string {
  if (!path) return ''
  return path.replace(/^\/Users\/[^/]+/, '~')
}
</script>

<template>
  <div class="py-2.5 px-4">
    <div class="mb-1 text-[12px] font-medium text-gray-600 dark:text-gray-300">{{ t(store.settings.locale, 'settings.localScanTitle') }}</div>
    <div class="mb-2 text-[10px] leading-relaxed text-gray-400">{{ t(store.settings.locale, 'settings.localScanDesc') }}</div>
    <div class="space-y-1.5">

      <div class="rounded-lg border border-gray-100 bg-white px-2.5 py-1.5 dark:border-neutral-800 dark:bg-neutral-950">
        <div class="flex items-start gap-2">
          <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-gray-50 dark:bg-neutral-800">
            <LobeIcon :slug="TOOL_LOBE_ICONS.claude_code" :size="15" @error="() => {}" />
          </div>
          <div class="min-w-0">
            <div class="text-[10.5px] font-medium leading-none text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanClaudeCode') }}</div>
            <div class="mt-0.5 break-all font-mono text-[9px] leading-tight text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.localScanClaudeCodePath') }}</div>
          </div>
        </div>
      </div>

      <div class="rounded-lg border border-gray-100 bg-white px-2.5 py-1.5 dark:border-neutral-800 dark:bg-neutral-950">
        <div class="flex items-start gap-2">
          <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-gray-50 dark:bg-neutral-800">
            <LobeIcon :slug="TOOL_LOBE_ICONS.codex" :size="15" @error="() => {}" />
          </div>
          <div class="min-w-0">
            <div class="text-[10.5px] font-medium leading-none text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanCodex') }}</div>
            <div class="mt-0.5 break-all font-mono text-[9px] leading-tight text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.localScanCodexPath') }}</div>
          </div>
        </div>
      </div>

      <div
        class="rounded-lg border px-2.5 py-1.5"
        :class="opencodeSchema?.dbFound && !opencodeSchema.schemaCompatible
          ? 'border-amber-200 bg-amber-50 dark:border-amber-800 dark:bg-amber-950/30'
          : 'border-gray-100 bg-white dark:border-neutral-800 dark:bg-neutral-950'"
      >
        <div class="flex items-start gap-2">
          <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-gray-50 dark:bg-neutral-800">
            <LobeIcon :slug="TOOL_LOBE_ICONS.opencode" :size="15" @error="() => {}" />
          </div>
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-1.5">
              <span class="text-[10.5px] font-medium leading-none text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanOpenCode') }}</span>
              <span
                v-if="opencodeSchema?.dbFound && !opencodeSchema.schemaCompatible"
                class="rounded px-1 py-px text-[9px] font-medium bg-amber-100 text-amber-700 dark:bg-amber-900/50 dark:text-amber-300"
              >{{ t(store.settings.locale, 'settings.localScanSchemaIncompatibleBadge') }}</span>
              <span
                v-else-if="opencodeSchema?.dbFound && opencodeSchema.compatibilityMode === 'message_only'"
                class="rounded px-1 py-px text-[9px] font-medium bg-sky-100 text-sky-700 dark:bg-sky-900/50 dark:text-sky-300"
              >{{ t(store.settings.locale, 'settings.localScanMessageOnlyBadge') }}</span>
            </div>
            <div class="mt-0.5 break-all font-mono text-[9px] leading-tight text-gray-400 dark:text-gray-500">
              {{ compactPath(opencodeSchema?.dbPath) || t(store.settings.locale, 'settings.localScanOpenCodePath') }}
            </div>
            <div
              v-if="opencodeSchema?.persistedCompatibilityMode && opencodeSchema.persistedCompatibilityMode !== opencodeSchema.compatibilityMode"
              class="mt-1 text-[9px] leading-relaxed text-gray-400 dark:text-gray-500"
            >
              {{ t(store.settings.locale, 'settings.localScanSchemaRefreshing') }}
            </div>
            <div
              v-if="opencodeSchema?.dbFound && opencodeSchema.compatibilityMode === 'message_only'"
              class="mt-1 text-[9px] leading-relaxed text-sky-600 dark:text-sky-400"
            >
              {{ t(store.settings.locale, 'settings.localScanMessageOnlyWarning') }}
            </div>
            <div
              v-if="opencodeSchema?.messageIdConflict?.hasConflict"
              class="mt-1 text-[9px] leading-relaxed text-amber-600 dark:text-amber-400"
            >
              {{ t(store.settings.locale, 'settings.localScanMessageIdConflictWarning') }}
              <template v-if="opencodeSchema.messageIdConflict.conflictCount > 0">
                {{ t(store.settings.locale, 'settings.localScanMessageIdConflictCount').replace('{count}', String(opencodeSchema.messageIdConflict.conflictCount)) }}
              </template>
            </div>
            <div
              v-if="opencodeSchema?.dbFound && !opencodeSchema.schemaCompatible"
              class="mt-1 text-[9px] leading-relaxed text-amber-600 dark:text-amber-400"
            >
              {{ t(store.settings.locale, 'settings.localScanSchemaWarning') }}
              <template v-if="opencodeSchema.incompatibilityReason">
                <br>
                <span class="opacity-75">{{ opencodeSchema.incompatibilityReason }}</span>
              </template>
            </div>
          </div>
        </div>
      </div>

      <div class="rounded-lg border border-gray-100 bg-white px-2.5 py-1.5 dark:border-neutral-800 dark:bg-neutral-950">
        <div class="flex items-start gap-2">
          <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-gray-50 dark:bg-neutral-800">
            <LobeIcon :slug="TOOL_LOBE_ICONS.reasonix" :size="15" @error="() => {}" />
          </div>
          <div class="min-w-0">
            <div class="flex items-center gap-1.5">
              <span class="text-[10.5px] font-medium leading-none text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanReasonix') }}</span>
              <span class="rounded px-1 py-px text-[9px] font-medium bg-sky-100 text-sky-700 dark:bg-sky-900/50 dark:text-sky-300">
                {{ t(store.settings.locale, 'settings.localScanRequestOnlyBadge') }}
              </span>
            </div>
            <div class="mt-0.5 break-all font-mono text-[9px] leading-tight text-gray-400 dark:text-gray-500">
              {{ t(store.settings.locale, 'settings.localScanReasonixPath') }}
            </div>
            <div class="mt-1 text-[9px] leading-relaxed text-sky-600 dark:text-sky-400">
              {{ t(store.settings.locale, 'settings.localScanReasonixWarning') }}
            </div>
          </div>
        </div>
      </div>

      <div class="rounded-lg border border-gray-100 bg-white px-2.5 py-1.5 dark:border-neutral-800 dark:bg-neutral-950">
        <div class="flex items-start gap-2">
          <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-gray-50 dark:bg-neutral-800">
            <LobeIcon :slug="TOOL_LOBE_ICONS.gemini" :size="15" @error="() => {}" />
          </div>
          <div class="min-w-0">
            <div class="text-[10.5px] font-medium leading-none text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanGemini') }}</div>
            <div class="mt-0.5 break-all font-mono text-[9px] leading-tight text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.localScanGeminiPath') }}</div>
          </div>
        </div>
      </div>

    </div>
  </div>
</template>
