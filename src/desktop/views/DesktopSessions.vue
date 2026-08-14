<script setup lang="ts">
/**
 * 桌面主窗口「会话」页（设计文档第 8 章）。
 * 三个视图：会话（表格）/ 项目（master-detail）/ 请求（表格 + 右侧抽屉）。
 * 数据加载复用 useSessionViewData（store.fetchSessionsForTool / fetchRecentRequestRecordsForTool / fetchProjectStatsForTool）。
 * 会话工作区：nav.activeSessionKey 非空时渲染 SessionWorkspace 全页面（hash 由 desktopNavigation 路由处理）。
 */
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { ArrowDown, ArrowUp, ChevronDown, Copy, Folder, FileQuestionMark, Globe, Search, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t } from '../../i18n'
import type { ProjectStats, RequestRecord, SessionStats } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useSessionViewData } from '../../composables/useSessionViewData'
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
  requestModelLabel,
  requestStatusLabel,
  requestStatusClasses,
  requestCoverageLabel,
  requestProjectLabel,
  requestSourceLabel,
  requestToolLabel,
  requestCacheTokens,
  requestHasProxyPerformance,
  sessionUsageVisible,
  displaySessionTitle,
  displaySessionProjectBadge,
  projectBadgeClasses,
  displayProjectName,
  displayProjectHint,
  getToolIcon,
} = useSessionDisplay(store)

// —— 视图切换（沿用 useSessionViewData 的 tab 类型） ——
const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
const {
  selectedTool,
  hasMore,
  loadingMore,
  requestHasMore,
  loadingMoreRequests,
  loadMore,
  loadMoreRequests,
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
  time: string            // 'all' | 'today' | '7d' | '30d'（运行时字面量，模板 select 赋值放宽为 string）
  project: string | null  // 项目名（含系统分组标记 global/unknown）
  model: string | null
  coverage: CoverageFilter
}
const filters = reactive<LocalFilters>({ time: 'all', project: null, model: null, coverage: 'all' })
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
    '30d': t(locale.value, 'desktop.sessions.filterTime30d')
  }
  if (filters.time !== 'all') {
    result.push({ id: 'time', label: timeLabels[filters.time], remove: () => { filters.time = 'all' } })
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
  const now = Date.now()
  const horizon = filters.time === 'today' ? 24 * 3600e3 : filters.time === '7d' ? 7 * 24 * 3600e3 : 30 * 24 * 3600e3
  return (session.lastRequestTime || 0) * 1000 >= now - horizon
}

const setTimeFilter = (value: string) => { filters.time = value }

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

// —— 项目视图（master-detail） ——
const selectedProjectKey = ref<string | null>(null)
const projectList = computed(() => {
  const list = [...store.projectStats].sort((a, b) => (b.lastActive || 0) - (a.lastActive || 0))
  return list.slice(0, 200) // 超长保护：最多渲染 200 个
})
const projectTruncated = computed(() => store.projectStats.length > 200)
const projectKeyOf = (project: ProjectStats) => project.projectKey || project.projectPath || project.name
const selectProject = (project: ProjectStats) => { selectedProjectKey.value = projectKeyOf(project) }
const selectedProject = computed(() => store.projectStats.find(project => projectKeyOf(project) === selectedProjectKey.value) ?? null)

const projectTotalTokens = (project: ProjectStats) =>
  (project.totalInputTokens || 0) + (project.totalOutputTokens || 0) + (project.totalCacheCreateTokens || 0) + (project.totalCacheReadTokens || 0)

// 项目详情中的会话（复用会话表组件逻辑，预设项目过滤）
const sessionsOfProject = computed(() => {
  const project = selectedProject.value
  if (!project) return []
  return filteredSessions.value.filter(session => {
    if (project.projectIdentity === 'global') return session.projectIdentity === 'global'
    if (project.projectIdentity === 'unknown') return session.projectIdentity === 'unknown'
    return !!session.projectName && session.projectName === project.name
  })
})

