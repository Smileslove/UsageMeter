import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { computed, ref, watch, type Ref } from 'vue'
import type { RequestSortDir, RequestSortField } from '../../types'
import type { useMonitorStore } from '../../stores/monitor'
import { normalizeSessionTool, SESSION_SOURCE_TOOLS } from '../../composables/useSessionViewData'
import { useColumnConfig, type ColumnDef } from './useColumnConfig'

export { SESSION_SOURCE_TOOLS }

export type RequestColumnKey =
  | 'time'
  | 'tool'
  | 'source'
  | 'model'
  | 'status'
  | 'input'
  | 'output'
  | 'cache'
  | 'totalTokens'
  | 'cost'
  | 'ttft'
  | 'duration'
  | 'rate'
  | 'session'

export interface RequestColumnDef extends ColumnDef<RequestColumnKey> {
  align: 'left' | 'right'
  sortable: boolean
  sortField?: RequestSortField
}

export const REQUEST_COLUMNS: RequestColumnDef[] = [
  { key: 'time', align: 'left', sortable: true, sortField: 'timestamp', defaultVisible: true },
  { key: 'tool', align: 'left', sortable: false, defaultVisible: true },
  { key: 'source', align: 'left', sortable: false, defaultVisible: false },
  { key: 'model', align: 'left', sortable: false, defaultVisible: true },
  { key: 'status', align: 'left', sortable: false, defaultVisible: true },
  { key: 'input', align: 'right', sortable: true, sortField: 'input', defaultVisible: true },
  { key: 'output', align: 'right', sortable: true, sortField: 'output', defaultVisible: true },
  { key: 'cache', align: 'right', sortable: false, defaultVisible: false },
  { key: 'totalTokens', align: 'right', sortable: true, sortField: 'totalTokens', defaultVisible: true },
  { key: 'cost', align: 'right', sortable: true, sortField: 'cost', defaultVisible: true },
  { key: 'ttft', align: 'right', sortable: true, sortField: 'ttft', defaultVisible: false },
  { key: 'duration', align: 'right', sortable: true, sortField: 'duration', defaultVisible: true },
  { key: 'rate', align: 'right', sortable: true, sortField: 'rate', defaultVisible: true },
  { key: 'session', align: 'left', sortable: false, defaultVisible: true },
]

const STORAGE_KEY = 'usagemeter.requestColumns'
const SEARCH_DEBOUNCE_MS = 300
const REFRESH_DEBOUNCE_MS = 800

