<script setup lang="ts">
/**
 * 桌面主窗口「会话」页（设计文档第 8 章）。
 * 会话表格视图（会话/项目/请求已拆分为独立一级页面，本页只保留会话表格）。
 * 数据加载复用 useSessionViewData（store.fetchSessionsForTool）。
 * 会话工作区：nav.activeSessionKey 非空时渲染 SessionWorkspace 全页面（hash 由 desktopNavigation 路由处理）。
 */
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { ArrowDown, ArrowUp, ChevronDown, Search, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t } from '../../i18n'
import type { SessionStats } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useSessionViewData } from '../../composables/useSessionViewData'
import { useInfiniteScroll } from '../composables/useInfiniteScroll'
import { useClipboard } from '../composables/useClipboard'
import { useFocusTrap } from '../composables/useFocusTrap'
import LobeIcon from '../../components/LobeIcon.vue'
import SessionWorkspace from './SessionWorkspace.vue'

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

// —— 数据加载（本页固定会话表格 tab；hook 按 tab 分支加载） ——
const activeTab = ref<'recent'>('recent')
const {
  selectedTool,
  hasMore,
  loadingMore,
  loadMore,
  initialize: initializeSessionView,
  dispose: disposeSessionView,
} = useSessionViewData(store, activeTab)

// —— 会话工作区（全页面） ——
const workspaceKey = computed(() => nav.activeSessionKey)

// —— 从工作区返回列表时恢复滚动位置与筛选 ——
// openSession 保存的恢复信息原本只在 onMounted 消费，但列表/工作区切换不重挂载组件，
// 深链进入时还会把恢复值错误地消费在错误时机（空上下文）；改为返回瞬间消费。
watch(workspaceKey, async (key, prevKey) => {
  if (key != null || prevKey == null) return
  const restored = nav.consumePreviousSessionsQuery()
  if (!restored) return
  restoredScrollTop.value = restored.scrollTop
  if (restored.sourceId) await store.setActiveSourceFilter(restored.sourceId)
  if (restored.tool) await store.setActiveToolFilter(restored.tool)
  await nextTick()
  applyScrollRestore()
})

// —— 列表恢复信息（openSession 前保存的 scrollTop + 全局筛选） ——
const restoredScrollTop = ref(0)
const applyScrollRestore = () => {
  if (!restoredScrollTop.value) return
  const main = document.querySelector('main')
  if (main) {
    main.scrollTop = restoredScrollTop.value
    restoredScrollTop.value = 0
    nav.clearSessionScrollTop()
  }
}

// —— 会话列表滚动上报（主滚动容器为 DesktopShell 的 <main>；节流 150ms，仅列表视图时上报） ——
let scrollReportTimer: ReturnType<typeof setTimeout> | null = null
const handleMainScroll = () => {
  // 进入工作区后 main 的滚动属于工作区内容，不再上报
  if (nav.activeSessionKey) return
  if (scrollReportTimer) return
  scrollReportTimer = setTimeout(() => {
    scrollReportTimer = null
    const main = document.querySelector('main')
    if (main) nav.reportSessionListScrollTop(main.scrollTop)
  }, 150)
}

// —— 搜索（无全文索引：仅标题 / topic / 项目 / cwd） ——
const searchQuery = ref('')

// —— 筛选（chips 可删；来源由顶栏全局筛选承担，子代理/深度活动 M1 无数据） ——
type CoverageFilter = 'all' | 'full' | 'partial' | 'uncovered'
interface LocalFilters {
  /** 'all' | 'today' | '7d' | '30d' | 'custom'（custom 由趋势峰值下钻的 timeRange 填充，模板赋值放宽为 string） */
  time: string
  /** 趋势下钻携带的自定义时间桶（秒级半开区间 [startEpoch, endEpoch)，无则 null） */
  customRange: { startEpoch: number; endEpoch: number } | null
  project: string | null  // 项目名（含系统分组标记 global/unknown）
  model: string | null
  coverage: CoverageFilter
}
const filters = reactive<LocalFilters>({ time: 'all', customRange: null, project: null, model: null, coverage: 'all' })
const filterMenuOpen = ref(false)

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

