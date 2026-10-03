<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { Activity, ArrowDown, ArrowUp, ArrowDownToLine, ArrowUpFromLine, BarChart3, Code2, Copy, Database, DollarSign, ExternalLink, FileCheck2, FileText, Info, Link2, Timer, X, Zap } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t } from '../../i18n'
import type { RequestRecord, RequestSortField } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useClipboard } from '../composables/useClipboard'
import { useRequestTable, REQUEST_COLUMNS } from '../composables/useRequestTable'
import { normalizeSessionTool } from '../../composables/useSessionViewData'
import {
  clearManualRequestAttribution,
  clearManualSessionAttribution,
  clearManualTimeRangeAttribution,
  setManualRequestAttribution,
  setManualSessionAttribution,
  setManualTimeRangeAttribution,
} from '../../stores/sessionQueries'
import LobeIcon from '../../components/LobeIcon.vue'
import DesktopSelect from '../components/DesktopSelect.vue'
import type { SelectOption } from '../components/DesktopSelect.vue'
import PaginationBar from '../components/PaginationBar.vue'
import ColumnConfigPopover from '../components/ColumnConfigPopover.vue'
import DrawerShell from '../components/DrawerShell.vue'
import SearchInput from '../components/SearchInput.vue'
import StatusBadge from '../components/StatusBadge.vue'
import { shortId } from '../../utils/format'

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

const {
  formatTime,
  formatTokens,
  formatCost,
  formatDuration,
  requestModelLabel,
  requestStatusLabel,
  requestStatusClasses,
  requestCoverageLabel,
  requestProjectLabel,
  requestAttributionLabel,
  requestReconciliationStatusLabel,
  requestAccountingRoleLabel,
  requestReconciliationConfidenceLabel,
  requestObservationSourcesLabel,
  requestObservationIdLabel,
  requestSourceLabel,
  requestToolLabel,
  requestCacheTokens,
  requestHasProxyPerformance,
  getToolIcon,
} = useSessionDisplay(store)

const selectedTool = ref<string | null>(normalizeSessionTool(store.settings.clientTools.activeToolFilter))

const {
  currentPage,
  pageSize,
  searchInput,
  statusFilter,
  coverageFilter,
  perfFilter,
  sortDir,
  totalPages,
  pageNumbers,
  loading,
  hasPrev,
  hasNext,
  hasActiveFilters,
  records,
  total,
  reload,
  gotoPage,
  nextPage,
  prevPage,
  toggleSort,
  isSortedBy,
  toggleColumn,
  isColumnVisible,
  resetFilters,
  initialize,
  dispose,
} = useRequestTable(store, selectedTool)

const visibleCols = computed(() => REQUEST_COLUMNS.filter(c => isColumnVisible(c.key)))

async function applyPendingFilters() {
  const pending = nav.consumePendingFilters()
  if (!pending) return
  if (pending.sourceId && store.settings.sourceAware.activeSourceFilter !== pending.sourceId) {
    await store.setActiveSourceFilter(pending.sourceId)
  }
  if (pending.tool && store.settings.clientTools.activeToolFilter !== pending.tool) {
    await store.setActiveToolFilter(pending.tool)
  }
}

watch(() => nav.pendingConsumeTick, () => {
  if (nav.currentPage === 'requests') void applyPendingFilters()
})

const selectedRequest = ref<RequestRecord | null>(null)
const drawerOpen = ref(false)
const MANUAL_ATTRIBUTION_AUTOMATIC = '__automatic__'
const MANUAL_ATTRIBUTION_UNATTRIBUTED = '__unattributed__'
const manualAttributionSelection = ref(MANUAL_ATTRIBUTION_AUTOMATIC)
const manualAttributionSaving = ref(false)
const manualAttributionError = ref(false)
const manualAttributionResult = ref<number | null>(null)
const applyManualAttributionToSession = ref(false)
const manualTimeRangeStart = ref('')
const manualTimeRangeEnd = ref('')
const manualTimeRangeSelection = ref(MANUAL_ATTRIBUTION_AUTOMATIC)
const manualTimeRangeSaving = ref(false)
const manualTimeRangeError = ref<string | null>(null)
const manualTimeRangeResult = ref<number | null>(null)
const manualAttributionSelectionFor = (request: RequestRecord) => (
  request.attributionMethod === 'manual'
    ? request.attributionSourceId || MANUAL_ATTRIBUTION_UNATTRIBUTED
    : MANUAL_ATTRIBUTION_AUTOMATIC
)
const openDrawer = (request: RequestRecord) => {
  selectedRequest.value = request
  manualAttributionSelection.value = manualAttributionSelectionFor(request)
  manualAttributionError.value = false
  manualAttributionResult.value = null
  applyManualAttributionToSession.value = false
  drawerOpen.value = true
}
const closeDrawer = () => {
  drawerOpen.value = false
  selectedRequest.value = null
}

const manualAttributionOptions = computed(() => [
  { value: MANUAL_ATTRIBUTION_AUTOMATIC, label: t(locale.value, 'sessions.manualAttributionAutomatic') },
  { value: MANUAL_ATTRIBUTION_UNATTRIBUTED, label: t(locale.value, 'sessions.manualAttributionUnattributed') },
  ...store.settings.sourceAware.sources.map(source => ({
    value: source.id,
    label: source.displayName || source.baseUrl || source.id,
  })),
])

