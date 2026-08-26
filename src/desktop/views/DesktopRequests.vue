<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { ArrowDown, ArrowUp, ChevronsLeft, ChevronsRight, ChevronLeft, ChevronRight, Copy, ExternalLink, Search, Settings2, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t } from '../../i18n'
import type { RequestRecord, RequestSortField } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useClipboard } from '../composables/useClipboard'
import { useRequestTable, REQUEST_COLUMNS } from '../composables/useRequestTable'
import { normalizeSessionTool } from '../../composables/useSessionViewData'
import LobeIcon from '../../components/LobeIcon.vue'

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
const openDrawer = (request: RequestRecord) => {
  selectedRequest.value = request
  drawerOpen.value = true
}
const closeDrawer = () => {
  drawerOpen.value = false
  selectedRequest.value = null
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

const showColumnConfig = ref(false)

const toolOptions = computed(() => {
  const tools = new Set<string>()
  for (const session of store.sessions) tools.add(session.tool)
  for (const request of records.value) tools.add(request.tool)
  return [...tools].sort((a, b) => a.localeCompare(b))
})

const shortId = (value: string) => (value.length > 12 ? `${value.slice(0, 6)}…${value.slice(-4)}` : value)
const { copiedValue, copyText } = useClipboard()

const skeletonRows = [0, 1, 2, 3, 4, 5, 6, 7]

const pageStart = computed(() => total.value === 0 ? 0 : currentPage.value * pageSize.value + 1)
const pageEnd = computed(() => Math.min((currentPage.value + 1) * pageSize.value, total.value))

const jumpPageInput = ref('')
const jumpPage = () => {
  const num = parseInt(jumpPageInput.value, 10)
  if (Number.isFinite(num) && num >= 1 && num <= totalPages.value) {
    gotoPage(num - 1)
  }
  jumpPageInput.value = ''
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

function gotoPageNumber(num: number | '...') {
  if (num === '...') return
  gotoPage(num - 1)
}
</script>

<template>
  <div class="flex h-full flex-col gap-3">
    <!-- 工具栏 -->
    <div class="flex shrink-0 flex-wrap items-center gap-2 text-[11px]">
      <div class="relative">
        <Search class="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <input
          v-model="searchInput"
          type="text"
          class="theme-input h-7 w-44 rounded-lg pl-7 pr-2 text-[11px] outline-none"
          :placeholder="t(locale, 'desktop.requests.searchPlaceholder')"
          :aria-label="t(locale, 'desktop.requests.searchPlaceholder')"
        />
      </div>
      <select
        v-model="selectedTool"
        class="theme-input h-7 rounded-lg px-2 text-[11px] outline-none"
        :aria-label="t(locale, 'desktop.sessions.filterTool')"
      >
        <option :value="null">{{ t(locale, 'desktop.allTools') }}</option>
        <option v-for="tool in toolOptions" :key="tool" :value="tool">{{ requestToolLabel(tool) }}</option>
      </select>
      <select
        v-model="statusFilter"
        class="theme-input h-7 rounded-lg px-2 text-[11px] outline-none"
        :aria-label="t(locale, 'desktop.sessions.requestFilterStatus')"
      >
        <option value="all">{{ t(locale, 'desktop.sessions.requestFilterStatusAll') }}</option>
        <option value="success">{{ t(locale, 'common.success') }}</option>
        <option value="error">{{ t(locale, 'common.error') }}</option>
        <option value="local">{{ t(locale, 'desktop.sessions.requestFilterStatusLocal') }}</option>
      </select>
      <select
        v-model="coverageFilter"
        class="theme-input h-7 rounded-lg px-2 text-[11px] outline-none"
        :aria-label="t(locale, 'desktop.sessions.requestFilterCoverage')"
      >
        <option value="all">{{ t(locale, 'desktop.sessions.requestFilterCoverageAll') }}</option>
        <option value="proxy_only">{{ t(locale, 'sessions.requestCoverageProxy') }}</option>
        <option value="local_only">{{ t(locale, 'sessions.requestCoverageLocal') }}</option>
        <option value="merged">{{ t(locale, 'sessions.requestCoverageMerged') }}</option>
      </select>
      <select
        v-model="perfFilter"
        class="theme-input h-7 rounded-lg px-2 text-[11px] outline-none"
        :aria-label="t(locale, 'desktop.sessions.requestFilterPerformance')"
      >
        <option value="all">{{ t(locale, 'desktop.sessions.requestFilterPerfAll') }}</option>
        <option value="has">{{ t(locale, 'desktop.sessions.requestFilterPerfHas') }}</option>
        <option value="none">{{ t(locale, 'desktop.sessions.requestFilterPerfNone') }}</option>
      </select>
      <button
        v-if="hasActiveFilters"
        type="button"
        class="h-7 rounded-lg px-2 text-[10.5px] text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
        @click="resetFilters"
      >{{ t(locale, 'desktop.requests.clearFilters') }}</button>

      <div class="ml-auto flex items-center gap-2">
        <span class="text-[10.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.requests.totalRecords', { count: total }) }}</span>
        <div class="relative">
          <button
            type="button"
            class="inline-flex h-7 items-center gap-1 rounded-lg border border-[var(--theme-border-default)] px-2 text-[10.5px] font-medium text-[var(--theme-text-secondary)] transition-colors hover:border-[var(--theme-accent-primary)] hover:text-[var(--theme-accent-primary)]"
            :aria-label="t(locale, 'desktop.requests.columnConfig')"
            :title="t(locale, 'desktop.requests.columnConfig')"
            @click="showColumnConfig = !showColumnConfig"
          >
            <Settings2 class="h-3 w-3" aria-hidden="true" />
            <span class="hidden sm:inline">{{ t(locale, 'desktop.requests.columnConfig') }}</span>
          </button>
          <Transition name="popover">
            <div
              v-if="showColumnConfig"
              class="theme-surface-elevated absolute right-0 top-8 z-30 w-40 rounded-xl border p-2 shadow-lg"
              :aria-label="t(locale, 'desktop.requests.columnConfigTitle')"
            >
              <p class="mb-1.5 px-1 text-[9.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.requests.columnConfigTitle') }}</p>
              <label
                v-for="col in REQUEST_COLUMNS"
                :key="col.key"
                class="flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 text-[11px] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
              >
                <input
                  type="checkbox"
                  class="h-3 w-3 accent-[var(--theme-accent-primary)]"
                  :checked="isColumnVisible(col.key)"
                  @change="toggleColumn(col.key)"
                />
                <span>{{ t(locale, `desktop.sessions.column${col.key.charAt(0).toUpperCase()}${col.key.slice(1)}`) }}</span>
              </label>
            </div>
          </Transition>
        </div>
      </div>
    </div>

    <!-- 表格 + 覆盖式抽屉 -->
    <div class="relative min-h-0 flex-1">
      <div class="theme-surface h-full overflow-auto rounded-xl border">
        <table class="w-full min-w-[720px] border-collapse text-[11px]">
          <thead class="sticky top-0 z-10">
            <tr class="border-b border-[var(--theme-border-default)] text-[9.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
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
              <td :colspan="visibleCols.length" class="px-3 py-12 text-center text-[11.5px] text-[var(--theme-text-tertiary)]">
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
              <td v-if="isColumnVisible('model')" class="max-w-28 truncate px-1.5 py-1.5 font-mono text-[10px] text-[var(--theme-text-secondary)]" :title="request.model">{{ requestModelLabel(request) }}</td>
              <td v-if="isColumnVisible('status')" class="px-1.5 py-1.5">
                <span v-if="request.coverageOrigin === 'local_only'" class="text-[10px] text-[var(--theme-text-quaternary)]">—</span>
                <span v-else class="inline-flex items-center rounded-full border px-1.5 py-px text-[9px] font-bold leading-none" :class="requestStatusClasses(request)">{{ requestStatusLabel(request) }}</span>
              </td>
              <td v-if="isColumnVisible('input')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.inputTokens) }}</td>
              <td v-if="isColumnVisible('output')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.outputTokens) }}</td>
              <td v-if="isColumnVisible('cache')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(requestCacheTokens(request)) }}</td>
              <td v-if="isColumnVisible('totalTokens')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(request.totalTokens) }}</td>
              <td v-if="isColumnVisible('cost')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(request.estimatedCost) }}</td>
              <td v-if="isColumnVisible('ttft')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) ? formatDuration(request.ttftMs) : '—' }}</td>
              <td v-if="isColumnVisible('duration')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) ? formatDuration(request.durationMs) : '—' }}</td>
              <td v-if="isColumnVisible('rate')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) && request.outputTokensPerSecond ? `${request.outputTokensPerSecond.toFixed(1)}t/s` : '—' }}</td>
              <td v-if="isColumnVisible('session')" class="max-w-24 px-1.5 py-1.5 font-mono text-[10px]">
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
      <Transition name="drawer-overlay">
        <div v-if="drawerOpen" class="absolute inset-0 z-20 flex justify-end" @click.self="closeDrawer">
          <aside
            class="theme-surface-elevated flex h-full w-80 flex-col overflow-y-auto rounded-xl border shadow-[0_4px_24px_rgba(0,0,0,0.08)]"
            :aria-label="t(locale, 'desktop.sessions.drawerTitle')"
          >
            <div class="flex items-start justify-between gap-2 border-b border-[var(--theme-border-default)] px-3 py-2.5">
              <div class="min-w-0">
                <div class="mb-1 flex items-center gap-1.5">
                  <span v-if="selectedRequest!.coverageOrigin !== 'local_only'" class="inline-flex items-center rounded-full border px-1.5 py-px text-[9.5px] font-bold leading-none" :class="requestStatusClasses(selectedRequest!)">{{ requestStatusLabel(selectedRequest!) }}</span>
                  <span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ formatTime(selectedRequest!.timestampSec) }}</span>
                </div>
                <h3 class="truncate text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ requestModelLabel(selectedRequest!) }}</h3>
                <p class="mt-0.5 truncate text-[10px] text-[var(--theme-text-tertiary)]">{{ requestProjectLabel(selectedRequest!) }} / {{ requestToolLabel(selectedRequest!.tool) }} / {{ requestSourceLabel(selectedRequest!) }}</p>
              </div>
              <div class="flex shrink-0 items-start gap-1">
                <button
                  type="button"
                  class="inline-flex items-center gap-1 rounded-lg border border-[var(--theme-border-default)] px-2 py-1 text-[10px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:border-[var(--theme-accent-primary)] hover:text-[var(--theme-accent-primary)]"
                  :title="t(locale, 'desktop.requests.openInSession')"
                  @click="openInSession"
                >
                  <ExternalLink class="h-3 w-3" aria-hidden="true" />
                  {{ t(locale, 'desktop.requests.openInSession') }}
                </button>
                <button
                  type="button"
                  class="shrink-0 rounded-lg p-1 transition-colors hover:bg-[var(--theme-bg-hover)]"
                  :aria-label="t(locale, 'common.close')"
                  :title="t(locale, 'common.close')"
                  @click="closeDrawer"
                >
                  <X class="h-4 w-4 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                </button>
              </div>
            </div>

            <div class="space-y-3 p-3">
              <div class="grid grid-cols-3 gap-2">
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-[9.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'common.totalTokens') }}</div>
                  <div class="font-mono text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest!.totalTokens) }}</div>
                </div>
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-[9.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.cost') }}</div>
                  <div class="font-mono text-[12px] font-semibold text-[var(--theme-chart-cost)]">{{ formatCost(selectedRequest!.estimatedCost) }}</div>
                </div>
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-[9.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.duration') }}</div>
                  <div class="font-mono text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ formatDuration(selectedRequest!.durationMs) }}</div>
                </div>
              </div>

              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.input') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest!.inputTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.output') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest!.outputTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.cacheCreate') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest!.cacheCreateTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.cacheRead') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest!.cacheReadTokens) }}</span></div>
              </section>

              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.ttft') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ requestHasProxyPerformance(selectedRequest!) ? formatDuration(selectedRequest!.ttftMs) : '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'metrics.tokensPerSecond') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ requestHasProxyPerformance(selectedRequest!) && selectedRequest!.outputTokensPerSecond ? `${selectedRequest!.outputTokensPerSecond.toFixed(1)}t/s` : '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.status') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ selectedRequest!.statusCode || '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.requestCoverage') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ requestCoverageLabel(selectedRequest!.coverageOrigin) }}</span></div>
              </section>

              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'common.source') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-[10.5px] text-[var(--theme-text-secondary)]">{{ requestSourceLabel(selectedRequest!) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copySource')" :title="t(locale, 'desktop.sessions.copySource')" @click="copyText(requestSourceLabel(selectedRequest!), 'source')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.sessionId') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-[10.5px] text-[var(--theme-text-secondary)]" :title="selectedRequest!.sessionId">{{ shortId(selectedRequest!.sessionId) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copyId')" :title="t(locale, 'desktop.sessions.copyId')" @click="copyText(selectedRequest!.sessionId, 'id')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.requestKey') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-[10.5px] text-[var(--theme-text-secondary)]" :title="selectedRequest!.requestKey">{{ shortId(selectedRequest!.requestKey) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copyKey')" :title="t(locale, 'desktop.sessions.copyKey')" @click="copyText(selectedRequest!.requestKey, 'key')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
              </section>

              <p v-if="copiedValue" class="text-center text-[10px] font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.sessions.copied') }}</p>
            </div>
          </aside>
        </div>
      </Transition>
    </div>

    <!-- 分页控件 -->
    <div class="flex shrink-0 items-center justify-between gap-3 text-[10.5px] text-[var(--theme-text-tertiary)]">
      <span class="shrink-0">{{ pageStart }}–{{ pageEnd }} / {{ total }}</span>
      <div class="flex items-center gap-2">
        <div class="flex items-center gap-1">
          <button
            type="button"
            class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!hasPrev || loading"
            :aria-label="t(locale, 'desktop.requests.pageFirst')"
            :title="t(locale, 'desktop.requests.pageFirst')"
            @click="gotoPage(0)"
          >
            <ChevronsLeft class="h-3.5 w-3.5" aria-hidden="true" />
          </button>
          <button
            type="button"
            class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!hasPrev || loading"
            :aria-label="t(locale, 'desktop.requests.pagePrev')"
            @click="prevPage"
          >
            <ChevronLeft class="h-3.5 w-3.5" aria-hidden="true" />
          </button>
          <template v-for="(num, idx) in pageNumbers" :key="idx">
            <span v-if="num === '...'" class="px-1 text-[var(--theme-text-quaternary)]">…</span>
            <button
              v-else
              type="button"
              class="inline-flex h-7 min-w-[28px] items-center justify-center rounded-lg border px-1.5 font-mono text-[10.5px] transition-colors"
              :class="num === currentPage + 1
                ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-soft)] font-semibold text-[var(--theme-accent-primary)]'
                : 'border-[var(--theme-border-default)] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]'"
              :disabled="loading"
              :aria-current="num === currentPage + 1 ? 'page' : undefined"
              @click="gotoPageNumber(num)"
            >{{ num }}</button>
          </template>
          <button
            type="button"
            class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!hasNext || loading"
            :aria-label="t(locale, 'desktop.requests.pageNext')"
            @click="nextPage"
          >
            <ChevronRight class="h-3.5 w-3.5" aria-hidden="true" />
          </button>
          <button
            type="button"
            class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!hasNext || loading"
            :aria-label="t(locale, 'desktop.requests.pageLast')"
            :title="t(locale, 'desktop.requests.pageLast')"
            @click="gotoPage(totalPages - 1)"
          >
            <ChevronsRight class="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>
        <div v-if="totalPages > 7" class="flex items-center gap-1">
          <span class="text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.requests.jumpTo') }}</span>
          <input
            v-model="jumpPageInput"
            type="number"
            min="1"
            :max="totalPages"
            class="theme-input h-7 w-12 rounded-lg px-1 text-center font-mono text-[10.5px] outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
            :aria-label="t(locale, 'desktop.requests.jumpTo')"
            @keydown.enter="jumpPage"
          />
          <span class="text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.requests.pageOf', { current: '', total: totalPages }) }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.popover-enter-active,
.popover-leave-active {
  transition: opacity 0.12s ease-out, transform 0.12s ease-out;
}
.popover-enter-from,
.popover-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

.drawer-overlay-enter-active,
.drawer-overlay-leave-active {
  transition: opacity 0.18s ease-out;
}
.drawer-overlay-enter-from,
.drawer-overlay-leave-to {
  opacity: 0;
}
.drawer-overlay-enter-active aside,
.drawer-overlay-leave-active aside {
  transition: transform 0.18s ease-out;
}
.drawer-overlay-enter-from aside,
.drawer-overlay-leave-to aside {
  transform: translateX(16px);
}
</style>