// chips 展示（可逐个删除）
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

// —— 排序（原始数值） ——
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

const setTimeFilter = (value: string) => {
  // custom 需要先有趋势下钻携带的时间桶（无范围时手动点击不生效）
  if (value === 'custom' && !filters.customRange) return
  filters.time = value
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

// —— 会话表格选择 / 打开工作区 ——
const selectedSessionId = ref<string | null>(null)
const selectSession = (session: SessionStats) => { selectedSessionId.value = session.sessionId }
const openWorkspace = (session: SessionStats) => {
  nav.openSession(session.sessionId)
  selectedSessionId.value = null
}
const handleRowKeydown = (event: KeyboardEvent, session: SessionStats) => {
  if (event.key === 'Enter') {
    event.preventDefault()
    openWorkspace(session)
  }
}

// —— 会话行右键菜单（设计 8.3：仅安全操作——打开会话 / 复制会话 ID / 复制工作目录） ——
// 「在 Finder/Explorer 显示」依赖后端命令（当前无 show_in_folder/open_path 能力），留待后端补齐。
const { copiedValue, copyText } = useClipboard({ duration: 1400 })

interface ContextMenuState {
  session: SessionStats
  x: number
  y: number
}
const contextMenu = ref<ContextMenuState | null>(null)
const contextMenuOpen = computed(() => contextMenu.value !== null)
const contextMenuRef = ref<HTMLElement | null>(null)

/** 打开右键菜单：菜单固定在视口内（估算尺寸，防溢出）。 */
const openContextMenu = (clientX: number, clientY: number, session: SessionStats) => {
  const MENU_WIDTH = 176 // w-44
  const MENU_HEIGHT = 104 // 3 项 + padding
  const margin = 8
  const x = Math.max(margin, Math.min(clientX, window.innerWidth - MENU_WIDTH - margin))
  const y = Math.max(margin, Math.min(clientY, window.innerHeight - MENU_HEIGHT - margin))
  contextMenu.value = { session, x, y }
}
const closeContextMenu = () => { contextMenu.value = null }
useFocusTrap({ open: contextMenuOpen, container: contextMenuRef, onClose: closeContextMenu })

const handleRowContextMenu = (event: MouseEvent, session: SessionStats) => {
  // 模板 @contextmenu.prevent 已阻止系统菜单；这里仅定位
  openContextMenu(event.clientX, event.clientY, session)
}
const handleRowMenuKeydown = (event: KeyboardEvent, session: SessionStats) => {
  // Shift+F10 或菜单键（键盘可达性）
  if ((event.shiftKey && event.key === 'F10') || event.key === 'ContextMenu') {
    event.preventDefault()
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
    openContextMenu(rect.left + 32, rect.bottom - 8, session)
  }
}
const handleMenuKeydown = (event: KeyboardEvent) => {
  // 菜单内上下方向键在可用菜单项间循环移动焦点
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
    openWorkspace(session)
  } else if (action === 'copyId') {
    void copyText(session.sessionId, t(locale.value, 'desktop.sessions.menuCopyId'))
  } else if (action === 'copyCwd' && session.cwd) {
    void copyText(session.cwd, t(locale.value, 'desktop.sessions.menuCopyCwd'))
  }
}

// 点击外部 / 滚动 / 窗口尺寸变化时关闭（scroll 用捕获阶段监听，可覆盖 main 与表格内部滚动）
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

// —— 触底续载 ——
const loadMoreTrigger = ref<HTMLElement | null>(null)
useInfiniteScroll({
  trigger: loadMoreTrigger,
  onLoadMore: loadMore,
  hasMore: () => hasMore.value,
  loading: () => loadingMore.value,
  delay: 100,
  rootMargin: '160px',
  reobserve: () => store.sessions.length,
  reobserveDelay: 60
})

// 可选列（错误/时长，默认隐藏；设计文档 8.3）
const showOptionalColumns = ref(false)

/** 深链/事件导航带来的全局筛选上下文（sourceId/tool）应用；sessionKey 走 hash 路由
 *  （openSession 会写 hash，hashchange 同步 activeSessionKey 后工作区自动切换）。 */
