<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import ConfirmDialog from './ConfirmDialog.vue'

interface LocalCacheStats {
  totalLocalFacts: number
  orphanLocalFacts: number
}

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

const localCacheStats = ref<LocalCacheStats>({ totalLocalFacts: 0, orphanLocalFacts: 0 })
const localCacheBusy = ref(false)
const localCacheMessage = ref('')
const localCacheMessageIsError = ref(false)
const localCachePurgeDays = ref<0 | 30 | 90 | 180>(90)
const localCacheConfirmMode = ref<'purge-orphan' | 'rebuild-cache' | null>(null)
const opencodeSchema = ref<OpenCodeSchemaStatus | null>(null)

const loadLocalCacheStats = async () => {
  try {
    localCacheStats.value = await invoke<LocalCacheStats>('get_local_usage_maintenance_stats')
  } catch (error) {
    localCacheMessage.value = `${t(store.settings.locale, 'settings.localCacheLoadFailed')}: ${error}`
    localCacheMessageIsError.value = true
  }
}

const loadOpenCodeSchemaStatus = async () => {
  try {
    opencodeSchema.value = await invoke<OpenCodeSchemaStatus>('get_opencode_schema_status')
  } catch {
    // 静默失败，不影响其他功能
  }
}

const openLocalCachePurgeConfirm = () => {
  localCacheConfirmMode.value = 'purge-orphan'
}

const openLocalCacheRebuildConfirm = () => {
  localCacheConfirmMode.value = 'rebuild-cache'
}

const closeLocalCacheConfirm = () => {
  localCacheConfirmMode.value = null
}

const confirmLocalCacheAction = async () => {
  const mode = localCacheConfirmMode.value
  if (!mode) {
    return
  }

  localCacheBusy.value = true
  localCacheMessage.value = ''
  localCacheMessageIsError.value = false

  try {
    if (mode === 'purge-orphan') {
      const removed = await invoke<number>('purge_orphan_local_facts', {
        olderThanDays: localCachePurgeDays.value,
      })
      localCacheMessage.value = t(store.settings.locale, 'settings.localCachePurgeSuccess')
        .replace('{count}', String(removed))
    } else {
      await invoke('rebuild_local_usage_cache')
      localCacheMessage.value = t(store.settings.locale, 'settings.localCacheRebuildSuccess')
    }

    await loadLocalCacheStats()
    await store.refreshUsage()
  } catch (error) {
    localCacheMessage.value = `${t(store.settings.locale, 'settings.localCacheActionFailed')}: ${error}`
    localCacheMessageIsError.value = true
  } finally {
    localCacheBusy.value = false
    closeLocalCacheConfirm()
  }
}

onMounted(() => {
  loadLocalCacheStats()
  loadOpenCodeSchemaStatus()
})
</script>