async function updateManualAttribution(event: Event) {
  const request = selectedRequest.value
  const selection = (event.target as HTMLSelectElement).value
  if (!request || manualAttributionSaving.value) return

  manualAttributionSaving.value = true
  manualAttributionError.value = false
  manualAttributionResult.value = null
  try {
    if (applyManualAttributionToSession.value && !window.confirm(
      t(locale.value, 'sessions.manualAttributionConfirmSession')
    )) {
      manualAttributionSelection.value = manualAttributionSelectionFor(request)
      return
    }
    const changed = applyManualAttributionToSession.value
      ? selection === MANUAL_ATTRIBUTION_AUTOMATIC
        ? await clearManualSessionAttribution(store.settings, request.sessionId)
        : await setManualSessionAttribution(
          store.settings,
          request.sessionId,
          selection === MANUAL_ATTRIBUTION_UNATTRIBUTED ? null : selection,
        )
      : selection === MANUAL_ATTRIBUTION_AUTOMATIC
        ? await clearManualRequestAttribution([request.requestKey])
        : await setManualRequestAttribution(
          store.settings,
          [request.requestKey],
          selection === MANUAL_ATTRIBUTION_UNATTRIBUTED ? null : selection,
        )
    manualAttributionResult.value = changed
    await reload()
    const refreshed = records.value.find(item => item.requestKey === request.requestKey)
    if (refreshed) {
      selectedRequest.value = refreshed
      manualAttributionSelection.value = manualAttributionSelectionFor(refreshed)
    } else {
      closeDrawer()
    }
  } catch {
    manualAttributionError.value = true
    manualAttributionSelection.value = manualAttributionSelectionFor(request)
  } finally {
    manualAttributionSaving.value = false
  }
}

function localDateTimeToEpoch(value: string): number | null {
  const epoch = new Date(value).getTime()
  return Number.isFinite(epoch) ? Math.floor(epoch / 1000) : null
}

function manualTimeRangeErrorKey(error: unknown) {
  const message = String(error)
  if (message.includes('ERR_MANUAL_ATTRIBUTION_INVALID_TIME_RANGE')) {
    return 'sessions.manualAttributionInvalidTimeRange'
  }
  if (message.includes('ERR_MANUAL_ATTRIBUTION_EMPTY_TIME_RANGE')) {
    return 'sessions.manualAttributionEmptyTimeRange'
  }
  if (message.includes('ERR_MANUAL_ATTRIBUTION_INVALID_TARGETS')) {
    return 'sessions.manualAttributionTimeRangeLimit'
  }
  return 'sessions.manualAttributionError'
}

async function updateManualTimeRangeAttribution() {
  const startEpoch = localDateTimeToEpoch(manualTimeRangeStart.value)
  const endEpoch = localDateTimeToEpoch(manualTimeRangeEnd.value)
  if (startEpoch === null || endEpoch === null || endEpoch <= startEpoch) {
    manualTimeRangeError.value = 'sessions.manualAttributionInvalidTimeRange'
    return
  }
  if (!window.confirm(t(locale.value, 'sessions.manualAttributionConfirmTimeRange'))) return

  manualTimeRangeSaving.value = true
  manualTimeRangeError.value = null
  manualTimeRangeResult.value = null
  try {
    const changed = manualTimeRangeSelection.value === MANUAL_ATTRIBUTION_AUTOMATIC
      ? await clearManualTimeRangeAttribution(store.settings, startEpoch, endEpoch)
      : await setManualTimeRangeAttribution(
        store.settings,
        startEpoch,
        endEpoch,
        manualTimeRangeSelection.value === MANUAL_ATTRIBUTION_UNATTRIBUTED
          ? null
          : manualTimeRangeSelection.value,
      )
    manualTimeRangeResult.value = changed
    await reload()
  } catch (error) {
    manualTimeRangeError.value = manualTimeRangeErrorKey(error)
  } finally {
    manualTimeRangeSaving.value = false
  }
}

const handleRowKeydown = (event: KeyboardEvent, request: RequestRecord) => {
  if (event.target !== event.currentTarget) return
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    openDrawer(request)
  }
}

const onEscape = (event: KeyboardEvent) => {
  if (event.key === 'Escape' && drawerOpen.value) closeDrawer()
}

const openInSession = () => {
  const sessionKey = selectedRequest.value?.sessionId
  closeDrawer()
  if (sessionKey) nav.openSession(sessionKey)
}

const toolOptions = computed(() => {
  const tools = new Set<string>()
  for (const session of store.sessions) tools.add(session.tool)
  for (const request of records.value) tools.add(request.tool)
  return [...tools].sort((a, b) => a.localeCompare(b))
})

const toolSelectOptions = computed<SelectOption[]>(() => [
  { value: null, label: t(locale.value, 'desktop.allTools') },
  ...toolOptions.value.map(tool => ({ value: tool, label: requestToolLabel(tool) })),
])

const statusSelectOptions = computed<SelectOption[]>(() => [
  { value: 'all', label: t(locale.value, 'desktop.sessions.requestFilterStatusAll') },
  { value: 'success', label: t(locale.value, 'common.success') },
  { value: 'error', label: t(locale.value, 'common.error') },
  { value: 'local', label: t(locale.value, 'desktop.sessions.requestFilterStatusLocal') },
])

const coverageSelectOptions = computed<SelectOption[]>(() => [
  { value: 'all', label: t(locale.value, 'desktop.sessions.requestFilterCoverageAll') },
  { value: 'proxy_only', label: t(locale.value, 'sessions.requestCoverageProxy') },
  { value: 'local_only', label: t(locale.value, 'sessions.requestCoverageLocal') },
  { value: 'merged', label: t(locale.value, 'sessions.requestCoverageMerged') },
])

const perfSelectOptions = computed<SelectOption[]>(() => [
  { value: 'all', label: t(locale.value, 'desktop.sessions.requestFilterPerfAll') },
  { value: 'has', label: t(locale.value, 'desktop.sessions.requestFilterPerfHas') },
  { value: 'none', label: t(locale.value, 'desktop.sessions.requestFilterPerfNone') },
])