// 项目详情中的模型构成（从该项目会话聚合）
const modelsOfProject = computed(() => {
  const counts = new Map<string, number>()
  for (const session of sessionsOfProject.value) {
    const model = session.models[0]
    if (!model) continue
    counts.set(model, (counts.get(model) ?? 0) + 1)
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12)
})

// —— 请求视图（表格 + 右侧抽屉） ——
// 请求筛选（select 的 v-model 值放宽为 string，比较时按字面量处理）
const requestStatusFilter = ref<string>('all')
const requestCoverageFilter = ref<string>('all')
const requestPerfFilter = ref<string>('all')

const filteredRequests = computed(() => store.requestRecords.filter(request => {
  if (requestStatusFilter.value === 'local' && request.coverageOrigin !== 'local_only') return false
  if (requestStatusFilter.value === 'success') {
    if (request.coverageOrigin === 'local_only' || (request.statusCode ?? 0) >= 400) return false
  }
  if (requestStatusFilter.value === 'error' && (request.statusCode ?? 0) < 400) return false
  if (requestCoverageFilter.value !== 'all' && request.coverageOrigin !== requestCoverageFilter.value) return false
  if (requestPerfFilter.value === 'has' && !requestHasProxyPerformance(request)) return false
  if (requestPerfFilter.value === 'none' && requestHasProxyPerformance(request)) return false
  return true
}))

const selectedRequest = ref<RequestRecord | null>(null)
const requestDrawerOpen = ref(false)
const openRequestDrawer = (request: RequestRecord) => {
  selectedRequest.value = request
  requestDrawerOpen.value = true
}
const closeRequestDrawer = () => {
  requestDrawerOpen.value = false
  selectedRequest.value = null
}

// 请求 ID 折叠为短值，复制时复制完整值
const shortId = (value: string) => (value.length > 12 ? `${value.slice(0, 6)}…${value.slice(-4)}` : value)
const copiedValue = ref('')
let copiedTimer: ReturnType<typeof setTimeout> | null = null
const copyText = async (value: string, label: string) => {
  try {
    await navigator.clipboard.writeText(value)
    copiedValue.value = label
    if (copiedTimer) clearTimeout(copiedTimer)
    copiedTimer = setTimeout(() => { copiedValue.value = '' }, 1200)
  } catch { /* 剪贴板不可用时静默失败 */ }
}

// —— 触底续载（会话/请求两个观察器） ——
const loadMoreTrigger = ref<HTMLElement | null>(null)
const requestLoadMoreTrigger = ref<HTMLElement | null>(null)
let observer: IntersectionObserver | null = null
let observeTimer: ReturnType<typeof setTimeout> | null = null

const observeTriggers = () => {
  if (!observer) return
  if (loadMoreTrigger.value) observer.observe(loadMoreTrigger.value)
  if (requestLoadMoreTrigger.value) observer.observe(requestLoadMoreTrigger.value)
}
const scheduleObserve = () => {
  if (observeTimer) clearTimeout(observeTimer)
  observeTimer = setTimeout(() => { observeTimer = null; observeTriggers() }, 60)
}
watch(() => store.sessions.length, scheduleObserve)
watch(() => store.requestRecords.length, scheduleObserve)
watch(activeTab, scheduleObserve)

// 请求筛选变化时选中行可能消失，清空抽屉避免悬挂引用
watch(filteredRequests, list => {
  if (selectedRequest.value && !list.some(request => request.requestKey === selectedRequest.value?.requestKey)) {
    closeRequestDrawer()
  }
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
  setTimeout(() => {
    observer = new IntersectionObserver(
      entries => {
        if (!entries[0].isIntersecting) return
        if (entries[0].target === loadMoreTrigger.value && hasMore.value && !loadingMore.value) loadMore()
        if (entries[0].target === requestLoadMoreTrigger.value && requestHasMore.value && !loadingMoreRequests.value) loadMoreRequests()
      },
      { root: null, rootMargin: '160px' }
    )
    observeTriggers()
  }, 100)
  // 滚动上报：主滚动容器是 DesktopShell 的 <main>（与 applyScrollRestore 一致）
  document.querySelector('main')?.addEventListener('scroll', handleMainScroll, { passive: true })
})