<template>
  <div class="p-3 px-4">
    <!-- 数据来源告知区域 -->
    <div class="mb-3">
      <div class="mb-1.5 text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanTitle') }}</div>
      <div class="mb-2 text-[10px] leading-relaxed text-gray-400">{{ t(store.settings.locale, 'settings.localScanDesc') }}</div>
      <div class="space-y-1">
        <!-- Claude Code -->
        <div class="flex items-start gap-2 rounded-lg border border-gray-100 bg-white px-2.5 py-1.5 dark:border-neutral-800 dark:bg-neutral-950">
          <div class="mt-px h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--theme-accent-primary)]"></div>
          <div class="min-w-0">
            <div class="text-[11px] font-medium text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanClaudeCode') }}</div>
            <div class="mt-0.5 break-all font-mono text-[9.5px] text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.localScanClaudeCodePath') }}</div>
          </div>
        </div>
        <!-- Codex -->
        <div class="flex items-start gap-2 rounded-lg border border-gray-100 bg-white px-2.5 py-1.5 dark:border-neutral-800 dark:bg-neutral-950">
          <div class="mt-px h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--theme-accent-primary)]"></div>
          <div class="min-w-0">
            <div class="text-[11px] font-medium text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanCodex') }}</div>
            <div class="mt-0.5 break-all font-mono text-[9.5px] text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.localScanCodexPath') }}</div>
          </div>
        </div>
        <!-- OpenCode -->
        <div
          class="flex items-start gap-2 rounded-lg border px-2.5 py-1.5"
          :class="opencodeSchema?.dbFound && !opencodeSchema.schemaCompatible
            ? 'border-amber-200 bg-amber-50 dark:border-amber-800 dark:bg-amber-950/30'
            : 'border-gray-100 bg-white dark:border-neutral-800 dark:bg-neutral-950'"
        >
          <div
            class="mt-px h-1.5 w-1.5 shrink-0 rounded-full"
            :class="opencodeSchema?.dbFound && !opencodeSchema.schemaCompatible
              ? 'bg-amber-400'
              : 'bg-[var(--theme-accent-primary)]'"
          ></div>
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-1.5">
              <span class="text-[11px] font-medium text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localScanOpenCode') }}</span>
              <!-- 兼容性状态徽章 -->
              <span
                v-if="opencodeSchema?.dbFound && !opencodeSchema.schemaCompatible"
                class="rounded px-1 py-px text-[9px] font-medium bg-amber-100 text-amber-700 dark:bg-amber-900/50 dark:text-amber-300"
              >{{ t(store.settings.locale, 'settings.localScanSchemaIncompatibleBadge') }}</span>
              <span
                v-else-if="opencodeSchema?.dbFound && opencodeSchema.compatibilityMode === 'message_only'"
                class="rounded px-1 py-px text-[9px] font-medium bg-sky-100 text-sky-700 dark:bg-sky-900/50 dark:text-sky-300"
              >{{ t(store.settings.locale, 'settings.localScanMessageOnlyBadge') }}</span>
              <span
                v-else-if="opencodeSchema?.dbFound"
                class="rounded px-1 py-px text-[9px] font-medium bg-emerald-100 text-emerald-700 dark:bg-emerald-900/50 dark:text-emerald-300"
              >{{ t(store.settings.locale, 'settings.localScanDetectedBadge') }}</span>
            </div>
            <div class="mt-0.5 break-all font-mono text-[9.5px] text-gray-400 dark:text-gray-500">
              {{ opencodeSchema?.dbPath || t(store.settings.locale, 'settings.localScanOpenCodePath') }}
            </div>
            <!-- OpenCode 特殊说明 -->
            <div class="mt-1 text-[9.5px] leading-relaxed text-gray-400 dark:text-gray-500">
              {{ t(store.settings.locale, 'settings.localScanOpenCodeNote') }}
            </div>
            <div
              v-if="opencodeSchema?.persistedCompatibilityMode && opencodeSchema.persistedCompatibilityMode !== opencodeSchema.compatibilityMode"
              class="mt-1 text-[9.5px] leading-relaxed text-gray-400 dark:text-gray-500"
            >
              {{ t(store.settings.locale, 'settings.localScanSchemaRefreshing') }}
            </div>
            <div
              v-if="opencodeSchema?.dbFound && opencodeSchema.compatibilityMode === 'message_only'"
              class="mt-1 text-[9.5px] leading-relaxed text-sky-600 dark:text-sky-400"
            >
              {{ t(store.settings.locale, 'settings.localScanMessageOnlyWarning') }}
            </div>
            <div
              v-if="opencodeSchema?.messageIdConflict?.hasConflict"
              class="mt-1 text-[9.5px] leading-relaxed text-amber-600 dark:text-amber-400"
            >
              {{ t(store.settings.locale, 'settings.localScanMessageIdConflictWarning') }}
              <template v-if="opencodeSchema.messageIdConflict.conflictCount > 0">
                {{ t(store.settings.locale, 'settings.localScanMessageIdConflictCount').replace('{count}', String(opencodeSchema.messageIdConflict.conflictCount)) }}
              </template>
            </div>
            <!-- schema 不兼容警告 -->
            <div
              v-if="opencodeSchema?.dbFound && !opencodeSchema.schemaCompatible"
              class="mt-1 text-[9.5px] leading-relaxed text-amber-600 dark:text-amber-400"
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
    </div>

    <div class="mb-3 flex items-start justify-between gap-3">
      <div class="min-w-0">
        <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localCacheTitle') }}</div>
        <div class="mt-0.5 text-[10px] leading-relaxed text-gray-400">{{ t(store.settings.locale, 'settings.localCacheDesc') }}</div>
      </div>
    </div>

    <div class="mb-3 grid grid-cols-2 gap-2">
      <div class="rounded-xl border border-gray-100 bg-white px-2.5 py-2 dark:border-neutral-800 dark:bg-neutral-950">
        <div class="text-[10px] text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.localCacheTotalFacts') }}</div>
        <div class="mt-0.5 text-sm font-semibold tabular-nums text-gray-700 dark:text-gray-200">{{ localCacheStats.totalLocalFacts }}</div>
        <div class="truncate text-[10px] text-gray-400 dark:text-gray-500" :title="t(store.settings.locale, 'settings.localCacheTotalFactsDesc')">
          {{ t(store.settings.locale, 'settings.localCacheTotalFactsDesc') }}
        </div>
      </div>
      <div class="rounded-xl border border-gray-100 bg-white px-2.5 py-2 dark:border-neutral-800 dark:bg-neutral-950">
        <div class="text-[10px] text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.localCacheOrphanFacts') }}</div>
        <div
          class="mt-0.5 text-sm font-semibold tabular-nums"
          :class="localCacheStats.orphanLocalFacts > 0 ? 'text-amber-500 dark:text-amber-400' : 'text-gray-700 dark:text-gray-200'"
        >
          {{ localCacheStats.orphanLocalFacts }}
        </div>
        <div class="truncate text-[10px] text-gray-400 dark:text-gray-500" :title="t(store.settings.locale, 'settings.localCacheOrphanFactsDesc')">
          {{ t(store.settings.locale, 'settings.localCacheOrphanFactsDesc') }}
        </div>
      </div>
    </div>

    <div class="mb-2 flex items-center justify-between gap-2 rounded-xl border border-gray-100 bg-white px-2.5 py-2 dark:border-neutral-800 dark:bg-neutral-950">
      <div class="min-w-0">
        <div class="text-[11px] font-medium text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localCachePurgeOrphan') }}</div>
        <div
          class="mt-0.5 truncate text-[10px] text-gray-400 dark:text-gray-500"
          :title="localCacheStats.orphanLocalFacts === 0
            ? t(store.settings.locale, 'settings.localCacheNoOrphan')
            : t(store.settings.locale, 'settings.localCachePurgeOrphanDesc')"
        >
          {{ localCacheStats.orphanLocalFacts === 0
            ? t(store.settings.locale, 'settings.localCacheNoOrphan')
            : t(store.settings.locale, 'settings.localCachePurgeOrphanDesc') }}
        </div>
      </div>
      <div class="flex shrink-0 items-center gap-1.5">
        <select
          v-model.number="localCachePurgeDays"
          :disabled="localCacheBusy || localCacheStats.orphanLocalFacts === 0"
          class="rounded-lg border border-gray-200 bg-white px-2 py-1.5 text-[11px] text-gray-700 outline-none transition-colors focus:border-gray-300 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:text-gray-200 dark:focus:border-neutral-600"
        >
          <option :value="0">{{ t(store.settings.locale, 'settings.localCachePurgeWindowAll') }}</option>
          <option :value="30">{{ t(store.settings.locale, 'settings.localCachePurgeWindow30d') }}</option>
          <option :value="90">{{ t(store.settings.locale, 'settings.localCachePurgeWindow90d') }}</option>
          <option :value="180">{{ t(store.settings.locale, 'settings.localCachePurgeWindow180d') }}</option>
        </select>
        <button
          type="button"
          class="rounded-lg bg-[var(--theme-accent-primary)] px-2.5 py-1.5 text-[11px] font-medium text-[var(--theme-accent-contrast)] transition-opacity hover:opacity-90 disabled:opacity-50"
          :disabled="localCacheBusy || localCacheStats.orphanLocalFacts === 0"
          @click="openLocalCachePurgeConfirm"
        >
          {{ t(store.settings.locale, 'settings.localCachePurgeOrphan') }}
        </button>
      </div>
    </div>

    <div class="flex items-center justify-between gap-2 rounded-xl border border-gray-100 bg-white px-2.5 py-2 dark:border-neutral-800 dark:bg-neutral-950">
      <div class="min-w-0">
        <div class="text-[11px] font-medium text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.localCacheRebuild') }}</div>
        <div class="mt-0.5 truncate text-[10px] text-gray-400 dark:text-gray-500" :title="t(store.settings.locale, 'settings.localCacheRebuildDesc')">
          {{ t(store.settings.locale, 'settings.localCacheRebuildDesc') }}
        </div>
      </div>
      <button
        type="button"
        class="shrink-0 rounded-lg bg-[var(--theme-accent-primary)] px-2.5 py-1.5 text-[11px] font-medium text-[var(--theme-accent-contrast)] transition-opacity hover:opacity-90 disabled:opacity-50"
        :disabled="localCacheBusy"
        @click="openLocalCacheRebuildConfirm"
      >
        {{ localCacheBusy ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'settings.localCacheRebuild') }}
      </button>
    </div>

    <div
      v-if="localCacheMessage"
      class="mt-2 text-[10px] leading-relaxed"
      :class="localCacheMessageIsError ? 'text-red-500 dark:text-red-400' : 'text-emerald-500 dark:text-emerald-400'"
    >
      {{ localCacheMessage }}
    </div>

    <ConfirmDialog
      :open="!!localCacheConfirmMode"
      :title="t(store.settings.locale, localCacheConfirmMode === 'purge-orphan' ? 'settings.localCachePurgeConfirmTitle' : 'settings.localCacheRebuildConfirmTitle')"
      :body="t(store.settings.locale, localCacheConfirmMode === 'purge-orphan' ? 'settings.localCachePurgeConfirmBody' : 'settings.localCacheRebuildConfirmBody')"
      :confirm-label="localCacheBusy ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'common.confirm')"
      :cancel-label="t(store.settings.locale, 'common.cancel')"
      :busy="localCacheBusy"
      @cancel="closeLocalCacheConfirm"
      @confirm="confirmLocalCacheAction"
    />
  </div>
</template>
