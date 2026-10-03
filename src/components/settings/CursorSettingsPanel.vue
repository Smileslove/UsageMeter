<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { RefreshCw, Upload } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { t, backendErrorLabel } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenValue, formatRelativeTime } from '../../utils/format'
import { getCursorStatus, syncCursorUsage, selectCursorCsv, previewCursorCsvImport, importCursorCsv, listCursorImportBatches, revokeCursorImportBatch, type CursorStatus, type CursorBatch, type CursorCsvPreview } from '../../api/cursorApi'
import SettingsSwitch from './SettingsSwitch.vue'

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)
const status = ref<CursorStatus | null>(null)
const batches = ref<CursorBatch[]>([])
const preview = ref<CursorCsvPreview | null>(null)
const csvPath = ref('')
const namespace = ref('offline')
const useAccount = ref(false)
const databasePath = ref(store.settings.cursor.databasePath ?? '')
const historyStart = ref('')
const busy = ref(false)
const error = ref('')
const errorElement = ref<HTMLElement | null>(null)
const message = ref('')
const csvBatches = computed(() => batches.value.filter(batch => batch.source === 'csv'))
const stateText = computed(() => {
  const code = status.value?.sync?.status ?? status.value?.authStatus ?? 'unknown'
  const key = `cursor.status.${code.replace(/^cursor_/, '')}`
  const translated = t(locale.value, key)
  return translated === key ? backendErrorLabel(locale.value, code) : translated
})
function metadataStatusLabel(code: string) {
  const key = `cursor.status.${code.replace(/^cursor_/, '')}`
  const label = t(locale.value, key)
  return label === key ? backendErrorLabel(locale.value, code) : label
}
function showError(cause: unknown) {
  const code = typeof cause === 'string' ? cause : (cause as { code?: string })?.code ?? 'cursor_import_failed'
  error.value = code
}
async function load() {
  const results = await Promise.all([getCursorStatus(), listCursorImportBatches()])
  status.value = results[0]
  batches.value = results[1]
}
async function run(operation: () => Promise<void>) {
  if (busy.value) return
  busy.value = true; error.value = ''; message.value = ''
  try { await operation(); await load() } catch (cause) { showError(cause); await nextTick(); errorElement.value?.focus() }
  finally { busy.value = false }
}
async function toggleSync() {
  await run(async () => {
    const previous = store.settings.cursor.accountSyncEnabled
    store.settings.cursor.accountSyncEnabled = !previous
    try { await store.saveSettings() } catch (cause) { store.settings.cursor.accountSyncEnabled = previous; throw cause }
    store.cursorQuota = null
    if (!previous) { await syncCursorUsage(); await store.fetchCursorQuota(true); await store.refreshUsage() }
  })
}
async function synchronize() {
  await run(async () => {
    const start = historyStart.value ? Date.parse(`${historyStart.value}T00:00:00Z`) : undefined
    await syncCursorUsage(start)
    await store.fetchCursorQuota(true)
    await store.refreshUsage()
    message.value = 'cursor.synced'
  })
}
async function selectFile() {
  await run(async () => {
    const path = await selectCursorCsv()
    if (!path) return
    preview.value = null; csvPath.value = path
    preview.value = await previewCursorCsvImport(path)
  })
}
async function importFile() {
  if (!preview.value || !namespace.value.trim()) return
  await run(async () => {
    const accountKey = useAccount.value ? status.value?.accountKey ?? null : null
    if (useAccount.value && !accountKey) throw 'cursor_identity_mismatch'
    await importCursorCsv(csvPath.value, preview.value!.contentHash, namespace.value, accountKey)
    preview.value = null; csvPath.value = ''
    await store.refreshUsage()
    message.value = 'cursor.imported'
  })
}
async function revoke(batch: CursorBatch) {
  await run(async () => { await revokeCursorImportBatch(batch.batchId); await store.refreshUsage(); message.value = 'cursor.revoked' })
}
async function savePath() {
  await run(async () => {
    const previous = store.settings.cursor.databasePath
    store.settings.cursor.databasePath = databasePath.value.trim() || null
    try { await store.saveSettings() } catch (cause) { store.settings.cursor.databasePath = previous; throw cause }
    store.cursorQuota = null
  })
}
onMounted(() => { void load().catch(showError) })
</script>