export function useRequestTable(
  store: ReturnType<typeof useMonitorStore>,
  selectedTool: Ref<string | null>
) {
  const currentPage = ref(0)
  const pageSize = ref(50)
  const searchInput = ref('')
  const searchQuery = ref('')
  const statusFilter = ref<string>('all')
  const coverageFilter = ref<string>('all')
  const perfFilter = ref<string>('all')
  const sortField = ref<RequestSortField>('timestamp')
  const sortDir = ref<RequestSortDir>('desc')
  const { visibleColumns, toggleColumn, isColumnVisible } = useColumnConfig(STORAGE_KEY, REQUEST_COLUMNS, 3)

  const lastGlobalTool = ref<string | null>(
    normalizeSessionTool(store.settings.clientTools.activeToolFilter)
  )
  const lastProxyRecordCount = ref<number | null>(null)
  let searchTimer: ReturnType<typeof setTimeout> | null = null
  let refreshTimer: ReturnType<typeof setTimeout> | null = null
  let unlistenLocalUsageSynced: UnlistenFn | null = null

  const totalPages = computed(() =>
    Math.max(1, Math.ceil(store.requestTotal / pageSize.value))
  )
  const loading = computed(() => store.requestRecordsLoading)
  const hasPrev = computed(() => currentPage.value > 0)
  const hasNext = computed(() =>
    currentPage.value < totalPages.value - 1 || store.requestHasMore
  )

  const pageNumbers = computed<(number | '...')[]>(() => {
    const total = totalPages.value
    const cur = currentPage.value + 1
    if (total <= 7) return Array.from({ length: total }, (_, i) => i + 1)
    if (cur <= 4) return [1, 2, 3, 4, 5, '...', total]
    if (cur >= total - 3) return [1, '...', total - 4, total - 3, total - 2, total - 1, total]
    return [1, '...', cur - 1, cur, cur + 1, '...', total]
  })

  const sortableColumns = computed(() =>
    REQUEST_COLUMNS.filter(c => c.sortable)
  )

  async function loadCurrentPage() {
    const params = {
      limit: pageSize.value,
      offset: currentPage.value * pageSize.value,
      search: searchQuery.value.trim() || null,
      status: statusFilter.value !== 'all' ? statusFilter.value : null,
      coverage: coverageFilter.value !== 'all' ? coverageFilter.value : null,
      performance: perfFilter.value !== 'all' ? perfFilter.value : null,
      sortField: sortField.value,
      sortDir: sortDir.value,
    }
    await store.fetchRequestRecordsPage(selectedTool.value, params)
  }

  async function reload() {
    currentPage.value = 0
    await loadCurrentPage()
  }

  async function gotoPage(page: number) {
    const clamped = Math.max(0, Math.min(page, totalPages.value - 1))
    if (clamped === currentPage.value && store.requestRecords.length > 0) return
    currentPage.value = clamped
    await loadCurrentPage()
  }

  async function nextPage() {
    if (!hasNext.value) return
    currentPage.value++
    await loadCurrentPage()
  }

  async function prevPage() {
    if (!hasPrev.value) return
    currentPage.value--
    await loadCurrentPage()
  }

  function toggleSort(field: RequestSortField) {
    if (sortField.value === field) {
      sortDir.value = sortDir.value === 'asc' ? 'desc' : 'asc'
    } else {
      sortField.value = field
      sortDir.value = 'desc'
    }
  }

  function isSortedBy(field: RequestSortField) {
    return sortField.value === field
  }

  function resetFilters() {
    searchInput.value = ''
    searchQuery.value = ''
    statusFilter.value = 'all'
    coverageFilter.value = 'all'
    perfFilter.value = 'all'
  }

  function clearSort() {
    sortField.value = 'timestamp'
    sortDir.value = 'desc'
  }

  const hasActiveFilters = computed(() =>
    searchQuery.value.trim() !== '' ||
    statusFilter.value !== 'all' ||
    coverageFilter.value !== 'all' ||
    perfFilter.value !== 'all'
  )

  function scheduleRefresh() {
    if (refreshTimer) clearTimeout(refreshTimer)
    refreshTimer = setTimeout(() => {
      refreshTimer = null
      void loadCurrentPage()
    }, REFRESH_DEBOUNCE_MS)
  }

  async function setupListeners() {
    try {
      unlistenLocalUsageSynced = await listen('local_usage_synced', () => scheduleRefresh())
    } catch (err) {
      console.warn('[useRequestTable] listen local_usage_synced failed', err)
    }
  }

  watch(searchInput, value => {
    if (searchTimer) clearTimeout(searchTimer)
    searchTimer = setTimeout(() => {
      searchTimer = null
      const next = value.trim()
      if (next === searchQuery.value) return
      searchQuery.value = next
      void reload()
    }, SEARCH_DEBOUNCE_MS)
  })

  watch([statusFilter, coverageFilter, perfFilter, sortField, sortDir], () => {
    void reload()
  })

  watch(selectedTool, () => {
    void reload()
  })

  watch(() => store.settings.clientTools.activeToolFilter, globalTool => {
    const normalized = normalizeSessionTool(globalTool)
    if (selectedTool.value === lastGlobalTool.value) selectedTool.value = normalized
    lastGlobalTool.value = normalized
  })

  watch(() => store.sessionViewsRevision, () => {
    void reload()
  })

  watch(() => store.proxyStatus?.recordCount ?? null, (current, previous) => {
    if (current === null || previous === null || lastProxyRecordCount.value === null) {
      lastProxyRecordCount.value = current
      return
    }
    if (current <= lastProxyRecordCount.value) {
      lastProxyRecordCount.value = current
      return
    }
    lastProxyRecordCount.value = current
    scheduleRefresh()
  })

  async function initialize() {
    await setupListeners()
    await loadCurrentPage()
  }

  function dispose() {
    unlistenLocalUsageSynced?.()
    unlistenLocalUsageSynced = null
    if (searchTimer) {
      clearTimeout(searchTimer)
      searchTimer = null
    }
    if (refreshTimer) {
      clearTimeout(refreshTimer)
      refreshTimer = null
    }
  }

  return {
    currentPage,
    pageSize,
    searchInput,
    statusFilter,
    coverageFilter,
    perfFilter,
    sortField,
    sortDir,
    visibleColumns,
    totalPages,
    pageNumbers,
    loading,
    hasPrev,
    hasNext,
    hasActiveFilters,
    sortableColumns,
    records: computed(() => store.requestRecords),
    total: computed(() => store.requestTotal),
    reload,
    gotoPage,
    nextPage,
    prevPage,
    toggleSort,
    isSortedBy,
    toggleColumn,
    isColumnVisible,
    resetFilters,
    clearSort,
    initialize,
    dispose,
  }
}
