<script setup lang="ts">
/**
 * 桌面主窗口「会话」页。
 * 会话表格 + 覆盖式抽屉（点击行展开详情，不跳转页面）。
 * 客户端分页：对已加载的 filteredSessions 做内存分页，到达末页时自动续载。
 */
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { ArrowDown, ArrowUp, ChevronsLeft, ChevronsRight, ChevronLeft, ChevronRight, ExternalLink, Search, Settings2, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { t } from '../../i18n'
import type { SessionStats } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useSessionViewData } from '../../composables/useSessionViewData'
import { useClipboard } from '../composables/useClipboard'
import { useFocusTrap } from '../composables/useFocusTrap'
import LobeIcon from '../../components/LobeIcon.vue'
import DesktopSelect from '../components/DesktopSelect.vue'
import type { SelectOption } from '../components/DesktopSelect.vue'

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

const {
  formatTime,
  formatTokens,
  formatCost,
  formatDuration,
  sessionModelLabel,
  requestToolLabel,
  sessionUsageVisible,
  displaySessionTitle,
  displaySessionProjectBadge,
  projectBadgeClasses,
  getToolIcon,
} = useSessionDisplay(store)

// —— 数据加载 ——
const activeTab = ref<'recent'>('recent')
const {
  selectedTool,
  hasMore,
  loadingMore,
  loadMore,
  initialize: initializeSessionView,
  dispose: disposeSessionView,
} = useSessionViewData(store, activeTab)

// —— 搜索 ——
const searchQuery = ref('')

// —— 筛选 ——
type CoverageFilter = 'all' | 'full' | 'partial' | 'uncovered'
interface LocalFilters {
  time: string
  customRange: { startEpoch: number; endEpoch: number } | null
  project: string | null
  model: string | null
  coverage: CoverageFilter
}
const filters = reactive<LocalFilters>({ time: 'all', customRange: null, project: null, model: null, coverage: 'all' })

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
const projectFilterLabel = (value: string | null) => {
  if (!value) return ''
  if (value === '__global__') return t(locale.value, 'common.global')
  if (value === '__unknown__') return t(locale.value, 'common.unknownProject')
  return value
}

const projectSelectOptions = computed<SelectOption[]>(() => [
  { value: null, label: t(locale.value, 'desktop.sessions.filterAllProjects') },
  ...projectOptions.value.map(p => ({ value: p, label: projectFilterLabel(p) }))
])

const modelSelectOptions = computed<SelectOption[]>(() => [
  { value: null, label: t(locale.value, 'desktop.sessions.filterAllModels') },
  ...modelOptions.value.map(m => ({ value: m, label: m }))
])

const timeSelectOptions = computed<SelectOption[]>(() => [
  { value: 'all', label: t(locale.value, 'desktop.sessions.filterTimeAll') },
  { value: 'today', label: t(locale.value, 'desktop.sessions.filterTimeToday') },
  { value: '7d', label: t(locale.value, 'desktop.sessions.filterTime7d') },
  { value: '30d', label: t(locale.value, 'desktop.sessions.filterTime30d') },
  { value: 'custom', label: t(locale.value, 'desktop.sessions.filterTimeCustom') },
])

const coverageSelectOptions = computed<SelectOption[]>(() => [
  { value: 'all', label: t(locale.value, 'desktop.sessions.filterCoverageAll') },
  { value: 'full', label: t(locale.value, 'desktop.sessions.filterCoverageFull') },
  { value: 'partial', label: t(locale.value, 'desktop.sessions.filterCoveragePartial') },
  { value: 'uncovered', label: t(locale.value, 'desktop.sessions.filterCoverageUncovered') },
])

interface Chip {
  id: string
  label: string
  remove: () => void
}
const chips = computed<Chip[]>(() => {
  const result: Chip[] = []
  const timeLabels: Record<string, string> = {
    today: t(locale.value, 'desktop.sessions.filterTimeToday'),
    '7d': t(locale.value, 'desktop.sessions.filterTime7d'),
    '30d': t(locale.value, 'desktop.sessions.filterTime30d'),
    custom: t(locale.value, 'desktop.sessions.filterTimeCustom')
  }
  if (filters.time !== 'all') {
    result.push({
      id: 'time',
      label: timeLabels[filters.time],
      remove: () => {
        filters.time = 'all'
        filters.customRange = null
      }
    })
  }
  if (selectedTool.value) {
    result.push({ id: 'tool', label: requestToolLabel(selectedTool.value), remove: () => { selectedTool.value = null } })
  }
  if (filters.project) {
    result.push({ id: 'project', label: projectFilterLabel(filters.project), remove: () => { filters.project = null } })
  }
  if (filters.model) {
    result.push({ id: 'model', label: filters.model, remove: () => { filters.model = null } })
  }
  if (filters.coverage !== 'all') {
    const coverageLabels: Record<CoverageFilter, string> = {
      all: '',
      full: t(locale.value, 'desktop.sessions.filterCoverageFull'),
      partial: t(locale.value, 'desktop.sessions.filterCoveragePartial'),
      uncovered: t(locale.value, 'desktop.sessions.filterCoverageUncovered')
    }
    result.push({ id: 'coverage', label: coverageLabels[filters.coverage], remove: () => { filters.coverage = 'all' } })
  }
  return result
})
const visibleChips = computed(() => chips.value.slice(0, 4))
const extraChipsCount = computed(() => Math.max(0, chips.value.length - 4))
const chipsCollapsed = ref(true)