<template>
  <section class="min-w-0 space-y-3 border-b border-[var(--theme-border-default)] px-4 py-4" :aria-label="t(locale, 'cursor.title')" :aria-busy="busy">
    <div class="flex items-center justify-between gap-3">
      <h3 class="text-sm font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'cursor.title') }}</h3>
      <SettingsSwitch :checked="store.settings.cursor.accountSyncEnabled" :disabled="busy || store.saving || (!status?.accountSyncAvailable && !store.settings.cursor.accountSyncEnabled)" :aria-label="t(locale, 'cursor.enableSync')" @toggle="toggleSync" />
    </div>
    <p class="text-xs leading-relaxed text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.description') }}</p>
    <p v-if="status && !status.accountSyncAvailable" class="text-xs leading-relaxed text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.contractUnverified') }}</p>
    <p class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.currentStatus', { status: stateText }) }}</p>
    <p v-if="status" class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.localStatus', { status: metadataStatusLabel(status.metadataStatus ?? 'unknown') }) }}</p>
    <p v-if="status?.sync?.lastSuccessMs" class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.lastSync', { time: formatRelativeTime(status.sync.lastSuccessMs / 1000, locale) }) }}</p>
    <div class="flex flex-wrap gap-2">
      <button type="button" class="theme-button-secondary inline-flex min-h-10 items-center gap-2 rounded-lg px-3 text-xs disabled:opacity-50" :disabled="busy || !store.settings.cursor.accountSyncEnabled || !status?.accountSyncAvailable" @click="synchronize"><RefreshCw class="h-4 w-4" :class="{ 'animate-spin motion-reduce:animate-none': busy }" aria-hidden="true" />{{ t(locale, 'cursor.syncNow') }}</button>
      <button type="button" class="theme-button-secondary inline-flex min-h-10 items-center gap-2 rounded-lg px-3 text-xs disabled:opacity-50" :disabled="busy" @click="selectFile"><Upload class="h-4 w-4" aria-hidden="true" />{{ t(locale, 'cursor.selectCsv') }}</button>
    </div>
    <div v-if="preview" class="space-y-3 rounded-lg border border-[var(--theme-border-default)] p-3">
      <p class="break-words text-xs font-medium text-[var(--theme-text-primary)]">{{ t(locale, 'cursor.preview', { events: formatRequestCount(preview.eventCount), tokens: formatTokenValue(preview.totalTokens) }) }}</p>
      <p class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.meteredCost', { value: preview.usageCostUsd == null ? '—' : formatCost(preview.usageCostUsd, store.settings.currency) }) }}</p>
      <p v-if="preview.unknownCostEvents" class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.partialCost', { count: preview.unknownCostEvents }) }}</p>
      <p class="text-xs leading-relaxed text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.importScopeNote') }}</p>
      <label v-if="status?.accountKey" class="flex items-start gap-2 text-xs text-[var(--theme-text-secondary)]"><input v-model="useAccount" type="checkbox" class="mt-0.5" :disabled="busy" />{{ t(locale, 'cursor.useCurrentAccount') }}</label>
      <label v-if="!useAccount" class="block space-y-1 text-xs text-[var(--theme-text-secondary)]"><span>{{ t(locale, 'cursor.namespace') }}</span><input v-model="namespace" type="text" maxlength="128" class="theme-input min-h-10 w-full rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-2" :disabled="busy" /></label>
      <button type="button" class="theme-button-secondary min-h-10 rounded-lg px-3 text-xs disabled:opacity-50" :disabled="busy || !namespace.trim()" @click="importFile">{{ t(locale, 'cursor.confirmImport') }}</button>
    </div>
    <details class="text-xs text-[var(--theme-text-secondary)]">
      <summary class="cursor-pointer py-2">{{ t(locale, 'cursor.advanced') }}</summary>
      <div class="mt-2 space-y-3">
        <label class="block space-y-1"><span>{{ t(locale, 'cursor.databasePath') }}</span><input v-model="databasePath" type="text" class="theme-input min-h-10 w-full rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-2" :disabled="busy" /></label>
        <button type="button" class="theme-button-secondary min-h-10 rounded-lg px-3 disabled:opacity-50" :disabled="busy" @click="savePath">{{ t(locale, 'common.save') }}</button>
        <label class="block space-y-1"><span>{{ t(locale, 'cursor.historyStart') }}</span><input v-model="historyStart" type="date" class="theme-input min-h-10 max-w-full rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-2" :disabled="busy" /></label>
        <p>{{ t(locale, 'cursor.historyNote') }}</p>
      </div>
    </details>
    <ul v-if="csvBatches.length" class="space-y-2">
      <li v-for="batch in csvBatches" :key="batch.batchId" class="flex items-center justify-between gap-2 text-xs">
        <span class="min-w-0 text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.batch', { id: batch.accountKey.slice(0, 8), count: formatRequestCount(batch.eventCount) }) }}</span>
        <button type="button" class="theme-button-secondary min-h-10 shrink-0 rounded-lg px-2 disabled:opacity-50" :disabled="busy" @click="revoke(batch)">{{ t(locale, 'cursor.revoke') }}</button>
      </li>
    </ul>
    <p v-if="busy" role="status" class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'cursor.working') }}</p>
    <p v-if="message" role="status" class="text-xs text-[var(--theme-status-info-fg)]">{{ t(locale, message) }}</p>
    <p v-if="error" ref="errorElement" role="alert" tabindex="-1" class="break-words text-xs text-red-500 dark:text-red-400">{{ backendErrorLabel(locale, error) }}</p>
  </section>
</template>