const { copiedValue, copyText } = useClipboard()

const skeletonRows = [0, 1, 2, 3, 4, 5, 6, 7]

const PAGE_SIZE_OPTIONS = [20, 50, 100, 200]
const pageSizeSelectOptions = computed<SelectOption[]>(() =>
  PAGE_SIZE_OPTIONS.map(size => ({ value: size, label: `${size} / ${t(locale.value, 'desktop.requests.pageSize')}` })),
)

function onPageSizeUpdate(size: number) {
  pageSize.value = size
  reload()
}

onMounted(async () => {
  document.addEventListener('keydown', onEscape)
  await applyPendingFilters()
  if (store.sessions.length === 0) {
    await store.fetchSessionsForTool(selectedTool.value, 30, 0, false)
  }
  await initialize()
})

onUnmounted(() => {
  document.removeEventListener('keydown', onEscape)
  dispose()
})

function handleSortClick(col: { sortable: boolean; sortField?: RequestSortField }) {
  if (col.sortable && col.sortField) toggleSort(col.sortField)
}
</script>

<template>
  <div class="flex h-full flex-col gap-3">
    <!-- 工具栏 -->
    <div class="flex shrink-0 flex-wrap items-center gap-2 text-xs">
      <SearchInput v-model="searchInput" :placeholder="t(locale, 'desktop.requests.searchPlaceholder')" />
      <DesktopSelect
        v-model="selectedTool"
        :options="toolSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterTool')"
      />
      <DesktopSelect
        v-model="statusFilter"
        :options="statusSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.requestFilterStatus')"
      />
      <DesktopSelect
        v-model="coverageFilter"
        :options="coverageSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.requestFilterCoverage')"
      />
      <DesktopSelect
        v-model="perfFilter"
        :options="perfSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.requestFilterPerformance')"
      />
      <button
        v-if="hasActiveFilters"
        type="button"
        class="h-7 rounded-lg px-2 text-xs text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
        @click="resetFilters"
      >{{ t(locale, 'desktop.requests.clearFilters') }}</button>

      <div class="ml-auto flex items-center gap-2">
        <span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.requests.totalRecords', { count: total }) }}</span>
        <ColumnConfigPopover
          :columns="REQUEST_COLUMNS"
          :is-visible="isColumnVisible"
          :toggle-column="toggleColumn"
          :button-label="t(locale, 'desktop.requests.columnConfig')"
          :title-label="t(locale, 'desktop.requests.columnConfigTitle')"
          label-prefix="desktop.sessions.column"
          :locale="locale"
        />
      </div>
    </div>

    <details class="theme-surface shrink-0 rounded-xl border px-3 py-2">
      <summary class="cursor-pointer select-none text-xs font-medium text-[var(--theme-text-secondary)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]">
        {{ t(locale, 'sessions.manualAttributionTimeRange') }}
      </summary>
      <form class="mt-2 space-y-2" @submit.prevent="updateManualTimeRangeAttribution">
        <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
          <label class="min-w-0 text-xs text-[var(--theme-text-tertiary)]" for="manual-attribution-time-start">
            <span>{{ t(locale, 'sessions.manualAttributionTimeStart') }}</span>
            <input
              id="manual-attribution-time-start"
              v-model="manualTimeRangeStart"
              type="datetime-local"
              step="60"
              required
              :disabled="manualTimeRangeSaving"
              aria-describedby="manual-attribution-time-range-hint"
              class="mt-1 h-7 w-full rounded-md border border-[var(--theme-border-default)] bg-[var(--theme-bg-base)] px-1.5 text-xs text-[var(--theme-text-secondary)] outline-none transition-colors focus:border-[var(--theme-accent-primary)] disabled:cursor-wait disabled:opacity-60"
            >
          </label>
          <label class="min-w-0 text-xs text-[var(--theme-text-tertiary)]" for="manual-attribution-time-end">
            <span>{{ t(locale, 'sessions.manualAttributionTimeEnd') }}</span>
            <input
              id="manual-attribution-time-end"
              v-model="manualTimeRangeEnd"
              type="datetime-local"
              step="60"
              required
              :disabled="manualTimeRangeSaving"
              aria-describedby="manual-attribution-time-range-hint"
              class="mt-1 h-7 w-full rounded-md border border-[var(--theme-border-default)] bg-[var(--theme-bg-base)] px-1.5 text-xs text-[var(--theme-text-secondary)] outline-none transition-colors focus:border-[var(--theme-accent-primary)] disabled:cursor-wait disabled:opacity-60"
            >
          </label>
        </div>
        <p id="manual-attribution-time-range-hint" class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.manualAttributionTimeHint') }}</p>
        <div class="flex flex-wrap items-center justify-between gap-2">
          <label class="flex min-w-0 items-center gap-2 text-xs text-[var(--theme-text-tertiary)]" for="manual-attribution-time-source">
            <span class="shrink-0">{{ t(locale, 'sessions.manualAttribution') }}</span>
            <select
              id="manual-attribution-time-source"
              v-model="manualTimeRangeSelection"
              :disabled="manualTimeRangeSaving"
              class="min-w-0 max-w-52 rounded-md border border-[var(--theme-border-default)] bg-[var(--theme-bg-base)] px-1.5 py-1 text-xs text-[var(--theme-text-secondary)] outline-none transition-colors focus:border-[var(--theme-accent-primary)] disabled:cursor-wait disabled:opacity-60"
            >
              <option v-for="option in manualAttributionOptions" :key="option.value" :value="option.value">{{ option.label }}</option>
            </select>
          </label>
          <button
            type="submit"
            :disabled="manualTimeRangeSaving || !manualTimeRangeStart || !manualTimeRangeEnd"
            class="h-7 rounded-lg bg-[var(--theme-accent-primary)] px-2 text-xs font-medium text-white transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)] disabled:cursor-wait disabled:opacity-50"
          >{{ t(locale, 'sessions.manualAttributionApplyTimeRange') }}</button>
        </div>
        <p v-if="manualTimeRangeResult !== null" class="text-xs text-emerald-700 dark:text-emerald-300" role="status">{{ t(locale, 'sessions.manualAttributionUpdated', { count: manualTimeRangeResult }) }}</p>
        <p v-if="manualTimeRangeError" class="text-xs text-rose-600 dark:text-rose-300" role="alert">{{ t(locale, manualTimeRangeError) }}</p>
      </form>
    </details>

    <!-- 表格 + 覆盖式抽屉 -->
    <div class="relative min-h-0 flex-1">
      <div class="theme-surface h-full overflow-auto rounded-xl border">
        <table class="w-full min-w-[720px] border-collapse text-xs">
          <thead class="sticky top-0 z-10">
            <tr class="whitespace-nowrap border-b border-[var(--theme-border-default)] text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
              <th
                v-for="col in visibleCols"
                :key="col.key"
                class="select-none px-1.5 py-1.5"
                :class="[
                  col.align === 'right' ? 'text-right' : 'text-left',
                  col.sortable ? 'cursor-pointer hover:text-[var(--theme-text-primary)]' : '',
                ]"
                @click="handleSortClick(col)"
              >
                <span class="inline-flex items-center gap-0.5" :class="col.align === 'right' ? 'flex-row-reverse' : ''">
                  {{ t(locale, `desktop.sessions.column${col.key.charAt(0).toUpperCase()}${col.key.slice(1)}`) }}
                  <template v-if="col.sortable && isSortedBy(col.sortField!)">
                    <ArrowUp v-if="sortDir === 'asc'" class="h-2.5 w-2.5" aria-hidden="true" />
                    <ArrowDown v-else class="h-2.5 w-2.5" aria-hidden="true" />
                  </template>
                </span>
              </th>
            </tr>
          </thead>
          <tbody>
            <template v-if="loading && records.length === 0">
              <tr
                v-for="row in skeletonRows"
                :key="`rsk-${row}`"
                class="border-b border-[var(--theme-border-subtle)] last:border-0"
              >
                <td v-for="col in visibleCols" :key="col.key" class="px-1.5 py-2">
                  <div class="h-2.5 animate-pulse rounded bg-[var(--theme-border-default)]"></div>
                </td>
              </tr>
            </template>
            <tr v-else-if="records.length === 0">
              <td :colspan="visibleCols.length" class="px-3 py-12 text-center text-xs text-[var(--theme-text-tertiary)]">
                {{ hasActiveFilters ? t(locale, 'desktop.sessions.noMatch') : t(locale, 'desktop.sessions.noRequests') }}
              </td>
            </tr>
            <tr
              v-for="request in records"
              :key="request.requestKey"
              tabindex="0"
              class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
              :class="selectedRequest?.requestKey === request.requestKey && drawerOpen ? 'bg-[var(--theme-accent-soft)]' : ''"
              @click="openDrawer(request)"
              @keydown="handleRowKeydown($event, request)"
            >
              <td v-if="isColumnVisible('time')" class="whitespace-nowrap px-1.5 py-1.5 text-[var(--theme-text-secondary)]">{{ formatTime(request.timestampSec) }}</td>
              <td v-if="isColumnVisible('tool')" class="px-1.5 py-1.5">
                <span class="flex items-center gap-1">
                  <LobeIcon v-if="getToolIcon(request.tool)" :slug="getToolIcon(request.tool) ?? 'claudecode'" :size="12" @error="() => {}" />
                  <span v-else class="h-1.5 w-1.5 rounded-full bg-[var(--theme-border-strong)]"></span>
                  <span class="max-w-16 truncate text-[var(--theme-text-secondary)]">{{ requestToolLabel(request.tool) }}</span>
                </span>
              </td>
              <td v-if="isColumnVisible('source')" class="max-w-24 truncate px-1.5 py-1.5 text-[var(--theme-text-secondary)]" :title="requestSourceLabel(request)">{{ requestSourceLabel(request) }}</td>
              <td v-if="isColumnVisible('model')" class="max-w-28 truncate px-1.5 py-1.5 font-mono text-xs text-[var(--theme-text-secondary)]" :title="request.model">{{ requestModelLabel(request) }}</td>
              <td v-if="isColumnVisible('status')" class="px-1.5 py-1.5">
                <span v-if="request.coverageOrigin === 'local_only'" class="text-xs text-[var(--theme-text-quaternary)]">—</span>
                <StatusBadge v-else :label="requestStatusLabel(request)" :cls="requestStatusClasses(request)" />
              </td>
              <td v-if="isColumnVisible('input')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.inputTokens) }}</td>
              <td v-if="isColumnVisible('output')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.outputTokens) }}</td>
              <td v-if="isColumnVisible('cache')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(requestCacheTokens(request)) }}</td>
              <td v-if="isColumnVisible('totalTokens')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(request.totalTokens) }}</td>
              <td v-if="isColumnVisible('cost')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(request.estimatedCost) }}</td>
              <td v-if="isColumnVisible('ttft')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) ? formatDuration(request.ttftMs) : '—' }}</td>
              <td v-if="isColumnVisible('duration')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) ? formatDuration(request.durationMs) : '—' }}</td>
              <td v-if="isColumnVisible('rate')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) && request.outputTokensPerSecond ? `${request.outputTokensPerSecond.toFixed(1)}t/s` : '—' }}</td>
              <td v-if="isColumnVisible('session')" class="max-w-24 px-1.5 py-1.5 font-mono text-xs">
                <button
                  type="button"
                  tabindex="0"
                  class="block w-full cursor-pointer truncate text-left text-[var(--theme-text-tertiary)] transition-colors hover:text-[var(--theme-accent-primary)] focus-visible:text-[var(--theme-accent-primary)] focus-visible:outline-none"
                  :title="request.sessionId"
                  @keydown.stop
                  @click.stop="nav.openSession(request.sessionId)"
                >{{ shortId(request.sessionId) }}</button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <!-- 覆盖式抽屉 -->
      <DrawerShell width="w-[min(360px,42vw)]" :open="drawerOpen && !!selectedRequest" :aria-label="t(locale, 'desktop.sessions.drawerTitle')" @close="closeDrawer">
        <template v-if="selectedRequest">
            <div class="desktop-detail-drawer__header">
              <div class="min-w-0 flex-1">
                <div class="desktop-detail-drawer__meta">
                  <StatusBadge v-if="selectedRequest.coverageOrigin !== 'local_only'" :label="requestStatusLabel(selectedRequest)" :cls="requestStatusClasses(selectedRequest)" />
                  <span class="desktop-detail-drawer__dot" aria-hidden="true">•</span>
                  <span>{{ formatTime(selectedRequest.timestampSec) }}</span>
                </div>
                <h3 class="desktop-detail-drawer__title truncate">{{ requestModelLabel(selectedRequest) }}</h3>
                <p class="desktop-detail-drawer__subtitle truncate">{{ requestProjectLabel(selectedRequest) }} <span>/</span> {{ requestToolLabel(selectedRequest.tool) }} <span>/</span> {{ requestSourceLabel(selectedRequest) }}</p>
              </div>
              <div class="desktop-detail-drawer__header-actions">
                <button
                  type="button"
                  class="desktop-detail-drawer__open"
                  :title="t(locale, 'desktop.requests.openInSession')"
                  @click="openInSession"
                >
                  <ExternalLink class="h-3 w-3" aria-hidden="true" />
                  {{ t(locale, 'desktop.requests.openInSession') }}
                </button>
                <button
                  type="button"
                  class="desktop-detail-drawer__close"
                  :aria-label="t(locale, 'common.close')"
                  :title="t(locale, 'common.close')"
                  @click="closeDrawer"
                >
                  <X class="h-4 w-4 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                </button>
              </div>
            </div>

            <div class="desktop-detail-drawer__body">
              <div class="desktop-detail-drawer__metrics">
                <div class="desktop-detail-drawer__metric">
                  <span class="desktop-detail-drawer__metric-icon desktop-detail-drawer__metric-icon--blue"><FileText class="h-4 w-4" /></span>
                  <div class="desktop-detail-drawer__metric-copy"><span>{{ t(locale, 'common.totalTokens') }}</span><strong>{{ formatTokens(selectedRequest.totalTokens) }}</strong></div>
                </div>
                <div class="desktop-detail-drawer__metric">
                  <span class="desktop-detail-drawer__metric-icon desktop-detail-drawer__metric-icon--green"><DollarSign class="h-4 w-4" /></span>
                  <div class="desktop-detail-drawer__metric-copy"><span>{{ t(locale, 'sessions.cost') }}</span><strong class="desktop-detail-drawer__metric-value--cost">{{ formatCost(selectedRequest.estimatedCost) }}</strong></div>
                </div>
                <div class="desktop-detail-drawer__metric">
                  <span class="desktop-detail-drawer__metric-icon desktop-detail-drawer__metric-icon--blue"><Timer class="h-4 w-4" /></span>
                  <div class="desktop-detail-drawer__metric-copy"><span>{{ t(locale, 'sessions.duration') }}</span><strong>{{ formatDuration(selectedRequest.durationMs) }}</strong></div>
                </div>
                <div class="desktop-detail-drawer__metric">
                  <span class="desktop-detail-drawer__metric-icon desktop-detail-drawer__metric-icon--violet"><Database class="h-4 w-4" /></span>
                  <div class="desktop-detail-drawer__metric-copy"><span>{{ t(locale, 'statistics.cacheHitRate') }}</span><strong>{{ selectedRequest.totalTokens > 0 ? `${((selectedRequest.cacheReadTokens / selectedRequest.totalTokens) * 100).toFixed(1)}%` : '—' }}</strong></div>
                </div>
              </div>

              <section class="desktop-detail-drawer__section">
                <div class="desktop-detail-drawer__section-title"><span class="desktop-detail-drawer__section-icon desktop-detail-drawer__section-icon--blue"><FileText class="h-4 w-4" /></span><h4>{{ t(locale, 'sessions.requestTokenBreakdown') }}</h4></div>
                <div class="desktop-detail-drawer__rows">
                  <div><span><ArrowDownToLine class="h-3.5 w-3.5" />{{ t(locale, 'sessions.input') }}</span><strong>{{ formatTokens(selectedRequest.inputTokens) }}</strong></div>
                  <div><span><ArrowUpFromLine class="h-3.5 w-3.5" />{{ t(locale, 'sessions.output') }}</span><strong>{{ formatTokens(selectedRequest.outputTokens) }}</strong></div>
                  <div><span><Database class="h-3.5 w-3.5" />{{ t(locale, 'statistics.cacheCreate') }}</span><strong>{{ formatTokens(selectedRequest.cacheCreateTokens) }}</strong></div>
                  <div><span><Database class="h-3.5 w-3.5" />{{ t(locale, 'statistics.cacheRead') }}</span><strong>{{ formatTokens(selectedRequest.cacheReadTokens) }}</strong></div>
                </div>
                <p class="desktop-detail-drawer__note"><Info class="h-3.5 w-3.5" />{{ t(locale, 'sessions.requestCacheHitHint') }}</p>
              </section>

              <section class="desktop-detail-drawer__section">
                <div class="desktop-detail-drawer__section-title"><span class="desktop-detail-drawer__section-icon desktop-detail-drawer__section-icon--green"><BarChart3 class="h-4 w-4" /></span><h4>{{ t(locale, 'sessions.requestDetailsTitle') }}</h4></div>
                <div class="desktop-detail-drawer__detail-grid">
                  <div><span><Timer class="h-3.5 w-3.5" />{{ t(locale, 'sessions.ttft') }}</span><strong>{{ requestHasProxyPerformance(selectedRequest) ? formatDuration(selectedRequest.ttftMs) : '—' }}</strong></div>
                  <div><span><Zap class="h-3.5 w-3.5" />{{ t(locale, 'metrics.tokensPerSecond') }}</span><strong>{{ requestHasProxyPerformance(selectedRequest) && selectedRequest.outputTokensPerSecond ? `${selectedRequest.outputTokensPerSecond.toFixed(1)}t/s` : '—' }}</strong></div>
                  <div><span><Code2 class="h-3.5 w-3.5" />{{ t(locale, 'statistics.status') }}</span><strong>{{ selectedRequest.statusCode || '—' }}</strong></div>
                  <div><span><FileCheck2 class="h-3.5 w-3.5" />{{ t(locale, 'sessions.requestCoverage') }}</span><strong>{{ requestCoverageLabel(selectedRequest.coverageOrigin) }}</strong></div>
                </div>
              </section>

              <details class="desktop-detail-drawer__advanced">
                <summary><Activity class="h-3.5 w-3.5" />{{ t(locale, 'sessions.reconciliation') }}</summary>
                <div class="desktop-detail-drawer__advanced-grid">
                <div class="flex items-center justify-between gap-2 py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.reconciliationStatus') }}</span><span class="text-right text-xs font-medium text-[var(--theme-text-secondary)]">{{ requestReconciliationStatusLabel(selectedRequest) }}</span></div>
                <div class="flex items-center justify-between gap-2 py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.accountingRole') }}</span><span class="text-right text-xs font-medium text-[var(--theme-text-secondary)]">{{ requestAccountingRoleLabel(selectedRequest) }}</span></div>
                <div class="flex items-center justify-between gap-2 py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.reconciliationConfidence') }}</span><span class="text-right text-xs font-medium text-[var(--theme-text-secondary)]">{{ requestReconciliationConfidenceLabel(selectedRequest) }}</span></div>
                <div class="flex items-center justify-between gap-2 py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.observationSources') }}</span><span class="text-right text-xs font-medium text-[var(--theme-text-secondary)]">{{ requestObservationSourcesLabel(selectedRequest) }}</span></div>
                <div v-if="selectedRequest.reconciliationMethod" class="flex items-center justify-between gap-2 py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.reconciliationMethod') }}</span><span class="max-w-52 truncate text-right font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedRequest.reconciliationMethod">{{ selectedRequest.reconciliationMethod }}</span></div>
                <div v-if="selectedRequest.localObservationKey" class="flex items-center justify-between gap-2 py-1"><span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.localObservationKey') }}</span><span class="max-w-52 truncate text-right font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedRequest.localObservationKey">{{ requestObservationIdLabel(selectedRequest.localObservationKey) }}</span></div>
                <div v-if="selectedRequest.proxyObservationId" class="flex items-center justify-between gap-2 py-1"><span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.proxyObservationId') }}</span><span class="max-w-52 truncate text-right font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedRequest.proxyObservationId">{{ requestObservationIdLabel(selectedRequest.proxyObservationId) }}</span></div>
                </div>
              </details>

              <section class="desktop-detail-drawer__section desktop-detail-drawer__source-section">
                <div class="desktop-detail-drawer__section-title"><span class="desktop-detail-drawer__section-icon desktop-detail-drawer__section-icon--violet"><Link2 class="h-4 w-4" /></span><h4>{{ t(locale, 'sessions.requestSourceTitle') }}</h4></div>
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'common.source') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-xs text-[var(--theme-text-secondary)]">{{ requestSourceLabel(selectedRequest) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copySource')" :title="t(locale, 'desktop.sessions.copySource')" @click="copyText(requestSourceLabel(selectedRequest), 'source')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div v-if="requestAttributionLabel(selectedRequest)" class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.requestAttribution') }}</span>
                  <span class="text-xs font-medium text-[var(--theme-text-secondary)]">{{ requestAttributionLabel(selectedRequest) }}</span>
                </div>
                <div class="flex items-center justify-between gap-2 py-1">
                  <label class="shrink-0 text-xs text-[var(--theme-text-tertiary)]" for="manual-request-attribution">{{ t(locale, 'sessions.manualAttribution') }}</label>
                  <select
                    id="manual-request-attribution"
                    :value="manualAttributionSelection"
                    :disabled="manualAttributionSaving"
                    class="min-w-0 max-w-52 rounded-md border border-[var(--theme-border-default)] bg-[var(--theme-bg-base)] px-1.5 py-1 text-xs text-[var(--theme-text-secondary)] outline-none transition-colors focus:border-[var(--theme-accent-primary)] disabled:cursor-wait disabled:opacity-60"
                    @change="updateManualAttribution"
                  >
                    <option v-for="option in manualAttributionOptions" :key="option.value" :value="option.value">{{ option.label }}</option>
                  </select>
                </div>
                <label class="flex cursor-pointer items-center justify-end gap-1.5 py-1 text-xs text-[var(--theme-text-secondary)]" for="apply-manual-attribution-to-session">
                  <input
                    id="apply-manual-attribution-to-session"
                    v-model="applyManualAttributionToSession"
                    type="checkbox"
                    :disabled="manualAttributionSaving"
                    class="h-3.5 w-3.5 rounded border-[var(--theme-border-strong)] text-[var(--theme-accent-primary)] focus:ring-[var(--theme-accent-primary)] disabled:cursor-wait"
                  >
                  {{ t(locale, 'sessions.manualAttributionApplySession') }}
                </label>
                <p v-if="manualAttributionResult !== null" class="py-1 text-right text-xs text-emerald-700 dark:text-emerald-300" role="status">{{ t(locale, 'sessions.manualAttributionUpdated', { count: manualAttributionResult }) }}</p>
                <p v-if="manualAttributionError" class="py-1 text-right text-xs text-rose-600 dark:text-rose-300">{{ t(locale, 'sessions.manualAttributionError') }}</p>
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.sessionId') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedRequest.sessionId">{{ shortId(selectedRequest.sessionId) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copyId')" :title="t(locale, 'desktop.sessions.copyId')" @click="copyText(selectedRequest.sessionId, 'id')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.requestKey') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedRequest.requestKey">{{ shortId(selectedRequest.requestKey) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copyKey')" :title="t(locale, 'desktop.sessions.copyKey')" @click="copyText(selectedRequest.requestKey, 'key')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
              </section>

              <p v-if="copiedValue" class="text-center text-xs font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.sessions.copied') }}</p>
            </div>
        </template>
      </DrawerShell>
    </div>

    <!-- 分页控件 -->
    <PaginationBar
      :current-page="currentPage"
      :total-pages="totalPages"
      :total="total"
      :page-numbers="pageNumbers"
      :has-prev="hasPrev"
      :has-next="hasNext"
      :page-size="pageSize"
      :page-size-options="pageSizeSelectOptions"
      :loading="loading"
      :page-size-label="t(locale, 'desktop.requests.pageSize')"
      :page-first-label="t(locale, 'desktop.requests.pageFirst')"
      :page-prev-label="t(locale, 'desktop.requests.pagePrev')"
      :page-next-label="t(locale, 'desktop.requests.pageNext')"
      :page-last-label="t(locale, 'desktop.requests.pageLast')"
      :jump-to-label="t(locale, 'desktop.requests.jumpTo')"
      :page-of-label="t(locale, 'desktop.requests.pageOf', { current: '', total: totalPages })"
      @goto="gotoPage"
      @next="nextPage"
      @prev="prevPage"
      @update:page-size="onPageSizeUpdate"
    />
  </div>