async function applyPendingFilters() {
  const pending = nav.consumePendingFilters()
  if (!pending) return
  if (pending.sourceId && store.settings.sourceAware.activeSourceFilter !== pending.sourceId) {
    await store.setActiveSourceFilter(pending.sourceId)
  }
  if (pending.tool && store.settings.clientTools.activeToolFilter !== pending.tool) {
    await store.setActiveToolFilter(pending.tool)
  }
  // 趋势峰值下钻：携带时间桶过滤（筛选菜单显示为「自定义」范围）
  if (pending.timeRange) {
    filters.time = 'custom'
    filters.customRange = pending.timeRange
  }
  // 深链携带 sessionKey 时直接打开对应会话工作区（复用 openSession 的 hash/恢复逻辑；
  // 已处于该工作区则不重复导航；view 字段由 sessionKey 决定，忽略）
  if (pending.sessionKey && !nav.activeSessionKey) {
    nav.openSession(pending.sessionKey)
  }
}

// 同页深链：hash 相同页面不重挂载，onMounted 消费路径不执行；
// pendingConsumeTick 变化时若本页激活则补消费（跨页场景由 onMounted 覆盖，这里幂等）。
watch(
  () => nav.pendingConsumeTick,
  () => {
    if (nav.currentPage === 'sessions') void applyPendingFilters()
  }
)

onMounted(async () => {
  // 深链/事件导航带来的全局筛选上下文（sourceId/tool）先应用
  await applyPendingFilters()
  await initializeSessionView()
  await nextTick()
  applyScrollRestore()
  // 滚动上报：主滚动容器是 DesktopShell 的 <main>（与 applyScrollRestore 一致）
  document.querySelector('main')?.addEventListener('scroll', handleMainScroll, { passive: true })
})

onUnmounted(() => {
  disposeSessionView()
  document.querySelector('main')?.removeEventListener('scroll', handleMainScroll)
  if (scrollReportTimer) clearTimeout(scrollReportTimer)
  document.removeEventListener('pointerdown', closeMenuOnPointerDown, true)
  window.removeEventListener('scroll', closeMenuOnScroll, true)
  window.removeEventListener('resize', closeMenuOnScroll)
})

// —— 表格骨架行 ——
const skeletonRows = [0, 1, 2, 3, 4]

// —— 工具筛选选项 ——
const toolOptions = computed(() => {
  const tools = new Set<string>()
  for (const session of store.sessions) tools.add(session.tool)
  return [...tools].sort((a, b) => a.localeCompare(b))
})

// 来源筛选说明：由顶栏全局筛选器（SourceSelector）承担，会话行无来源字段（见 chips 区提示文案）
</script>