const clearAllFilters = () => {
  filters.time = 'all'
  filters.customRange = null
  filters.project = null
  filters.model = null
  filters.coverage = 'all'
  selectedTool.value = null
  searchQuery.value = ''
}

// —— 排序 ——
type SortKey = 'lastActive' | 'firstRequest' | 'requests' | 'tokens' | 'cost' | 'rate' | 'errors'
const sortKey = ref<SortKey>('lastActive')
const sortDesc = ref(true)

const sessionTotalTokens = (session: SessionStats) =>
  (session.totalInputTokens || 0) + (session.totalOutputTokens || 0) + (session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)

const sortValue = (session: SessionStats, key: SortKey): number => {
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

const matchesSearch = (session: SessionStats): boolean => {
  const q = searchQuery.value.trim().toLowerCase()
  if (!q) return true
  return [
    displaySessionTitle(session),
    session.topic,
    session.lastPrompt,
    session.projectName,
    session.cwd,
    session.sessionId
  ].some(value => value?.toLowerCase().includes(q))
}

const matchesTime = (session: SessionStats): boolean => {
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

const onTimeChange = (value: string | number | null) => {
  if (value === 'custom' && !filters.customRange) return
  filters.time = value as string
}

const sessionCoverageKind = (session: SessionStats): CoverageFilter => {
  if (session.usageFullyCovered) return 'full'
  if ((session.uncoveredRequests ?? 0) > 0) return 'uncovered'
  if ((session.coveredRequests ?? 0) > 0) return 'partial'
  return 'full'
}

const matchesFilters = (session: SessionStats): boolean => {
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

const cycleSort = (key: SortKey) => {
  if (sortKey.value === key) {
    sortDesc.value = !sortDesc.value
  } else {
    sortKey.value = key
    sortDesc.value = true
  }
}

const sortIndicator = (key: SortKey) => (sortKey.value === key ? (sortDesc.value ? ArrowDown : ArrowUp) : null)

// —— 分页（客户端） ——
const currentPage = ref(0)
const pageSize = ref(50)

const total = computed(() => filteredSessions.value.length)
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize.value)))

const paginatedSessions = computed(() => {
  const start = currentPage.value * pageSize.value
  return filteredSessions.value.slice(start, start + pageSize.value)
})

const pageStart = computed(() => total.value === 0 ? 0 : currentPage.value * pageSize.value + 1)
const pageEnd = computed(() => Math.min((currentPage.value + 1) * pageSize.value, total.value))

const PAGE_SIZE_OPTIONS = [20, 50, 100, 200]
const pageSizeSelectOptions = computed<SelectOption[]>(() =>
  PAGE_SIZE_OPTIONS.map(size => ({ value: size, label: `${size} / ${t(locale.value, 'desktop.requests.pageSize')}` }))
)
const onPageSizeChange = () => {
  currentPage.value = 0
}

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

const gotoPage = (page: number) => {
  const clamped = Math.max(0, Math.min(page, totalPages.value - 1))
  currentPage.value = clamped
}

const nextPage = () => {
  if (!hasNext.value) return
  currentPage.value++
}

const prevPage = () => {
  if (!hasPrev.value) return
  currentPage.value--
}

function gotoPageNumber(num: number | '...') {
  if (num === '...') return
  gotoPage(num - 1)
}

const jumpPageInput = ref('')
const jumpPage = () => {
  const num = parseInt(jumpPageInput.value, 10)
  if (Number.isFinite(num) && num >= 1 && num <= totalPages.value) {
    gotoPage(num - 1)
  }
  jumpPageInput.value = ''
}

// 筛选/搜索变化时回到第一页
watch([searchQuery, () => filters.time, () => filters.project, () => filters.model, () => filters.coverage, () => filters.customRange, selectedTool], () => {
  currentPage.value = 0
})

// 到达末页且还有更多数据时自动续载
watch([currentPage, totalPages, hasMore, loadingMore], async () => {
  if (hasMore.value && !loadingMore.value && currentPage.value >= totalPages.value - 1 && total.value > 0) {
    await loadMore()
  }
})