</template>

<style scoped>
.desktop-detail-drawer__header {
  display: flex;
  flex-shrink: 0;
  align-items: flex-start;
  gap: 10px;
  border-bottom: 1px solid var(--theme-border-subtle);
  background: var(--theme-surface-gradient);
  padding: 16px 14px 14px;
}

.desktop-detail-drawer__meta {
  display: flex;
  align-items: center;
  gap: 7px;
  color: var(--theme-text-secondary);
  font-size: 11px;
  font-weight: 600;
}

.desktop-detail-drawer__dot {
  color: var(--theme-border-strong);
}

.desktop-detail-drawer__title {
  margin-top: 8px;
  color: var(--theme-text-primary);
  font-size: 20px;
  font-weight: 750;
  line-height: 1.15;
  letter-spacing: -0.02em;
}

.desktop-detail-drawer__subtitle {
  margin-top: 5px;
  color: var(--theme-text-secondary);
  font-size: 12px;
  font-weight: 600;
}

.desktop-detail-drawer__subtitle span {
  margin: 0 4px;
  color: var(--theme-border-strong);
}

.desktop-detail-drawer__header-actions {
  display: flex;
  flex-shrink: 0;
  align-items: flex-start;
  gap: 4px;
}

.desktop-detail-drawer__open {
  display: inline-flex;
  min-height: 32px;
  align-items: center;
  gap: 5px;
  border: 1px solid var(--theme-border-default);
  border-radius: 9px;
  padding: 0 8px;
  color: var(--theme-text-secondary);
  font-size: 11px;
  font-weight: 650;
  transition: border-color 150ms ease, color 150ms ease, background-color 150ms ease;
}

