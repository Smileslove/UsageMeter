/**
 * 会话表格 composable：封装搜索、多维筛选、排序、客户端分页、列配置、末页续载。
 * 对标 useRequestTable，但分页策略为客户端（slice 已加载列表）而非服务端。
 */
import { computed, reactive, ref, watch, type Ref } from 'vue'
import type { SessionStats } from '../../types'
import type { useMonitorStore } from '../../stores/monitor'
import { useSessionViewData } from '../../composables/useSessionViewData'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useColumnConfig, type ColumnDef } from './useColumnConfig'
import type { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { t } from '../../i18n'

export type SessionColumnKey = 'session' | 'lastActive' | 'model' | 'requests' | 'tokens' | 'cost' | 'rate' | 'errors' | 'duration'
export type SessionSortKey = 'lastActive' | 'firstRequest' | 'requests' | 'tokens' | 'cost' | 'rate' | 'errors'
export type CoverageFilter = 'all' | 'full' | 'partial' | 'uncovered'

export interface SessionColumnDef extends ColumnDef<SessionColumnKey> {
  align: 'left' | 'right'
  sortable: boolean
  sortField?: SessionSortKey
  labelKey: string
}

export const SESSION_COLUMNS: SessionColumnDef[] = [
  { key: 'session', align: 'left', sortable: true, sortField: 'lastActive', labelKey: 'desktop.sessions.columnSession', defaultVisible: true },
  { key: 'lastActive', align: 'left', sortable: true, sortField: 'lastActive', labelKey: 'desktop.sessions.columnLastActive', defaultVisible: true },
  { key: 'model', align: 'left', sortable: false, labelKey: 'desktop.sessions.columnModel', defaultVisible: true },
  { key: 'requests', align: 'right', sortable: true, sortField: 'requests', labelKey: 'desktop.sessions.columnRequests', defaultVisible: true },
  { key: 'tokens', align: 'right', sortable: true, sortField: 'tokens', labelKey: 'desktop.sessions.columnTokens', defaultVisible: true },
  { key: 'cost', align: 'right', sortable: true, sortField: 'cost', labelKey: 'desktop.sessions.columnCost', defaultVisible: true },
  { key: 'rate', align: 'right', sortable: true, sortField: 'rate', labelKey: 'desktop.sessions.columnAvgRate', defaultVisible: true },
  { key: 'errors', align: 'right', sortable: true, sortField: 'errors', labelKey: 'desktop.sessions.columnErrors', defaultVisible: false },
  { key: 'duration', align: 'right', sortable: true, sortField: 'firstRequest', labelKey: 'desktop.sessions.columnDuration', defaultVisible: false },
]

const COLUMN_STORAGE_KEY = 'usagemeter.sessionColumns'
const PAGE_SIZE_OPTIONS = [20, 50, 100, 200]

export function sessionTotalTokens(session: SessionStats): number {
  return (session.totalInputTokens || 0) + (session.totalOutputTokens || 0) + (session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)
}

interface LocalFilters {
  time: string
  customRange: { startEpoch: number; endEpoch: number } | null
  project: string | null
  model: string | null
  coverage: CoverageFilter
}

export function useSessionTable(
  store: ReturnType<typeof useMonitorStore>,
  nav: ReturnType<typeof useDesktopNavigationStore>,
  locale: Ref<string>,
) {
  const { selectedTool, hasMore, loadingMore, loadMore, initialize: initializeSessionView, dispose: disposeSessionView } = useSessionViewData(store, ref('recent'))

  const { displaySessionTitle } = useSessionDisplay(store)

  const searchQuery = ref('')

  const filters = reactive<LocalFilters>({
    time: 'all',
    customRange: null,
    project: null,
    model: null,
    coverage: 'all',
  })

  const sortKey = ref<SessionSortKey>('lastActive')
  const sortDesc = ref(true)

  const currentPage = ref(0)
  const pageSize = ref(50)

  const { visibleColumns, toggleColumn, isColumnVisible, visibleCols } =
    useColumnConfig(COLUMN_STORAGE_KEY, SESSION_COLUMNS, 1)

  function sessionCoverageKind(session: SessionStats): CoverageFilter {
    if (session.usageFullyCovered) return 'full'
    if ((session.uncoveredRequests ?? 0) > 0) return 'uncovered'
    if ((session.coveredRequests ?? 0) > 0) return 'partial'
    return 'full'
  }

  function sortValue(session: SessionStats, key: SessionSortKey): number {
    switch (key) {
      case 'lastActive': return session.lastRequestTime || 0
      case 'firstRequest': return session.firstRequestTime || 0
      case 'requests': return session.totalRequests || 0
      case 'tokens': return sessionTotalTokens(session)
      case 'cost': return session.estimatedCost ?? 0
      case 'rate': return session.avgOutputTokensPerSecond || 0
      case 'errors': return session.errorRequests ?? 0
    }
  }

  function matchesSearch(session: SessionStats): boolean {
    const q = searchQuery.value.trim().toLowerCase()
    if (!q) return true
    return [
      displaySessionTitle(session),
      session.topic,
      session.lastPrompt,
      session.projectName,
      session.cwd,
      session.sessionId,
    ].some(value => value?.toLowerCase().includes(q))
  }

  function matchesTime(session: SessionStats): boolean {
    if (filters.time === 'all') return true
    if (filters.time === 'custom') {
      const range = filters.customRange
      if (!range) return true
      const ts = session.lastRequestTime || 0
      return ts >= range.startEpoch && ts < range.endEpoch
    }
    const now = Date.now()
    const horizon = filters.time === 'today' ? 24 * 3600e3 : filters.time === '7d' ? 7 * 24 * 3600e3 : 30 * 24 * 3600e3
    return (session.lastRequestTime || 0) * 1000 >= now - horizon
  }

  function matchesFilters(session: SessionStats): boolean {
    if (filters.project === '__global__' && session.projectIdentity !== 'global') return false
    if (filters.project === '__unknown__' && session.projectIdentity !== 'unknown') return false
    if (filters.project && filters.project !== '__global__' && filters.project !== '__unknown__' && session.projectName !== filters.project) return false
    if (filters.model && session.models[0] !== filters.model) return false
    if (filters.coverage !== 'all' && sessionCoverageKind(session) !== filters.coverage) return false
    return true
  }

  const filteredSessions = computed(() => {
    const list = store.sessions.filter(session => matchesSearch(session) && matchesTime(session) && matchesFilters(session))
    return [...list].sort((a, b) => {
      const diff = sortValue(a, sortKey.value) - sortValue(b, sortKey.value)
      return sortDesc.value ? -diff : diff
    })
  })

  const total = computed(() => filteredSessions.value.length)
  const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize.value)))
  const paginatedSessions = computed(() => {
    const start = currentPage.value * pageSize.value
    return filteredSessions.value.slice(start, start + pageSize.value)
  })
  const hasPrev = computed(() => currentPage.value > 0)
  const hasNext = computed(() => currentPage.value < totalPages.value - 1)

  const pageNumbers = computed<(number | '...')[]>(() => {
    const tp = totalPages.value
    const cur = currentPage.value + 1
    if (tp <= 7) return Array.from({ length: tp }, (_, i) => i + 1)
    if (cur <= 4) return [1, 2, 3, 4, 5, '...', tp]
    if (cur >= tp - 3) return [1, '...', tp - 4, tp - 3, tp - 2, tp - 1, tp]
    return [1, '...', cur - 1, cur, cur + 1, '...', tp]
  })

  function gotoPage(page: number) {
    currentPage.value = Math.max(0, Math.min(page, totalPages.value - 1))
  }
  function nextPage() {
    if (hasNext.value) currentPage.value++
  }
  function prevPage() {
    if (hasPrev.value) currentPage.value--
  }

  function cycleSort(key: SessionSortKey) {
    if (sortKey.value === key) {
      sortDesc.value = !sortDesc.value
    } else {
      sortKey.value = key
      sortDesc.value = true
    }
  }

  function isSortedBy(field: SessionSortKey) {
    return sortKey.value === field
  }

  const pageSizeSelectOptions = computed(() =>
    PAGE_SIZE_OPTIONS.map(size => ({
      value: size,
      label: `${size} / ${t(locale.value, 'desktop.requests.pageSize')}`,
    })),
  )

  function onPageSizeChange() {
    currentPage.value = 0
  }

  function onTimeChange(value: string | number | null) {
    if (value === 'custom' && !filters.customRange) return
    filters.time = String(value)
  }

  function clearAllFilters() {
    filters.time = 'all'
    filters.customRange = null
    filters.project = null
    filters.model = null
    filters.coverage = 'all'
    selectedTool.value = null
    searchQuery.value = ''
  }

  const hasActiveFilters = computed(() =>
    searchQuery.value.trim() !== '' ||
    filters.time !== 'all' ||
    filters.project !== null ||
    filters.model !== null ||
    filters.coverage !== 'all' ||
    selectedTool.value !== null,
  )

  const projectOptions = computed(() => {
    const names = new Set<string>()
    for (const session of store.sessions) {
      if (session.projectIdentity === 'global') names.add('__global__')
      else if (session.projectIdentity === 'unknown') names.add('__unknown__')
      else if (session.projectName?.trim()) names.add(session.projectName.trim())
    }
    return [...names].sort((a, b) => a.localeCompare(b))
  })

  const modelOptions = computed(() => {
    const models = new Set<string>()
    for (const session of store.sessions) {
      if (session.models[0]) models.add(session.models[0])
    }
    return [...models].sort((a, b) => a.localeCompare(b))
  })

  const toolOptions = computed(() => {
    const tools = new Set<string>()
    for (const session of store.sessions) tools.add(session.tool)
    return [...tools].sort((a, b) => a.localeCompare(b))
  })

  async function applyPendingFilters() {
    const pending = nav.consumePendingFilters()
    if (!pending) return
    if (pending.sourceId && store.settings.sourceAware.activeSourceFilter !== pending.sourceId) {
      await store.setActiveSourceFilter(pending.sourceId)
    }
    if (pending.tool && store.settings.clientTools.activeToolFilter !== pending.tool) {
      await store.setActiveToolFilter(pending.tool)
    }
    if (pending.timeRange) {
      filters.time = 'custom'
      filters.customRange = pending.timeRange
    }
    if (pending.sessionKey && !nav.activeSessionKey) {
      nav.openSession(pending.sessionKey)
    }
  }

  watch([searchQuery, () => filters.time, () => filters.project, () => filters.model, () => filters.coverage, () => filters.customRange, selectedTool], () => {
    currentPage.value = 0
  })

  watch([currentPage, totalPages, hasMore, loadingMore], async () => {
    if (hasMore.value && !loadingMore.value && currentPage.value >= totalPages.value - 1 && total.value > 0) {
      await loadMore()
    }
  })

  watch(() => nav.pendingConsumeTick, () => {
    if (nav.currentPage === 'sessions') void applyPendingFilters()
  })

  return {
    selectedTool,
    hasMore,
    loadingMore,
    loadMore,
    initializeSessionView,
    disposeSessionView,
    searchQuery,
    filters,
    sortKey,
    sortDesc,
    currentPage,
    pageSize,
    pageSizeSelectOptions,
    visibleColumns,
    toggleColumn,
    isColumnVisible,
    visibleCols,
    filteredSessions,
    paginatedSessions,
    total,
    totalPages,
    pageNumbers,
    hasPrev,
    hasNext,
    hasActiveFilters,
    projectOptions,
    modelOptions,
    toolOptions,
    gotoPage,
    nextPage,
    prevPage,
    cycleSort,
    isSortedBy,
    onPageSizeChange,
    onTimeChange,
    clearAllFilters,
    applyPendingFilters,
  }
}