// —— 抽屉 ——
const selectedSession = ref<SessionStats | null>(null)
const drawerOpen = ref(false)

const openDrawer = (session: SessionStats) => {
  selectedSession.value = session
  drawerOpen.value = true
}
const closeDrawer = () => {
  drawerOpen.value = false
  selectedSession.value = null
}

const onEscape = (event: KeyboardEvent) => {
  if (event.key === 'Escape' && drawerOpen.value) closeDrawer()
}

const handleRowKeydown = (event: KeyboardEvent, session: SessionStats) => {
  if (event.target !== event.currentTarget) return
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    openDrawer(session)
  }
}

const openInWorkspace = () => {
  const sessionKey = selectedSession.value?.sessionId
  closeDrawer()
  if (sessionKey) nav.openSession(sessionKey)
}

const shortId = (value: string) => (value.length > 12 ? `${value.slice(0, 6)}…${value.slice(-4)}` : value)
const { copiedValue, copyText } = useClipboard()

// —— 右键菜单 ——
interface ContextMenuState {
  session: SessionStats
  x: number
  y: number
}
const contextMenu = ref<ContextMenuState | null>(null)
const contextMenuOpen = computed(() => contextMenu.value !== null)
const contextMenuRef = ref<HTMLElement | null>(null)

const openContextMenu = (clientX: number, clientY: number, session: SessionStats) => {
  const MENU_WIDTH = 176
  const MENU_HEIGHT = 104
  const margin = 8
  const x = Math.max(margin, Math.min(clientX, window.innerWidth - MENU_WIDTH - margin))
  const y = Math.max(margin, Math.min(clientY, window.innerHeight - MENU_HEIGHT - margin))
  contextMenu.value = { session, x, y }
}
const closeContextMenu = () => { contextMenu.value = null }
useFocusTrap({ open: contextMenuOpen, container: contextMenuRef, onClose: closeContextMenu })

const handleRowContextMenu = (event: MouseEvent, session: SessionStats) => {
  openContextMenu(event.clientX, event.clientY, session)
}
const handleRowMenuKeydown = (event: KeyboardEvent, session: SessionStats) => {
  if ((event.shiftKey && event.key === 'F10') || event.key === 'ContextMenu') {
    event.preventDefault()
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
    openContextMenu(rect.left + 32, rect.bottom - 8, session)
  }
}
const handleMenuKeydown = (event: KeyboardEvent) => {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
  event.preventDefault()
  const items = Array.from(contextMenuRef.value?.querySelectorAll<HTMLElement>('button[role="menuitem"]') ?? []).filter(
    el => !(el as HTMLButtonElement).disabled
  )
  if (items.length === 0) return
  const activeIndex = items.indexOf(document.activeElement as HTMLElement)
  const next = event.key === 'ArrowDown' ? activeIndex + 1 : activeIndex - 1
  items[(next + items.length) % items.length].focus()
}
const runContextAction = (action: 'open' | 'copyId' | 'copyCwd') => {
  const state = contextMenu.value
  if (!state) return
  const { session } = state
  closeContextMenu()
  if (action === 'open') {
    openDrawer(session)
  } else if (action === 'copyId') {
    void copyText(session.sessionId, t(locale.value, 'desktop.sessions.menuCopyId'))
  } else if (action === 'copyCwd' && session.cwd) {
    void copyText(session.cwd, t(locale.value, 'desktop.sessions.menuCopyCwd'))
  }
}

const closeMenuOnPointerDown = (event: PointerEvent) => {
  if (contextMenuRef.value?.contains(event.target as Node)) return
  closeContextMenu()
}
const closeMenuOnScroll = () => { closeContextMenu() }
watch(contextMenuOpen, isOpen => {
  if (isOpen) {
    document.addEventListener('pointerdown', closeMenuOnPointerDown, true)
    window.addEventListener('scroll', closeMenuOnScroll, true)
    window.addEventListener('resize', closeMenuOnScroll)
  } else {
    document.removeEventListener('pointerdown', closeMenuOnPointerDown, true)
    window.removeEventListener('scroll', closeMenuOnScroll, true)
    window.removeEventListener('resize', closeMenuOnScroll)
  }
})

// —— 列配置（localStorage 持久化） ——
type SessionColumnKey = 'session' | 'lastActive' | 'model' | 'requests' | 'tokens' | 'cost' | 'rate' | 'errors' | 'duration'

interface SessionColumnDef {
  key: SessionColumnKey
  defaultVisible: boolean
}