.desktop-detail-drawer__open:hover {
  border-color: var(--theme-accent-primary);
  color: var(--theme-accent-primary);
  background: var(--theme-accent-soft);
}

.desktop-detail-drawer__close {
  display: inline-flex;
  min-width: 40px;
  min-height: 40px;
  align-items: center;
  justify-content: center;
  border-radius: 10px;
  color: var(--theme-text-tertiary);
  transition: background-color 150ms ease, color 150ms ease;
}

.desktop-detail-drawer__close:hover {
  background: var(--theme-bg-hover);
  color: var(--theme-text-primary);
}

.desktop-detail-drawer__body {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  padding: 12px;
  scrollbar-width: thin;
  scrollbar-color: color-mix(in srgb, var(--theme-text-tertiary) 42%, transparent) transparent;
}

.desktop-detail-drawer__body::-webkit-scrollbar {
  width: 6px;
}

.desktop-detail-drawer__body::-webkit-scrollbar-thumb {
  border-radius: 999px;
  background: color-mix(in srgb, var(--theme-text-tertiary) 42%, transparent);
}

.desktop-detail-drawer__metrics {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.desktop-detail-drawer__metric {
  display: flex;
  min-width: 0;
  min-height: 48px;
  align-items: center;
  gap: 8px;
  border: 1px solid var(--theme-border-subtle);
  border-radius: 12px;
  background: var(--theme-bg-surface);
  padding: 7px 9px;
  box-shadow: var(--theme-shadow-inline);
}

.desktop-detail-drawer__metric-icon,
.desktop-detail-drawer__section-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 8px;
}

