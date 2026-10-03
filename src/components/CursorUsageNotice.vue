<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { getCursorStatus, type CursorStatus } from '../api/cursorApi'
import { OFFICIAL_CURSOR_ACCOUNT_SOURCE_ID } from '../types'
const store = useMonitorStore()
const hasUsage = ref(false)
const quality = ref<CursorStatus | null>(null)
const visible = computed(() => {
  const tool = store.settings.clientTools.activeToolFilter
  const source = store.settings.sourceAware.activeSourceFilter
  return (!tool || tool === 'cursor') && (!source || source === OFFICIAL_CURSOR_ACCOUNT_SOURCE_ID)
    && (tool === 'cursor' || hasUsage.value)
})
let unlisten: UnlistenFn | undefined
let closed = false
async function refresh() {
  try { quality.value = await getCursorStatus(); hasUsage.value = quality.value.usageEventCount > 0; store.hasCursorUsage = hasUsage.value } catch { /* A notice must not block existing views. */ }
}
onMounted(async () => {
  await refresh()
  const stop = await listen('local_usage_synced', () => { void refresh() }).catch(() => undefined)
  if (closed) stop?.()
  else unlisten = stop
})
onUnmounted(() => { closed = true; unlisten?.() })
</script>
<template>
  <div v-if="visible" class="mb-3 space-y-1 rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-3 py-2 text-xs leading-relaxed text-[var(--theme-text-secondary)]">
    <p>{{ t(store.settings.locale, 'cursor.scopeNote') }}</p>
    <p v-if="quality && (quality.unknownCostEvents > 0 || quality.incompleteUsageEvents > 0)">{{ t(store.settings.locale, 'cursor.knownTotalsNote') }}</p>
  </div>
</template>