const SESSION_COLUMNS: SessionColumnDef[] = [
  { key: 'session', defaultVisible: true },
  { key: 'lastActive', defaultVisible: true },
  { key: 'model', defaultVisible: true },
  { key: 'requests', defaultVisible: true },
  { key: 'tokens', defaultVisible: true },
  { key: 'cost', defaultVisible: true },
  { key: 'rate', defaultVisible: true },
  { key: 'errors', defaultVisible: false },
  { key: 'duration', defaultVisible: false },
]

const COLUMN_STORAGE_KEY = 'usagemeter.sessionColumns'

function loadVisibleColumns(): Set<SessionColumnKey> {
  try {
    const saved = localStorage.getItem(COLUMN_STORAGE_KEY)
    if (saved) {
      const keys = JSON.parse(saved) as string[]
      const valid = SESSION_COLUMNS.map(c => c.key)
      const filtered = keys.filter((k): k is SessionColumnKey => valid.includes(k as SessionColumnKey))
      if (filtered.length > 0) return new Set(filtered)
    }
  } catch { /* ignore */ }
  return new Set(SESSION_COLUMNS.filter(c => c.defaultVisible).map(c => c.key))
}

function saveVisibleColumns(cols: Set<SessionColumnKey>) {
  try {
    localStorage.setItem(COLUMN_STORAGE_KEY, JSON.stringify([...cols]))
  } catch { /* ignore */ }
}

const visibleColumns = ref<Set<SessionColumnKey>>(loadVisibleColumns())
const showColumnConfig = ref(false)

const isColumnVisible = (key: SessionColumnKey) => visibleColumns.value.has(key)

function toggleColumn(key: SessionColumnKey) {
  const next = new Set(visibleColumns.value)
  if (next.has(key)) {
    if (next.size > 1) next.delete(key)
  } else {
    next.add(key)
  }
  visibleColumns.value = next
  saveVisibleColumns(next)
}

const visibleCols = computed(() => SESSION_COLUMNS.filter(c => isColumnVisible(c.key)))

// —— 深链筛选 ——
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

watch(
  () => nav.pendingConsumeTick,
  () => {
    if (nav.currentPage === 'sessions') void applyPendingFilters()
  }
)

// —— 工具选项 ——
const toolOptions = computed(() => {
  const tools = new Set<string>()
  for (const session of store.sessions) tools.add(session.tool)
  return [...tools].sort((a, b) => a.localeCompare(b))
})

const toolSelectOptions = computed<SelectOption[]>(() => [
  { value: null, label: t(locale.value, 'desktop.allTools') },
  ...toolOptions.value.map(tool => ({ value: tool, label: requestToolLabel(tool) }))
])

const skeletonRows = [0, 1, 2, 3, 4, 5, 6, 7]

onMounted(async () => {
  document.addEventListener('keydown', onEscape)
  await applyPendingFilters()
  await initializeSessionView()
})

onUnmounted(() => {
  document.removeEventListener('keydown', onEscape)
  disposeSessionView()
  document.removeEventListener('pointerdown', closeMenuOnPointerDown, true)
  window.removeEventListener('scroll', closeMenuOnScroll, true)
  window.removeEventListener('resize', closeMenuOnScroll)
})
</script>