<template>
  <!-- 会话工作区：全页面替换列表（hash 已由 desktopNavigation 处理） -->
  <SessionWorkspace v-if="workspaceKey" :key="workspaceKey" :session-key="workspaceKey" />

  <div v-else class="flex flex-col gap-3 pb-4">
    <!-- 工具栏：搜索 + 筛选菜单 + 工具下拉 -->
    <div class="flex flex-wrap items-center gap-2">
      <div class="relative min-w-0 flex-1 basis-56">
        <!-- 搜索框：无全文索引时仅标题 / topic / 项目 / cwd 范围 -->
        <Search class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <input
          v-model="searchQuery"
          type="search"
          class="theme-input h-8 w-full rounded-lg pl-8 pr-3 text-[12px] outline-none"
          :placeholder="t(locale, 'desktop.sessions.searchPlaceholder')"
          :aria-label="t(locale, 'desktop.sessions.searchPlaceholder')"
        />
      </div>

      <!-- 筛选菜单（时间/项目/模型/覆盖状态；工具走会话列表右上角工具下拉；来源由顶栏全局筛选承担） -->
      <div class="relative">
        <button
          type="button"
          class="theme-button-secondary inline-flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12px] font-semibold"
          :aria-label="t(locale, 'desktop.sessions.filterLabel')"
          :aria-expanded="filterMenuOpen"
          @click="filterMenuOpen = !filterMenuOpen"
        >
          <ChevronDown class="h-3.5 w-3.5" aria-hidden="true" />
          {{ t(locale, 'desktop.sessions.filterLabel') }}
          <span v-if="chips.length > 0" class="rounded-full bg-[var(--theme-accent-primary)] px-1.5 text-[10px] font-bold text-[var(--theme-accent-contrast)]">{{ chips.length }}</span>
        </button>
        <div
          v-if="filterMenuOpen"
          class="theme-surface-elevated absolute right-0 top-10 z-30 w-60 rounded-xl border p-2 shadow-lg"
          role="menu"
        >
          <!-- 时间范围 -->
          <div class="px-2 pb-1 pt-1.5 text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.filterTime') }}</div>
          <div class="grid grid-cols-2 gap-1 px-1">
                        <button v-for="option in [['all', 'desktop.sessions.filterTimeAll'], ['today', 'desktop.sessions.filterTimeToday'], ['7d', 'desktop.sessions.filterTime7d'], ['30d', 'desktop.sessions.filterTime30d'], ['custom', 'desktop.sessions.filterTimeCustom']] as const" :key="option[0]" type="button" class="rounded-md px-2 py-1 text-left text-[11px] font-medium" :class="filters.time === option[0] ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]'" @click="setTimeFilter(option[0])">
              {{ t(locale, option[1]) }}
            </button>
          </div>
          <!-- 项目 -->
          <div class="px-2 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.filterProject') }}</div>
          <select v-model="filters.project" class="theme-input w-full rounded-lg px-2 py-1.5 text-[11px] outline-none" :aria-label="t(locale, 'desktop.sessions.filterProject')">
            <option :value="null">{{ t(locale, 'desktop.sessions.filterAllProjects') }}</option>
            <option v-for="project in projectOptions" :key="project" :value="project">{{ projectFilterLabel(project) }}</option>
          </select>
          <!-- 模型 -->
          <div class="px-2 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.filterModel') }}</div>
          <select v-model="filters.model" class="theme-input w-full rounded-lg px-2 py-1.5 text-[11px] outline-none" :aria-label="t(locale, 'desktop.sessions.filterModel')">
            <option :value="null">{{ t(locale, 'desktop.sessions.filterAllModels') }}</option>
            <option v-for="model in modelOptions" :key="model" :value="model">{{ model }}</option>
          </select>
          <!-- 覆盖状态 -->
          <div class="px-2 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.filterCoverage') }}</div>
          <div class="grid grid-cols-2 gap-1 px-1 pb-1">
            <button v-for="option in [['all', 'desktop.sessions.filterCoverageAll'], ['full', 'desktop.sessions.filterCoverageFull'], ['partial', 'desktop.sessions.filterCoveragePartial'], ['uncovered', 'desktop.sessions.filterCoverageUncovered']] as const" :key="option[0]" type="button" class="rounded-md px-2 py-1 text-left text-[11px] font-medium" :class="filters.coverage === option[0] ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]'" @click="filters.coverage = option[0]">
              {{ t(locale, option[1]) }}
            </button>
          </div>
          <div class="mt-1 border-t border-[var(--theme-border-default)] px-2 py-1.5 text-[10px] leading-snug text-[var(--theme-text-tertiary)]">
            {{ t(locale, 'desktop.sessions.filterSourceHint') }}
          </div>
        </div>
      </div>

      <!-- 工具下拉（复用 useSessionViewData 的 selectedTool，切换触发后端按工具重载） -->
      <select
        v-model="selectedTool"
        class="theme-input h-8 max-w-40 rounded-lg px-2 text-[12px] outline-none"
        :aria-label="t(locale, 'desktop.sessions.filterTool')"
      >
        <option :value="null">{{ t(locale, 'desktop.allTools') }}</option>
        <option v-for="tool in toolOptions" :key="tool" :value="tool">{{ requestToolLabel(tool) }}</option>
      </select>
    </div>

    <!-- 筛选 chips（可删；超 4 个折叠为“更多 N 项”，不横向滚动） -->
    <div v-if="chips.length > 0" class="flex flex-wrap items-center gap-1.5 text-[11px]">
      <span v-for="chip in (chipsCollapsed ? visibleChips : chips)" :key="chip.id" class="inline-flex items-center gap-1 rounded-full border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-2 py-0.5 font-medium text-[var(--theme-text-secondary)]">
        {{ chip.label }}
        <button type="button" class="rounded-full p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.removeFilter')" :title="t(locale, 'desktop.sessions.removeFilter')" @click="chip.remove()">
          <X class="h-3 w-3" aria-hidden="true" />
        </button>
      </span>
      <button v-if="extraChipsCount > 0" type="button" class="rounded-full border border-[var(--theme-border-default)] px-2 py-0.5 font-medium text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]" @click="chipsCollapsed = !chipsCollapsed">
        {{ chipsCollapsed ? t(locale, 'desktop.sessions.chipsMore', { count: extraChipsCount }) : t(locale, 'desktop.sessions.chipsLess') }}
      </button>
      <button type="button" class="ml-1 font-semibold text-[var(--theme-accent-primary)] hover:underline" @click="clearAllFilters">
        {{ t(locale, 'desktop.sessions.chipsClearAll') }}
      </button>
    </div>

    <!-- ================= 会话视图（表格） ================= -->
    <div class="flex items-center gap-3">
      <button
        type="button"
        class="inline-flex items-center gap-1 rounded-md border border-[var(--theme-border-default)] px-2 py-0.5 text-[10.5px] font-semibold text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]"
        :aria-pressed="showOptionalColumns"
        :title="t(locale, 'desktop.sessions.optionalColumnsHint')"
        @click="showOptionalColumns = !showOptionalColumns"
      >
        <ChevronDown class="h-3 w-3" aria-hidden="true" />
        {{ t(locale, 'desktop.sessions.optionalColumns') }}
      </button>
      <span class="text-[10.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.openWorkspaceHint') }}</span>
    </div>

    <!-- 表格容器：仅内部横向滚动，禁止页面级横向滚动 -->
    <div class="theme-surface overflow-x-auto rounded-xl border">
      <table class="w-full min-w-[860px] border-collapse text-[12px]">
        <thead>
          <tr class="border-b border-[var(--theme-border-default)] text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
            <!-- 首列 sticky -->
            <th class="sticky left-0 z-10 min-w-60 bg-[var(--theme-bg-elevated)] px-3 py-2 text-left font-semibold">
              <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('lastActive')">
                {{ t(locale, 'desktop.sessions.columnSession') }}
                <component :is="sortIndicator('lastActive')" v-if="sortIndicator('lastActive')" class="h-3 w-3" aria-hidden="true" />
              </button>
            </th>
            <th class="px-3 py-2 text-left font-semibold">
              <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('lastActive')">
                {{ t(locale, 'desktop.sessions.columnLastActive') }}
                <component :is="sortIndicator('lastActive')" v-if="sortIndicator('lastActive')" class="h-3 w-3" aria-hidden="true" />
              </button>
            </th>
            <th class="px-3 py-2 text-left font-semibold">{{ t(locale, 'desktop.sessions.columnModel') }}</th>
            <th class="px-3 py-2 text-right font-semibold">
              <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('requests')">
                {{ t(locale, 'desktop.sessions.columnRequests') }}
                <component :is="sortIndicator('requests')" v-if="sortIndicator('requests')" class="h-3 w-3" aria-hidden="true" />
              </button>
            </th>
            <th class="px-3 py-2 text-right font-semibold">
              <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('tokens')">
                {{ t(locale, 'desktop.sessions.columnTokens') }}
                <component :is="sortIndicator('tokens')" v-if="sortIndicator('tokens')" class="h-3 w-3" aria-hidden="true" />
              </button>
            </th>
            <th class="px-3 py-2 text-right font-semibold">
              <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('cost')">
                {{ t(locale, 'desktop.sessions.columnCost') }}
                <component :is="sortIndicator('cost')" v-if="sortIndicator('cost')" class="h-3 w-3" aria-hidden="true" />
              </button>
            </th>
            <th class="px-3 py-2 text-right font-semibold">
              <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('rate')">
                {{ t(locale, 'desktop.sessions.columnAvgRate') }}
                <component :is="sortIndicator('rate')" v-if="sortIndicator('rate')" class="h-3 w-3" aria-hidden="true" />
              </button>
            </th>
            <th v-if="showOptionalColumns" class="px-3 py-2 text-right font-semibold">
              <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('errors')">
                {{ t(locale, 'desktop.sessions.columnErrors') }}
                <component :is="sortIndicator('errors')" v-if="sortIndicator('errors')" class="h-3 w-3" aria-hidden="true" />
              </button>
            </th>
            <th v-if="showOptionalColumns" class="px-3 py-2 text-right font-semibold">
              <button type="button" class="inline-flex items-center gap-1 hover:text-[var(--theme-text-primary)]" @click="cycleSort('firstRequest')">
                {{ t(locale, 'desktop.sessions.columnDuration') }}
                <component :is="sortIndicator('firstRequest')" v-if="sortIndicator('firstRequest')" class="h-3 w-3" aria-hidden="true" />
              </button>
            </th>
          </tr>
        </thead>
        <tbody>
          <!-- 骨架行 -->
          <tr v-if="store.sessionsLoading && store.sessions.length === 0" v-for="row in skeletonRows" :key="`sk-${row}`" class="border-b border-[var(--theme-border-subtle)] last:border-0">
            <td v-for="col in showOptionalColumns ? 9 : 7" :key="col" class="px-3 py-2.5">
              <div class="h-2.5 animate-pulse rounded bg-[var(--theme-border-default)]" :style="{ width: `${40 + ((row + col) % 5) * 12}%` }"></div>
            </td>
          </tr>
          <!-- 空态 -->
          <tr v-else-if="filteredSessions.length === 0">
            <td :colspan="showOptionalColumns ? 9 : 7" class="px-3 py-14 text-center text-[12px] text-[var(--theme-text-tertiary)]">
              {{ store.sessions.length === 0 ? t(locale, 'desktop.sessions.noSessions') : t(locale, 'desktop.sessions.noMatch') }}
            </td>
          </tr>
          <!-- 数据行 -->
          <tr
            v-for="session in filteredSessions"
            :key="session.sessionId"
            tabindex="0"
            class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors last:border-0 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
            :class="selectedSessionId === session.sessionId ? 'bg-[var(--theme-accent-soft)]' : 'hover:bg-[var(--theme-bg-hover)]'"
            :aria-selected="selectedSessionId === session.sessionId"
            :title="t(locale, 'desktop.sessions.openWorkspaceHint')"
            @click="selectSession(session)"
            @dblclick="openWorkspace(session)"
            @contextmenu.prevent="handleRowContextMenu($event, session)"
            @keydown.enter="handleRowKeydown($event, session)"
            @keydown="handleRowMenuKeydown($event, session)"
          >
            <!-- 首列 sticky：标题 + 项目 badge + 工具图标 -->
            <td class="sticky left-0 z-10 max-w-72 bg-[var(--theme-bg-elevated)] px-3 py-2">
              <div class="flex min-w-0 items-center gap-2">
                <LobeIcon v-if="getToolIcon(session.tool)" :slug="getToolIcon(session.tool) ?? 'claudecode'" :size="14" @error="() => {}" />
                <span v-else class="h-2 w-2 shrink-0 rounded-full bg-[var(--theme-border-strong)]"></span>
                <div class="min-w-0">
                  <p class="truncate font-medium text-[var(--theme-text-primary)]">{{ displaySessionTitle(session) }}</p>
                  <div class="flex items-center gap-1">
                    <span v-if="displaySessionProjectBadge(session)" class="truncate rounded px-1 py-px text-[9px] font-semibold leading-none" :class="projectBadgeClasses(session.projectIdentity)">
                      {{ displaySessionProjectBadge(session) }}
                    </span>
                    <span v-if="session.wslDistro" class="truncate rounded bg-cyan-500/10 px-1 py-px text-[9px] font-semibold leading-none text-cyan-600 dark:text-cyan-300" :title="t(locale, 'sessions.wslBadgeTitle', { distro: session.wslDistro })">{{ session.wslDistro }}</span>
                    <span v-if="sessionUsageVisible(session) && (session.uncoveredRequests ?? 0) > 0" class="rounded bg-amber-500/10 px-1 py-px text-[9px] font-semibold leading-none text-amber-600 dark:text-amber-300" :title="t(locale, 'desktop.sessions.partialCoverageTitle')">
                      {{ t(locale, 'desktop.sessions.partialCoverage') }}
                    </span>
                  </div>
                </div>
              </div>
            </td>
            <td class="whitespace-nowrap px-3 py-2 text-[var(--theme-text-secondary)]">{{ formatTime(session.lastRequestTime) }}</td>
            <td class="max-w-44 truncate px-3 py-2 font-mono text-[11px] text-[var(--theme-text-secondary)]" :title="session.models.join(', ')">
              {{ sessionModelLabel(session) }}<span v-if="session.models.length > 1" class="text-[var(--theme-text-quaternary)]"> +{{ session.models.length - 1 }}</span>
            </td>
            <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-text-primary)]">{{ session.totalRequests ?? 0 }}</td>
            <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-text-primary)]">{{ sessionUsageVisible(session) ? formatTokens(sessionTotalTokens(session)) : '—' }}</td>
            <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-chart-cost)]">{{ sessionUsageVisible(session) ? formatCost(session.estimatedCost) : '—' }}</td>
            <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-text-secondary)]">
              {{ sessionUsageVisible(session) && (session.avgOutputTokensPerSecond || 0) > 0 ? `${session.avgOutputTokensPerSecond.toFixed(1)}t/s` : '—' }}
            </td>
            <td v-if="showOptionalColumns" class="whitespace-nowrap px-3 py-2 text-right font-mono" :class="(session.errorRequests ?? 0) > 0 ? 'text-red-500' : 'text-[var(--theme-text-secondary)]'">
              {{ sessionUsageVisible(session) ? (session.errorRequests ?? 0) : '—' }}
            </td>
            <td v-if="showOptionalColumns" class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatDuration(session.totalDurationMs) }}</td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 触底续载 -->
    <div v-if="loadingMore" class="flex justify-center py-3">
      <div class="h-4 w-4 animate-spin rounded-full border-2 border-[var(--theme-border-strong)] border-t-[var(--theme-accent-primary)]"></div>
    </div>
    <div v-else-if="!hasMore && store.sessions.length > 0" class="py-2 text-center text-[10.5px] text-[var(--theme-text-quaternary)]">
      {{ t(locale, 'common.noMore') }}
    </div>
    <div ref="loadMoreTrigger" class="h-1 w-full"></div>
  </div>

  <!-- 会话行右键菜单（仅安全操作：打开会话 / 复制会话 ID / 复制工作目录；cwd 为空禁用复制工作目录） -->
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
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-[11.5px] font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)]"
        @click="runContextAction('open')"
      >
        {{ t(locale, 'desktop.sessions.menuOpenSession') }}
      </button>
      <button
        type="button"
        role="menuitem"
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-[11.5px] font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)]"
        @click="runContextAction('copyId')"
      >
        {{ t(locale, 'desktop.sessions.menuCopyId') }}
      </button>
      <button
        type="button"
        role="menuitem"
        :disabled="!contextMenu.session.cwd"
        :title="contextMenu.session.cwd ? '' : t(locale, 'desktop.sessions.menuCopyCwdDisabled')"
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-[11.5px] font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent"
        @click="runContextAction('copyCwd')"
      >
        {{ t(locale, 'desktop.sessions.menuCopyCwd') }}
      </button>
    </div>

    <!-- 复制反馈轻提示（复用 useClipboard 的 copiedValue flash） -->
    <div
      v-if="copiedValue"
      class="pointer-events-none fixed bottom-5 left-1/2 z-[90] -translate-x-1/2 rounded-full border border-[var(--theme-border-default)] bg-[var(--theme-bg-elevated)] px-3.5 py-1.5 text-[11px] font-medium text-emerald-600 shadow-lg dark:text-emerald-300"
    >
      {{ t(locale, 'desktop.sessions.copied') }}
    </div>
  </Teleport>
</template>