.desktop-detail-drawer__metric-icon {
  width: 26px;
  height: 26px;
  flex: 0 0 26px;
}

.desktop-detail-drawer__metric-copy {
  display: flex;
  min-width: 0;
  flex: 1;
  align-items: baseline;
  justify-content: space-between;
  gap: 5px;
}

.desktop-detail-drawer__metric-icon--blue,
.desktop-detail-drawer__section-icon--blue {
  color: #1769e0;
  background: rgb(59 130 246 / 11%);
}

.desktop-detail-drawer__metric-icon--green,
.desktop-detail-drawer__section-icon--green {
  color: #07885f;
  background: rgb(16 185 129 / 11%);
}

.desktop-detail-drawer__metric-icon--violet,
.desktop-detail-drawer__section-icon--violet {
  color: #7c3aed;
  background: rgb(124 58 237 / 10%);
}

.desktop-detail-drawer__metric-copy > span {
  min-width: 0;
  overflow: hidden;
  color: var(--theme-text-secondary);
  font-size: 10px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.desktop-detail-drawer__metric-copy strong {
  flex-shrink: 0;
  overflow: hidden;
  color: var(--theme-text-primary);
  font-family: var(--font-mono);
  font-size: 13px;
  font-variant-numeric: tabular-nums;
  font-weight: 750;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.desktop-detail-drawer__metric-value--cost {
  color: var(--theme-chart-cost) !important;
}

.desktop-detail-drawer__section {
  margin-top: 10px;
  border: 1px solid var(--theme-border-subtle);
  border-radius: 15px;
  background: var(--theme-bg-surface);
  padding: 11px 12px;
  box-shadow: var(--theme-shadow-inline);
}

.desktop-detail-drawer__section-title {
  display: flex;
  align-items: center;
  gap: 7px;
}

.desktop-detail-drawer__section-icon {
  width: 26px;
  height: 26px;
}

.desktop-detail-drawer__section-title h4 {
  color: var(--theme-text-primary);
  font-size: 14px;
  font-weight: 750;
  letter-spacing: -0.015em;
}

.desktop-detail-drawer__rows {
  margin-top: 7px;
  overflow: hidden;
  border-radius: 10px;
  background: var(--theme-bg-surface-muted);
}

.desktop-detail-drawer__rows > div,
.desktop-detail-drawer__detail-grid > div {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-height: 29px;
  border-bottom: 1px solid var(--theme-border-default);
  color: var(--theme-text-secondary);
  font-size: 11px;
}

.desktop-detail-drawer__rows > div:last-child,
.desktop-detail-drawer__detail-grid > div:nth-last-child(-n + 2) {
  border-bottom: 0;
}

.desktop-detail-drawer__rows > div {
  padding: 3px 9px;
}

.desktop-detail-drawer__rows span,
.desktop-detail-drawer__detail-grid span {
  display: inline-flex;
  min-width: 0;
  align-items: center;
  gap: 6px;
}

.desktop-detail-drawer__rows span svg,
.desktop-detail-drawer__detail-grid span svg {
  flex-shrink: 0;
  color: var(--theme-accent-primary);
}

.desktop-detail-drawer__rows strong,
.desktop-detail-drawer__detail-grid strong {
  color: var(--theme-text-primary);
  font-family: var(--font-mono);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  font-weight: 650;
  text-align: right;
}

.desktop-detail-drawer__note {
  display: flex;
  align-items: flex-start;
  gap: 5px;
  margin-top: 7px;
  color: var(--theme-text-tertiary);
  font-size: 10px;
  line-height: 1.4;
}

.desktop-detail-drawer__note svg {
  flex-shrink: 0;
  margin-top: 1px;
}

.desktop-detail-drawer__detail-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  column-gap: 12px;
  margin-top: 5px;
}

.desktop-detail-drawer__detail-grid > div {
  min-height: 33px;
  padding: 4px 0;
}

.desktop-detail-drawer__detail-grid > div:nth-child(odd) {
  padding-right: 8px;
}

.desktop-detail-drawer__detail-grid > div:nth-child(even) {
  padding-left: 8px;
  border-left: 1px solid var(--theme-border-default);
}

.desktop-detail-drawer__advanced {
  margin-top: 10px;
  border: 1px solid var(--theme-border-default);
  border-radius: 13px;
  background: var(--theme-bg-surface-muted);
  padding: 9px 10px;
}

.desktop-detail-drawer__advanced summary {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  list-style: none;
  color: var(--theme-text-secondary);
  font-size: 11px;
  font-weight: 700;
}

.desktop-detail-drawer__advanced summary::-webkit-details-marker {
  display: none;
}

.desktop-detail-drawer__advanced-grid {
  display: grid;
  grid-template-columns: 1fr;
  gap: 2px;
  margin-top: 7px;
}

.desktop-detail-drawer__advanced-grid > div {
  min-width: 0;
  border-bottom: 1px solid var(--theme-border-default);
}

.desktop-detail-drawer__advanced-grid > div:last-child {
  border-bottom: 0;
}

.desktop-detail-drawer__source-section {
  margin-bottom: 2px;
}
</style>