<template>
  <div class="flex h-full flex-col gap-3">
    <!-- 工具栏 -->
    <div class="flex shrink-0 flex-wrap items-center gap-2 text-xs">
      <div class="relative">
        <Search class="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <input
          v-model="searchQuery"
          type="text"
          class="theme-input h-7 w-44 rounded-lg pl-7 pr-2 text-xs outline-none"
          :placeholder="t(locale, 'desktop.sessions.searchPlaceholder')"
          :aria-label="t(locale, 'desktop.sessions.searchPlaceholder')"
        />
      </div>

      <DesktopSelect
        v-model="filters.time"
        :options="timeSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterTime')"
        @change="onTimeChange"
      />

      <DesktopSelect
        v-model="filters.project"
        :options="projectSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterProject')"
      />

      <DesktopSelect
        v-model="filters.model"
        :options="modelSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterModel')"
      />

      <DesktopSelect
        v-model="filters.coverage"
        :options="coverageSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterCoverage')"
      />

      <DesktopSelect
        v-model="selectedTool"
        :options="toolSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterTool')"
      />

      <button
        v-if="chips.length > 0"
        type="button"
        class="h-7 rounded-lg px-2 text-xs text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
        @click="clearAllFilters"
      >
        {{ t(locale, 'desktop.requests.clearFilters') }}
      </button>

      <div class="ml-auto flex items-center gap-2">
        <span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.requests.totalRecords', { count: total }) }}</span>
        <div class="relative">
          <button
            type="button"
            class="inline-flex h-7 items-center gap-1 rounded-lg border border-[var(--theme-border-default)] px-2 text-xs font-medium text-[var(--theme-text-secondary)] transition-colors hover:border-[var(--theme-accent-primary)] hover:text-[var(--theme-accent-primary)]"
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
              <p class="mb-1.5 px-1 text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.requests.columnConfigTitle') }}</p>
              <label
                v-for="col in SESSION_COLUMNS"
                :key="col.key"
                class="flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 text-xs text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
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

    <!-- 筛选 chips -->
    <div v-if="chips.length > 0" class="flex shrink-0 flex-wrap items-center gap-1.5 text-xs">
      <span v-for="chip in (chipsCollapsed ? visibleChips : chips)" :key="chip.id" class="inline-flex items-center gap-1 rounded-full border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-2 py-0.5 font-medium text-[var(--theme-text-secondary)]">
        {{ chip.label }}
        <button type="button" class="rounded-full p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.removeFilter')" :title="t(locale, 'desktop.sessions.removeFilter')" @click="chip.remove()">
          <X class="h-3 w-3" aria-hidden="true" />
        </button>
      </span>
      <button v-if="extraChipsCount > 0" type="button" class="rounded-full border border-[var(--theme-border-default)] px-2 py-0.5 font-medium text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]" @click="chipsCollapsed = !chipsCollapsed">
        {{ chipsCollapsed ? t(locale, 'desktop.sessions.chipsMore', { count: extraChipsCount }) : t(locale, 'desktop.sessions.chipsLess') }}
      </button>
    </div>

    <!-- 表格 + 覆盖式抽屉 -->
    <div class="relative min-h-0 flex-1">
      <div class="theme-surface h-full overflow-auto rounded-xl border">
        <table class="w-full min-w-[860px] border-collapse text-xs">
          <thead class="sticky top-0 z-10">
            <tr class="whitespace-nowrap border-b border-[var(--theme-border-default)] text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
              <th v-if="isColumnVisible('session')" class="min-w-60 px-1.5 py-1.5 text-left font-semibold">
                <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('lastActive')">
                  {{ t(locale, 'desktop.sessions.columnSession') }}
                  <component :is="sortIndicator('lastActive')" v-if="sortIndicator('lastActive')" class="h-2.5 w-2.5" aria-hidden="true" />
                </button>
              </th>
              <th v-if="isColumnVisible('lastActive')" class="px-1.5 py-1.5 text-left font-semibold">
                <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('lastActive')">
                  {{ t(locale, 'desktop.sessions.columnLastActive') }}
                  <component :is="sortIndicator('lastActive')" v-if="sortIndicator('lastActive')" class="h-2.5 w-2.5" aria-hidden="true" />
                </button>
              </th>
              <th v-if="isColumnVisible('model')" class="px-1.5 py-1.5 text-left font-semibold">{{ t(locale, 'desktop.sessions.columnModel') }}</th>
              <th v-if="isColumnVisible('requests')" class="px-1.5 py-1.5 text-right font-semibold">
                <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('requests')">
                  {{ t(locale, 'desktop.sessions.columnRequests') }}
                  <component :is="sortIndicator('requests')" v-if="sortIndicator('requests')" class="h-2.5 w-2.5" aria-hidden="true" />
                </button>
              </th>
              <th v-if="isColumnVisible('tokens')" class="px-1.5 py-1.5 text-right font-semibold">
                <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('tokens')">
                  {{ t(locale, 'desktop.sessions.columnTokens') }}
                  <component :is="sortIndicator('tokens')" v-if="sortIndicator('tokens')" class="h-2.5 w-2.5" aria-hidden="true" />
                </button>
              </th>
              <th v-if="isColumnVisible('cost')" class="px-1.5 py-1.5 text-right font-semibold">
                <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('cost')">
                  {{ t(locale, 'desktop.sessions.columnCost') }}
                  <component :is="sortIndicator('cost')" v-if="sortIndicator('cost')" class="h-2.5 w-2.5" aria-hidden="true" />
                </button>
              </th>
              <th v-if="isColumnVisible('rate')" class="px-1.5 py-1.5 text-right font-semibold">
                <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('rate')">
                  {{ t(locale, 'desktop.sessions.columnAvgRate') }}
                  <component :is="sortIndicator('rate')" v-if="sortIndicator('rate')" class="h-2.5 w-2.5" aria-hidden="true" />
                </button>
              </th>
              <th v-if="isColumnVisible('errors')" class="px-1.5 py-1.5 text-right font-semibold">
                <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('errors')">
                  {{ t(locale, 'desktop.sessions.columnErrors') }}
                  <component :is="sortIndicator('errors')" v-if="sortIndicator('errors')" class="h-2.5 w-2.5" aria-hidden="true" />
                </button>
              </th>
              <th v-if="isColumnVisible('duration')" class="px-1.5 py-1.5 text-right font-semibold">
                <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('firstRequest')">
                  {{ t(locale, 'desktop.sessions.columnDuration') }}
                  <component :is="sortIndicator('firstRequest')" v-if="sortIndicator('firstRequest')" class="h-2.5 w-2.5" aria-hidden="true" />
                </button>
              </th>
            </tr>
          </thead>
          <tbody>
            <template v-if="store.sessionsLoading && store.sessions.length === 0">
              <tr
                v-for="row in skeletonRows"
                :key="`sk-${row}`"
                class="border-b border-[var(--theme-border-subtle)] last:border-0"
              >
                <td v-for="col in visibleCols.length" :key="col" class="px-1.5 py-1.5">
                  <div class="h-2.5 animate-pulse rounded bg-[var(--theme-border-default)]" :style="{ width: `${40 + ((row + col) % 5) * 12}%` }"></div>
                </td>
              </tr>
            </template>
            <tr v-else-if="paginatedSessions.length === 0">
              <td :colspan="visibleCols.length" class="px-3 py-12 text-center text-xs text-[var(--theme-text-tertiary)]">
                {{ store.sessions.length === 0 ? t(locale, 'desktop.sessions.noSessions') : t(locale, 'desktop.sessions.noMatch') }}
              </td>
            </tr>
            <tr
              v-for="session in paginatedSessions"
              :key="session.sessionId"
              tabindex="0"
              class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
              :class="selectedSession?.sessionId === session.sessionId && drawerOpen ? 'bg-[var(--theme-accent-soft)]' : ''"
              @click="openDrawer(session)"
              @contextmenu.prevent="handleRowContextMenu($event, session)"
              @keydown="handleRowKeydown($event, session); handleRowMenuKeydown($event, session)"
            >
              <td v-if="isColumnVisible('session')" class="max-w-72 px-1.5 py-1.5">
                <div class="flex min-w-0 items-center gap-2">
                  <LobeIcon v-if="getToolIcon(session.tool)" :slug="getToolIcon(session.tool) ?? 'claudecode'" :size="14" @error="() => {}" />
                  <span v-else class="h-2 w-2 shrink-0 rounded-full bg-[var(--theme-border-strong)]"></span>
                  <div class="min-w-0">
                    <p class="truncate font-medium text-[var(--theme-text-primary)]">{{ displaySessionTitle(session) }}</p>
                    <div class="flex items-center gap-1">
                      <span v-if="displaySessionProjectBadge(session)" class="truncate rounded px-1 py-px text-xs font-semibold leading-none" :class="projectBadgeClasses(session.projectIdentity)">
                        {{ displaySessionProjectBadge(session) }}
                      </span>
                      <span v-if="session.wslDistro" class="truncate rounded bg-cyan-500/10 px-1 py-px text-xs font-semibold leading-none text-cyan-600 dark:text-cyan-300" :title="t(locale, 'sessions.wslBadgeTitle', { distro: session.wslDistro })">{{ session.wslDistro }}</span>
                      <span v-if="sessionUsageVisible(session) && (session.uncoveredRequests ?? 0) > 0" class="rounded bg-amber-500/10 px-1 py-px text-xs font-semibold leading-none text-amber-600 dark:text-amber-300" :title="t(locale, 'desktop.sessions.partialCoverageTitle')">
                        {{ t(locale, 'desktop.sessions.partialCoverage') }}
                      </span>
                    </div>
                  </div>
                </div>
              </td>
              <td v-if="isColumnVisible('lastActive')" class="whitespace-nowrap px-1.5 py-1.5 text-[var(--theme-text-secondary)]">{{ formatTime(session.lastRequestTime) }}</td>
              <td v-if="isColumnVisible('model')" class="max-w-44 truncate px-1.5 py-1.5 font-mono text-xs text-[var(--theme-text-secondary)]" :title="session.models.join(', ')">
                {{ sessionModelLabel(session) }}<span v-if="session.models.length > 1" class="text-[var(--theme-text-quaternary)]"> +{{ session.models.length - 1 }}</span>
              </td>
              <td v-if="isColumnVisible('requests')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ session.totalRequests ?? 0 }}</td>
              <td v-if="isColumnVisible('tokens')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ sessionUsageVisible(session) ? formatTokens(sessionTotalTokens(session)) : '—' }}</td>
              <td v-if="isColumnVisible('cost')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ sessionUsageVisible(session) ? formatCost(session.estimatedCost) : '—' }}</td>
              <td v-if="isColumnVisible('rate')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">
                {{ sessionUsageVisible(session) && (session.avgOutputTokensPerSecond || 0) > 0 ? `${session.avgOutputTokensPerSecond.toFixed(1)}t/s` : '—' }}
              </td>
              <td v-if="isColumnVisible('errors')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono" :class="(session.errorRequests ?? 0) > 0 ? 'text-red-500' : 'text-[var(--theme-text-secondary)]'">
                {{ sessionUsageVisible(session) ? (session.errorRequests ?? 0) : '—' }}
              </td>
              <td v-if="isColumnVisible('duration')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatDuration(session.totalDurationMs) }}</td>
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
                  <span v-if="displaySessionProjectBadge(selectedSession!)" class="rounded px-1 py-px text-xs font-semibold leading-none" :class="projectBadgeClasses(selectedSession!.projectIdentity)">
                    {{ displaySessionProjectBadge(selectedSession!) }}
                  </span>
                  <span class="text-xs text-[var(--theme-text-tertiary)]">{{ formatTime(selectedSession!.lastRequestTime) }}</span>
                </div>
                <h3 class="truncate text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ displaySessionTitle(selectedSession!) }}</h3>
                <p class="mt-0.5 truncate text-xs text-[var(--theme-text-tertiary)]">
                  {{ selectedSession!.projectName || t(locale, 'common.unknownProject') }}
                  · {{ requestToolLabel(selectedSession!.tool) }}
                </p>
              </div>
              <div class="flex shrink-0 items-start gap-1">
                <button
                  type="button"
                  class="inline-flex items-center gap-1 rounded-lg border border-[var(--theme-border-default)] px-2 py-1 text-xs font-semibold text-[var(--theme-text-secondary)] transition-colors hover:border-[var(--theme-accent-primary)] hover:text-[var(--theme-accent-primary)]"
                  :title="t(locale, 'desktop.sessions.openWorkspaceHint')"
                  @click="openInWorkspace"
                >
                  <ExternalLink class="h-3 w-3" aria-hidden="true" />
                  {{ t(locale, 'desktop.sessions.menuOpenSession') }}
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
              <!-- 概览指标 -->
              <div class="grid grid-cols-3 gap-2">
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnRequests') }}</div>
                  <div class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">{{ selectedSession!.totalRequests ?? 0 }}</div>
                </div>
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'common.totalTokens') }}</div>
                  <div class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">{{ sessionUsageVisible(selectedSession!) ? formatTokens(sessionTotalTokens(selectedSession!)) : '—' }}</div>
                </div>
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.cost') }}</div>
                  <div class="font-mono text-xs font-semibold text-[var(--theme-chart-cost)]">{{ sessionUsageVisible(selectedSession!) ? formatCost(selectedSession!.estimatedCost) : '—' }}</div>
                </div>
              </div>

              <!-- Token 分项 -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.input') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatTokens(selectedSession!.totalInputTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.output') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatTokens(selectedSession!.totalOutputTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.cacheCreate') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatTokens(selectedSession!.totalCacheCreateTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.cacheRead') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatTokens(selectedSession!.totalCacheReadTokens) }}</span></div>
              </section>

              <!-- 性能 -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'metrics.tokensPerSecond') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ sessionUsageVisible(selectedSession!) && (selectedSession!.avgOutputTokensPerSecond || 0) > 0 ? `${selectedSession!.avgOutputTokensPerSecond.toFixed(1)}t/s` : '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.ttft') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ selectedSession!.avgTtftMs != null ? formatDuration(selectedSession!.avgTtftMs) : '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.duration') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatDuration(selectedSession!.totalDurationMs) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.errors') }}</span><span class="font-mono text-xs" :class="(selectedSession!.errorRequests ?? 0) > 0 ? 'text-red-500' : 'text-[var(--theme-text-primary)]'">{{ selectedSession!.errorRequests ?? 0 }}</span></div>
              </section>

              <!-- 模型列表 -->
              <section v-if="selectedSession!.models.length > 0" class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="mb-1 text-xs font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.models') }}</div>
                <div class="flex flex-wrap gap-1">
                  <span v-for="model in selectedSession!.models" :key="model" class="rounded bg-[var(--theme-border-subtle)] px-1.5 py-px font-mono text-xs text-[var(--theme-text-secondary)]">{{ model }}</span>
                </div>
              </section>

              <!-- 标识信息 -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.sessionId') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedSession!.sessionId">{{ shortId(selectedSession!.sessionId) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copyId')" :title="t(locale, 'desktop.sessions.copyId')" @click="copyText(selectedSession!.sessionId, 'id')"><ExternalLink class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div v-if="selectedSession!.cwd" class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'settings.cwd') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedSession!.cwd">{{ selectedSession!.cwd }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.menuCopyCwd')" :title="t(locale, 'desktop.sessions.menuCopyCwd')" @click="copyText(selectedSession!.cwd!, 'cwd')"><ExternalLink class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div v-if="selectedSession!.topic" class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.lastPrompt') }}</span>
                  <span class="truncate text-xs text-[var(--theme-text-secondary)]" :title="selectedSession!.topic">{{ selectedSession!.topic }}</span>
                </div>
              </section>

              <p v-if="copiedValue" class="text-center text-xs font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.sessions.copied') }}</p>
            </div>
          </aside>
        </div>
      </Transition>
    </div>

    <!-- 分页控件 -->
    <div class="flex shrink-0 items-center justify-between gap-3 text-xs text-[var(--theme-text-tertiary)]">
      <div class="flex shrink-0 items-center gap-2">
        <span>{{ pageStart }}–{{ pageEnd }} / {{ total }}</span>
        <DesktopSelect
          v-model="pageSize"
          :options="pageSizeSelectOptions"
          :aria-label="t(locale, 'desktop.requests.pageSize')"
          compact
          @change="onPageSizeChange"
        />
      </div>
      <div class="flex items-center gap-2">
        <div class="flex items-center gap-1">
          <button
            type="button"
            class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!hasPrev"
            :aria-label="t(locale, 'desktop.requests.pageFirst')"
            :title="t(locale, 'desktop.requests.pageFirst')"
            @click="gotoPage(0)"
          >
            <ChevronsLeft class="h-3.5 w-3.5" aria-hidden="true" />
          </button>
          <button
            type="button"
            class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!hasPrev"
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
              class="inline-flex h-7 min-w-[28px] items-center justify-center rounded-lg border px-1.5 font-mono text-xs transition-colors"
              :class="num === currentPage + 1
                ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-soft)] font-semibold text-[var(--theme-accent-primary)]'
                : 'border-[var(--theme-border-default)] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]'"
              :aria-current="num === currentPage + 1 ? 'page' : undefined"
              @click="gotoPageNumber(num)"
            >{{ num }}</button>
          </template>
          <button
            type="button"
            class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!hasNext"
            :aria-label="t(locale, 'desktop.requests.pageNext')"
            @click="nextPage"
          >
            <ChevronRight class="h-3.5 w-3.5" aria-hidden="true" />
          </button>
          <button
            type="button"
            class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
            :disabled="!hasNext"
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
            class="theme-input h-7 w-12 rounded-lg px-1 text-center font-mono text-xs outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
            :aria-label="t(locale, 'desktop.requests.jumpTo')"
            @keydown.enter="jumpPage"
          />
          <span class="text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.requests.pageOf', { current: '', total: totalPages }) }}</span>
        </div>
        <span v-if="loadingMore" class="text-xs text-[var(--theme-text-quaternary)]">{{ t(locale, 'common.syncing') }}</span>
      </div>
    </div>
  </div>

  <!-- 右键菜单 -->
  <Teleport to="body">
    <div
      v-if="contextMenu"
      ref="contextMenuRef"
      role="menu"
      class="theme-surface-elevated fixed z-[80] w-44 rounded-xl border p-1 shadow-xl"
      :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
      @keydown="handleMenuKeydown"
    >
      <button
        type="button"
        role="menuitem"
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)]"
        @click="runContextAction('open')"
      >
        {{ t(locale, 'desktop.sessions.menuOpenSession') }}
      </button>
      <button
        type="button"
        role="menuitem"
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)]"
        @click="runContextAction('copyId')"
      >
        {{ t(locale, 'desktop.sessions.menuCopyId') }}
      </button>
      <button
        type="button"
        role="menuitem"
        :disabled="!contextMenu.session.cwd"
        :title="contextMenu.session.cwd ? '' : t(locale, 'desktop.sessions.menuCopyCwdDisabled')"
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent"
        @click="runContextAction('copyCwd')"
      >
        {{ t(locale, 'desktop.sessions.menuCopyCwd') }}
      </button>
    </div>

    <div
      v-if="copiedValue"
      class="pointer-events-none fixed bottom-5 left-1/2 z-[90] -translate-x-1/2 rounded-full border border-[var(--theme-border-default)] bg-[var(--theme-bg-elevated)] px-3.5 py-1.5 text-xs font-medium text-emerald-600 shadow-lg dark:text-emerald-300"
    >
      {{ t(locale, 'desktop.sessions.copied') }}
    </div>
  </Teleport>
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
  transition: opacity 0.15s ease-out;
}
.drawer-overlay-enter-active > aside,
.drawer-overlay-leave-active > aside {
  transition: transform 0.18s ease-out;
}
.drawer-overlay-enter-from,
.drawer-overlay-leave-to {
  opacity: 0;
}
.drawer-overlay-enter-from > aside,
.drawer-overlay-leave-to > aside {
  transform: translateX(100%);
}
</style>