onUnmounted(() => {
  disposeSessionView()
  document.querySelector('main')?.removeEventListener('scroll', handleMainScroll)
  if (scrollReportTimer) clearTimeout(scrollReportTimer)
  if (observer) observer.disconnect()
  if (observeTimer) clearTimeout(observeTimer)
  if (copiedTimer) clearTimeout(copiedTimer)
})

// —— 表格骨架行 ——
const skeletonRows = [0, 1, 2, 3, 4]

// —— 工具筛选选项（SESSION_SOURCE_TOOLS 来自 useSessionViewData 内部，这里从 store 取已见工具） ——
const toolOptions = computed(() => {
  const tools = new Set<string>()
  for (const session of store.sessions) tools.add(session.tool)
  for (const request of store.requestRecords) tools.add(request.tool)
  return [...tools].sort((a, b) => a.localeCompare(b))
})

// 来源筛选说明：由顶栏全局筛选器（SourceSelector）承担，会话行无来源字段（见 chips 区提示文案）
</script>

<template>
  <!-- 会话工作区：全页面替换列表（hash 已由 desktopNavigation 处理） -->
  <SessionWorkspace v-if="workspaceKey" :key="workspaceKey" :session-key="workspaceKey" />

  <div v-else class="flex flex-col gap-3 pb-4">
    <!-- 工具栏：搜索 + 视图切换 + 筛选菜单 -->
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
            <button v-for="option in [['all', 'desktop.sessions.filterTimeAll'], ['today', 'desktop.sessions.filterTimeToday'], ['7d', 'desktop.sessions.filterTime7d'], ['30d', 'desktop.sessions.filterTime30d']] as const" :key="option[0]" type="button" class="rounded-md px-2 py-1 text-left text-[11px] font-medium" :class="filters.time === option[0] ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]'" @click="setTimeFilter(option[0])">
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

      <!-- 视图切换 -->
      <div class="ml-auto flex shrink-0 gap-0.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] p-0.5" role="tablist" :aria-label="t(locale, 'desktop.sessions.viewSwitch')">
        <button
          v-for="tab in [['recent', 'desktop.sessions.viewSessions'], ['requests', 'desktop.sessions.viewRequests'], ['projects', 'desktop.sessions.viewProjects']] as const"
          :key="tab[0]"
          type="button"
          role="tab"
          :aria-selected="activeTab === tab[0]"
          class="rounded-md px-3 py-1 text-[11.5px] font-semibold transition-colors"
          :class="activeTab === tab[0] ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'"
          @click="activeTab = tab[0]"
        >
          {{ t(locale, tab[1]) }}
        </button>
      </div>
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
    <template v-if="activeTab === 'recent'">
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
              class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors last:border-0 focus:outline-none"
              :class="selectedSessionId === session.sessionId ? 'bg-[var(--theme-accent-soft)]' : 'hover:bg-[var(--theme-bg-hover)]'"
              :title="t(locale, 'desktop.sessions.openWorkspaceHint')"
              @click="selectSession(session)"
              @dblclick="openWorkspace(session)"
              @keydown.enter="handleRowKeydown($event, session)"
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
    </template>

    <!-- ================= 项目视图（master-detail 两栏） ================= -->
    <template v-else-if="activeTab === 'projects'">
      <div v-if="store.projectStatsLoading && store.projectStats.length === 0" class="flex justify-center py-10">
        <div class="h-5 w-5 animate-spin rounded-full border-2 border-[var(--theme-border-strong)] border-t-[var(--theme-accent-primary)]"></div>
      </div>
      <div v-else-if="store.projectStats.length === 0" class="py-14 text-center text-[12px] text-[var(--theme-text-tertiary)]">
        {{ t(locale, 'desktop.sessions.noProjects') }}
      </div>
      <div v-else class="flex min-h-0 gap-3">
        <!-- 左：项目列表（最小 320px） -->
        <div class="theme-surface w-80 shrink-0 self-start overflow-hidden rounded-xl border">
          <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
            {{ t(locale, 'desktop.sessions.projectsColumn') }}
            <span v-if="projectTruncated" class="ml-1 normal-case text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.sessions.projectsTruncated', { count: 200 }) }}</span>
          </div>
          <div class="max-h-[calc(100vh-260px)] overflow-y-auto">
            <button
              v-for="project in projectList"
              :key="projectKeyOf(project)"
              type="button"
              class="flex w-full items-start gap-2 border-b border-[var(--theme-border-subtle)] px-3 py-2 text-left transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)]"
              :class="selectedProjectKey === projectKeyOf(project) ? 'bg-[var(--theme-accent-soft)]' : ''"
              @click="selectProject(project)"
            >
              <!-- 系统分组：全局 / 未知项目，不伪装成普通项目 -->
              <span v-if="project.projectIdentity === 'global'" class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-slate-500/10 text-slate-500 dark:text-slate-300"><Globe class="h-3.5 w-3.5" aria-hidden="true" /></span>
              <span v-else-if="project.projectIdentity === 'unknown'" class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-amber-500/10 text-amber-600 dark:text-amber-300"><FileQuestionMark class="h-3.5 w-3.5" aria-hidden="true" /></span>
              <span v-else class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-indigo-500/10 text-indigo-500 dark:text-indigo-300"><Folder class="h-3.5 w-3.5" aria-hidden="true" /></span>
              <span class="min-w-0 flex-1">
                <span class="flex items-center justify-between gap-2">
                  <span class="truncate text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ displayProjectName(project) }}</span>
                  <span class="shrink-0 text-[10px] text-[var(--theme-text-quaternary)]">{{ formatTime(project.lastActive) }}</span>
                </span>
                <span v-if="project.projectPath" class="mt-0.5 block truncate font-mono text-[10px] text-[var(--theme-text-tertiary)]">{{ project.projectPath }}</span>
                <span v-if="displayProjectHint(project)" class="mt-0.5 block text-[10px] text-[var(--theme-text-quaternary)]">{{ displayProjectHint(project) }}</span>
                <span class="mt-1 block truncate text-[10px] text-[var(--theme-text-secondary)]">
                  {{ t(locale, 'desktop.sessions.projectMeta', { sessions: project.sessionCount, requests: project.requestCount, tokens: formatTokens(projectTotalTokens(project)), cost: formatCost(project.totalCost) }) }}
                </span>
              </span>
            </button>
          </div>
        </div>

        <!-- 右：项目摘要 -->
        <div class="min-w-0 flex-1 space-y-3">
          <template v-if="selectedProject">
            <div class="theme-surface rounded-xl border px-4 py-3">
              <div class="flex flex-wrap items-center gap-2">
                <h2 class="text-[14px] font-bold text-[var(--theme-text-primary)]">{{ displayProjectName(selectedProject) }}</h2>
                <span v-if="selectedProject.projectIdentity === 'global'" class="rounded bg-slate-500/10 px-1.5 py-0.5 text-[9.5px] font-semibold text-slate-500 dark:text-slate-300">{{ t(locale, 'common.global') }}</span>
                <span v-else-if="selectedProject.projectIdentity === 'unknown'" class="rounded bg-amber-500/10 px-1.5 py-0.5 text-[9.5px] font-semibold text-amber-600 dark:text-amber-300">{{ t(locale, 'common.unknownProject') }}</span>
                <span class="ml-auto text-[11px] text-[var(--theme-text-tertiary)]">{{ formatTime(selectedProject.lastActive) }}</span>
              </div>
              <p v-if="selectedProject.projectPath" class="mt-1 truncate font-mono text-[10.5px] text-[var(--theme-text-tertiary)]">{{ selectedProject.projectPath }}</p>
              <p v-if="displayProjectHint(selectedProject)" class="mt-1 text-[10.5px] text-[var(--theme-text-quaternary)]">{{ displayProjectHint(selectedProject) }}</p>
            </div>

            <!-- 汇总统计 -->
            <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
              <div class="theme-surface rounded-xl border px-3 py-2.5">
                <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.projectSessions') }}</div>
                <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ selectedProject.sessionCount ?? 0 }}</div>
              </div>
              <div class="theme-surface rounded-xl border px-3 py-2.5">
                <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnRequests') }}</div>
                <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ selectedProject.requestCount ?? 0 }}</div>
              </div>
              <div class="theme-surface rounded-xl border px-3 py-2.5">
                <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnTokens') }}</div>
                <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(projectTotalTokens(selectedProject)) }}</div>
              </div>
              <div class="theme-surface rounded-xl border px-3 py-2.5">
                <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnCost') }}</div>
                <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-chart-cost)]">{{ formatCost(selectedProject.totalCost) }}</div>
              </div>
            </div>

            <!-- 工具构成 -->
            <div class="theme-surface rounded-xl border">
              <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.toolComposition') }}</div>
              <div v-if="!selectedProject.toolBreakdown || selectedProject.toolBreakdown.length === 0" class="px-3 py-4 text-center text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noToolData') }}</div>
              <table v-else class="w-full text-[11.5px]">
                <thead>
                  <tr class="border-b border-[var(--theme-border-subtle)] text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
                    <th class="px-3 py-1.5 text-left">{{ t(locale, 'sessions.tool') }}</th>
                    <th class="px-3 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnRequests') }}</th>
                    <th class="px-3 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnTokens') }}</th>
                    <th class="px-3 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnCost') }}</th>
                    <th class="px-3 py-1.5 text-right">{{ t(locale, 'sessions.lastActive') }}</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="tool in selectedProject.toolBreakdown" :key="tool.tool" class="border-b border-[var(--theme-border-subtle)] last:border-0">
                    <td class="px-3 py-1.5">
                      <span class="flex items-center gap-1.5">
                        <LobeIcon v-if="getToolIcon(tool.tool)" :slug="getToolIcon(tool.tool) ?? 'claudecode'" :size="12" @error="() => {}" />
                        <span v-else class="h-2 w-2 rounded-full bg-[var(--theme-border-strong)]"></span>
                        <span class="truncate text-[var(--theme-text-secondary)]">{{ requestToolLabel(tool.tool) }}</span>
                      </span>
                    </td>
                    <td class="px-3 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ tool.requestCount ?? 0 }}</td>
                    <td class="px-3 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens((tool.totalInputTokens || 0) + (tool.totalOutputTokens || 0) + (tool.totalCacheCreateTokens || 0) + (tool.totalCacheReadTokens || 0)) }}</td>
                    <td class="px-3 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(tool.totalCost) }}</td>
                    <td class="px-3 py-1.5 text-right text-[var(--theme-text-tertiary)]">{{ formatTime(tool.lastActive) }}</td>
                  </tr>
                </tbody>
              </table>
            </div>

            <!-- 模型构成 -->
            <div class="theme-surface rounded-xl border">
              <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.modelComposition') }}</div>
              <div v-if="modelsOfProject.length === 0" class="px-3 py-4 text-center text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noModelData') }}</div>
              <div v-else class="space-y-1 px-3 py-2">
                <div v-for="[model, count] in modelsOfProject" :key="model" class="flex items-center gap-2 text-[11.5px]">
                  <span class="min-w-0 flex-1 truncate font-mono text-[var(--theme-text-secondary)]">{{ model }}</span>
                  <span class="h-1.5 flex-1 rounded-full bg-[var(--theme-border-subtle)]">
                    <span class="block h-full rounded-full bg-[var(--theme-accent-primary)]" :style="{ width: `${(count / (modelsOfProject[0]?.[1] ?? 1)) * 100}%` }"></span>
                  </span>
                  <span class="w-10 text-right font-mono text-[var(--theme-text-primary)]">{{ count }}</span>
                </div>
              </div>
            </div>

            <!-- 会话列表（复用会话行，预设项目过滤） -->
            <div class="theme-surface rounded-xl border">
              <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.projectSessionsList') }}</div>
              <div v-if="sessionsOfProject.length === 0" class="px-3 py-4 text-center text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noSessions') }}</div>
              <button
                v-for="session in sessionsOfProject"
                :key="session.sessionId"
                type="button"
                class="flex w-full items-center gap-2 border-b border-[var(--theme-border-subtle)] px-3 py-1.5 text-left transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)]"
                :title="t(locale, 'desktop.sessions.openWorkspaceHint')"
                @dblclick="openWorkspace(session)"
              >
                <span class="min-w-0 flex-1 truncate text-[12px] font-medium text-[var(--theme-text-primary)]">{{ displaySessionTitle(session) }}</span>
                <span class="shrink-0 text-[10px] text-[var(--theme-text-tertiary)]">{{ formatTime(session.lastRequestTime) }}</span>
                <span class="w-16 shrink-0 text-right font-mono text-[11px] text-[var(--theme-text-secondary)]">{{ session.totalRequests ?? 0 }}</span>
                <span class="w-20 shrink-0 text-right font-mono text-[11px] text-[var(--theme-text-secondary)]">{{ sessionUsageVisible(session) ? formatTokens(sessionTotalTokens(session)) : '—' }}</span>
              </button>
            </div>
          </template>
          <div v-else class="theme-surface flex h-48 items-center justify-center rounded-xl border text-[12px] text-[var(--theme-text-tertiary)]">
            {{ t(locale, 'desktop.sessions.selectProjectHint') }}
          </div>
        </div>
      </div>
    </template>

    <!-- ================= 请求视图（表格 + 右侧抽屉） ================= -->
    <template v-else>
      <!-- 请求筛选：状态 / 覆盖来源 / 性能 -->
      <div class="flex flex-wrap items-center gap-2 text-[11px]">
        <select v-model="requestStatusFilter" class="theme-input h-7 rounded-lg px-2 text-[11px] outline-none" :aria-label="t(locale, 'desktop.sessions.requestFilterStatus')">
          <option value="all">{{ t(locale, 'desktop.sessions.requestFilterStatusAll') }}</option>
          <option value="success">{{ t(locale, 'common.success') }}</option>
          <option value="error">{{ t(locale, 'common.error') }}</option>
          <option value="local">{{ t(locale, 'desktop.sessions.requestFilterStatusLocal') }}</option>
        </select>
        <select v-model="requestCoverageFilter" class="theme-input h-7 rounded-lg px-2 text-[11px] outline-none" :aria-label="t(locale, 'desktop.sessions.requestFilterCoverage')">
          <option value="all">{{ t(locale, 'desktop.sessions.requestFilterCoverageAll') }}</option>
          <option value="proxy_only">{{ t(locale, 'sessions.requestCoverageProxy') }}</option>
          <option value="local_only">{{ t(locale, 'sessions.requestCoverageLocal') }}</option>
          <option value="merged">{{ t(locale, 'sessions.requestCoverageMerged') }}</option>
        </select>
        <select v-model="requestPerfFilter" class="theme-input h-7 rounded-lg px-2 text-[11px] outline-none" :aria-label="t(locale, 'desktop.sessions.requestFilterPerformance')">
          <option value="all">{{ t(locale, 'desktop.sessions.requestFilterPerfAll') }}</option>
          <option value="has">{{ t(locale, 'desktop.sessions.requestFilterPerfHas') }}</option>
          <option value="none">{{ t(locale, 'desktop.sessions.requestFilterPerfNone') }}</option>
        </select>
        <span class="ml-auto text-[10.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.requestCount', { count: filteredRequests.length }) }}</span>
      </div>

      <div class="flex min-h-0 gap-3">
        <!-- 请求表格 -->
        <div class="theme-surface min-w-0 flex-1 overflow-x-auto rounded-xl border">
          <table class="w-full min-w-[900px] border-collapse text-[11.5px]">
            <thead>
              <tr class="border-b border-[var(--theme-border-default)] text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnTime') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnTool') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnSource') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnModel') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnStatus') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnInput') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnOutput') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnCache') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnTotalTokens') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnCost') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnTtft') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnDuration') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnRate') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnSession') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-if="store.requestRecordsLoading && store.requestRecords.length === 0" v-for="row in skeletonRows" :key="`rsk-${row}`" class="border-b border-[var(--theme-border-subtle)] last:border-0">
                <td v-for="col in 14" :key="col" class="px-2.5 py-2"><div class="h-2.5 animate-pulse rounded bg-[var(--theme-border-default)]"></div></td>
              </tr>
              <tr v-else-if="filteredRequests.length === 0">
                <td colspan="14" class="px-3 py-12 text-center text-[11.5px] text-[var(--theme-text-tertiary)]">{{ store.requestRecords.length === 0 ? t(locale, 'desktop.sessions.noRequests') : t(locale, 'desktop.sessions.noMatch') }}</td>
              </tr>
              <tr
                v-for="request in filteredRequests"
                :key="request.requestKey"
                class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)]"
                :class="selectedRequest?.requestKey === request.requestKey ? 'bg-[var(--theme-accent-soft)]' : ''"
                @click="openRequestDrawer(request)"
              >
                <td class="whitespace-nowrap px-2.5 py-1.5 text-[var(--theme-text-secondary)]">{{ formatTime(request.timestampSec) }}</td>
                <td class="px-2.5 py-1.5">
                  <span class="flex items-center gap-1.5">
                    <LobeIcon v-if="getToolIcon(request.tool)" :slug="getToolIcon(request.tool) ?? 'claudecode'" :size="12" @error="() => {}" />
                    <span v-else class="h-1.5 w-1.5 rounded-full bg-[var(--theme-border-strong)]"></span>
                    <span class="max-w-20 truncate text-[var(--theme-text-secondary)]">{{ requestToolLabel(request.tool) }}</span>
                  </span>
                </td>
                <td class="max-w-28 truncate px-2.5 py-1.5 text-[var(--theme-text-secondary)]" :title="requestSourceLabel(request)">{{ requestSourceLabel(request) }}</td>
                <td class="max-w-36 truncate px-2.5 py-1.5 font-mono text-[10.5px] text-[var(--theme-text-secondary)]" :title="request.model">{{ requestModelLabel(request) }}</td>
                <td class="px-2.5 py-1.5">
                  <!-- 本地-only 记录不展示不存在的状态值 -->
                  <span v-if="request.coverageOrigin === 'local_only'" class="text-[10px] text-[var(--theme-text-quaternary)]">—</span>
                  <span v-else class="inline-flex items-center rounded-full border px-1.5 py-px text-[9.5px] font-bold leading-none" :class="requestStatusClasses(request)">{{ requestStatusLabel(request) }}</span>
                </td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.inputTokens) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.outputTokens) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(requestCacheTokens(request)) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(request.totalTokens) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(request.estimatedCost) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) ? formatDuration(request.ttftMs) : '—' }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) ? formatDuration(request.durationMs) : '—' }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) && request.outputTokensPerSecond ? `${request.outputTokensPerSecond.toFixed(1)}t/s` : '—' }}</td>
                <td class="max-w-28 truncate px-2.5 py-1.5 font-mono text-[10px] text-[var(--theme-text-tertiary)]" :title="request.sessionId">{{ shortId(request.sessionId) }}</td>
              </tr>
            </tbody>
          </table>
        </div>

        <!-- 右侧请求详情抽屉（不使用居中 modal） -->
        <Transition name="drawer-slide">
          <aside
            v-if="requestDrawerOpen && selectedRequest"
            class="theme-surface-elevated w-80 shrink-0 overflow-y-auto rounded-xl border"
            :aria-label="t(locale, 'desktop.sessions.drawerTitle')"
          >
            <div class="flex items-start justify-between gap-2 border-b border-[var(--theme-border-default)] px-3 py-2.5">
              <div class="min-w-0">
                <div class="mb-1 flex items-center gap-1.5">
                  <!-- 本地-only 记录不展示状态 -->
                  <span v-if="selectedRequest.coverageOrigin !== 'local_only'" class="inline-flex items-center rounded-full border px-1.5 py-px text-[9.5px] font-bold leading-none" :class="requestStatusClasses(selectedRequest)">{{ requestStatusLabel(selectedRequest) }}</span>
                  <span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ formatTime(selectedRequest.timestampSec) }}</span>
                </div>
                <h3 class="truncate text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ requestModelLabel(selectedRequest) }}</h3>
                <p class="mt-0.5 truncate text-[10px] text-[var(--theme-text-tertiary)]">{{ requestProjectLabel(selectedRequest) }} / {{ requestToolLabel(selectedRequest.tool) }} / {{ requestSourceLabel(selectedRequest) }}</p>
              </div>
              <button
                type="button"
                class="shrink-0 rounded-lg p-1 transition-colors hover:bg-[var(--theme-bg-hover)]"
                :aria-label="t(locale, 'common.close')"
                :title="t(locale, 'common.close')"
                @click="closeRequestDrawer"
              >
                <X class="h-4 w-4 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
              </button>
            </div>

            <div class="space-y-3 p-3">
              <!-- 统计卡片 -->
              <div class="grid grid-cols-3 gap-2">
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-[9.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'common.totalTokens') }}</div>
                  <div class="font-mono text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest.totalTokens) }}</div>
                </div>
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-[9.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.cost') }}</div>
                  <div class="font-mono text-[12px] font-semibold text-[var(--theme-chart-cost)]">{{ formatCost(selectedRequest.estimatedCost) }}</div>
                </div>
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-[9.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.duration') }}</div>
                  <div class="font-mono text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ formatDuration(selectedRequest.durationMs) }}</div>
                </div>
              </div>

              <!-- Token 明细 -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.input') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest.inputTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.output') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest.outputTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.cacheCreate') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest.cacheCreateTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.cacheRead') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ formatTokens(selectedRequest.cacheReadTokens) }}</span></div>
              </section>

              <!-- 性能与覆盖（本地-only 不展示不存在的性能值） -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.ttft') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ requestHasProxyPerformance(selectedRequest) ? formatDuration(selectedRequest.ttftMs) : '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'metrics.tokensPerSecond') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ requestHasProxyPerformance(selectedRequest) && selectedRequest.outputTokensPerSecond ? `${selectedRequest.outputTokensPerSecond.toFixed(1)}t/s` : '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.status') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ selectedRequest.statusCode || '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.requestCoverage') }}</span><span class="font-mono text-[11px] text-[var(--theme-text-primary)]">{{ requestCoverageLabel(selectedRequest.coverageOrigin) }}</span></div>
              </section>

              <!-- 标识信息：ID 折叠为短值，复制时复制完整值 -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'common.source') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-[10.5px] text-[var(--theme-text-secondary)]">{{ requestSourceLabel(selectedRequest) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copySource')" :title="t(locale, 'desktop.sessions.copySource')" @click="copyText(requestSourceLabel(selectedRequest), 'source')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.sessionId') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-[10.5px] text-[var(--theme-text-secondary)]" :title="selectedRequest.sessionId">{{ shortId(selectedRequest.sessionId) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copyId')" :title="t(locale, 'desktop.sessions.copyId')" @click="copyText(selectedRequest.sessionId, 'id')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.requestKey') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-[10.5px] text-[var(--theme-text-secondary)]" :title="selectedRequest.requestKey">{{ shortId(selectedRequest.requestKey) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copyKey')" :title="t(locale, 'desktop.sessions.copyKey')" @click="copyText(selectedRequest.requestKey, 'key')"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
              </section>

              <p v-if="copiedValue" class="text-center text-[10px] font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.sessions.copied') }}</p>
            </div>
          </aside>
        </Transition>
      </div>

      <!-- 触底续载 -->
      <div v-if="loadingMoreRequests" class="flex justify-center py-3">
        <div class="h-4 w-4 animate-spin rounded-full border-2 border-[var(--theme-border-strong)] border-t-[var(--theme-accent-primary)]"></div>
      </div>
      <div v-else-if="!requestHasMore && store.requestRecords.length > 0" class="py-2 text-center text-[10.5px] text-[var(--theme-text-quaternary)]">
        {{ t(locale, 'common.noMore') }}
      </div>
      <div ref="requestLoadMoreTrigger" class="h-1 w-full"></div>
    </template>
  </div>
</template>

<style scoped>
.drawer-slide-enter-active,
.drawer-slide-leave-active {
  transition: transform 0.18s ease-out, opacity 0.18s ease-out;
}
.drawer-slide-enter-from,
.drawer-slide-leave-to {
  transform: translateX(12px);
  opacity: 0;
}
</style>
