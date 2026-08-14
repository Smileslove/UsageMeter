<script setup lang="ts">
/**
 * 桌面主窗口「活动」页（设计文档第 9 章：三栏布局 9.2 / 左栏 9.3 / 时间线 9.4 /
 * 工具卡 9.5 / 子代理 9.6 / 检查器 9.7 / 搜索 9.8 / 请求关联 9.9）。
 *
 * - 顶部：会话选择器（搜索/下拉，数据来自 monitor store.sessions）+ 能力状态徽标；
 * - 数据流：选中会话 → get_session_activity_summary（null 且索引未建时先
 *   rebuild_session_activity_index("session:<key>") 再查）→ 分页 100/页滚动加载事件；
 *   隐私 off（deep_index_level='off'）时后端返回空，显示“深度活动未启用”+ 去设置；
 * - 三栏布局：≥1180px 左 240px + 中 min 480px + 右 360px（可折叠）；
 *   960-1179px 左栏折叠为顶部按钮、右栏 overlay drawer；最小宽度无横向滚动；
 * - 代理树：relationLevel=fullTree 才画父子树，rootGrouped/flagOnly 显示分组列表不画伪树（9.6 铁律）；
 * - 请求关联：strength=exact/sourceExplicit 实线 +“精确关联”，timeWindow 虚线 +“推测关联”（9.9）；
 * - 事件正文/工具输入输出一律来自后端脱敏 payload，前端不二次处理敏感逻辑、不使用 v-html。
 *
 * fixedSessionKey prop：会话工作区（SessionWorkspace 活动 tab）内嵌模式——隐藏顶部
 * 会话选择器并固定到该会话，其余逻辑与自由模式完全一致。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import {
  CheckCircle2, ChevronDown, ChevronLeft, ChevronRight, CircleHelp, Copy, Download,
  Eye, EyeOff, FileOutput, GitBranch, Globe, Info, Link2, Loader2, MessagesSquare,
  PanelLeft, RefreshCw, Search, Shrink, TriangleAlert, User, Wrench, X, FolderOpen, Bot, Clock, Layers
} from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t, backendErrorLabel } from '../../i18n'
import { formatDurationMs } from '../../utils/format'
import {
  exportSessionActivity, getSessionActivitySummary, getSessionAgents, getSessionEvents,
  getSessionEventPayload, getSessionToolSummary, rebuildSessionActivityIndex,
  searchActivityGlobal, searchSessionActivity
} from '../../api/activityApi'
import type {
  AgentNodeDto, ExportResult, GlobalSearchHit, RedactedPayloadPage,
  SessionActivitySummary, SessionEventFilter, SessionEventKind, SessionEventListItem,
  SessionStats, ToolSummaryRow
} from '../../types'

const props = defineProps<{
  /** 会话工作区内嵌模式：固定到指定会话并隐藏顶部会话选择器。 */
  fixedSessionKey?: string
}>()

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

// ============ 布局断点（设计 9.2）：≥1180px 三栏，否则左栏按钮 + 右栏 drawer ============
const wideQuery = window.matchMedia('(min-width: 1180px)')
const wideMode = ref(wideQuery.matches)
const onWideChange = (event: MediaQueryListEvent) => { wideMode.value = event.matches }
const sidebarOpen = ref(false)          // 窄屏左栏 overlay
const inspectorOpen = ref(false)        // 窄屏检查器 drawer
const inspectorCollapsed = ref(false)   // 宽屏检查器折叠
wideQuery.addEventListener('change', onWideChange)

// ============ 会话选择器（自由模式；数据来自 monitor store.sessions） ============
const sessionPickerOpen = ref(false)
const sessionSearch = ref('')
const sessionsLoading = ref(false)
const sessionsAllLoaded = ref(false)
let sessionsOffset = 0

const sessionTitle = (session: SessionStats) =>
  session.topic?.trim()
  || session.sessionName?.trim()
  || session.lastPrompt?.trim()
  || session.projectName?.trim()
  || t(locale.value, 'sessions.untitled')

const pickerSessions = computed(() => {
  const query = sessionSearch.value.trim().toLowerCase()
  if (!query) return store.sessions
  return store.sessions.filter(s =>
    sessionTitle(s).toLowerCase().includes(query)
    || (s.projectName ?? '').toLowerCase().includes(query)
    || (s.cwd ?? '').toLowerCase().includes(query)
  )
})

const loadMoreSessions = async () => {
  if (sessionsLoading.value || sessionsAllLoaded.value) return
  sessionsLoading.value = true
  try {
    const count = await store.fetchSessions(100, sessionsOffset, true)
    sessionsOffset += count
    if (count < 100) sessionsAllLoaded.value = true
  } finally {
    sessionsLoading.value = false
  }
}

const ensureSessions = async () => {
  if (store.sessions.length > 0) return
  const count = await store.fetchSessions(100, 0, false)
  sessionsOffset = count
  if (count < 100) sessionsAllLoaded.value = true
}

const selectSession = (session: SessionStats) => {
  sessionPickerOpen.value = false
  void loadSession(session.sessionId)
}

const currentSession = computed(() =>
  store.sessions.find(s => s.sessionId === activeSessionKey.value) ?? null
)

// ============ 活动数据状态 ============
const ALL_KINDS: SessionEventKind[] = [
  'userMessage', 'assistantMessage', 'toolInvocation', 'toolResult',
  'agentStarted', 'agentFinished', 'systemEvent', 'compaction', 'error', 'unknown'
]
const PAGE_SIZE = 100

type ViewState =
  | 'no-session' | 'privacy-off' | 'loading' | 'aggregate-only' | 'no-data' | 'error' | 'ready'

const activeSessionKey = ref<string | null>(null)
const summary = ref<SessionActivitySummary | null>(null)
const agents = ref<AgentNodeDto[]>([])
const toolSummary = ref<ToolSummaryRow[]>([])
const events = ref<SessionEventListItem[]>([])
const total = ref(0)
const hasMore = ref(false)
const loading = ref(false)
const loadingMore = ref(false)
const building = ref(false)
const error = ref('')
const viewState = ref<ViewState>('no-session')

const selectedKinds = ref<Set<SessionEventKind>>(new Set(ALL_KINDS))
const activeAgentKey = ref<string | null>(null)

let eventsOffset = 0
let loadGeneration = 0

const deepIndexEnabled = computed(() => store.settings.deepIndexLevel !== 'off')
const capabilityLevel = computed(() => summary.value?.capability.level ?? null)
const relationLevel = computed(() => summary.value?.capability.agentRelations ?? 'none')

const capabilityBadge = computed<'off' | 'none' | 'metadata' | 'structured' | 'fullContent' | null>(() => {
  if (!deepIndexEnabled.value) return 'off'
  return capabilityLevel.value
})

/** 顶部能力状态徽标（Structured / Off / 仅聚合…）。 */
const badgeMeta = computed<{ label: string; cls: string } | null>(() => {
  switch (capabilityBadge.value) {
    case 'off':
      return { label: t(locale.value, 'desktop.activity.capabilityOff'), cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300' }
    case 'none':
    case 'metadata':
      return { label: t(locale.value, 'desktop.activity.capabilityAggregate'), cls: 'bg-amber-500/10 text-amber-600 dark:text-amber-300' }
    case 'structured':
      return { label: t(locale.value, 'desktop.activity.capabilityStructured'), cls: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300' }
    case 'fullContent':
      return { label: t(locale.value, 'desktop.activity.capabilityOnDemand'), cls: 'bg-sky-500/10 text-sky-600 dark:text-sky-300' }
    default:
      return null
  }
})

/** 全文索引档启用（M3：会话内与跨会话搜索仅 fulltext 档可用，其余档位禁用搜索执行）。 */
const fulltextEnabled = computed(() => store.settings.deepIndexLevel === 'fulltext')

// ============ 会话内全文搜索（设计 9.8：命中列表 + 上一项/下一项；清除回到时间线） ============
const searchInput = ref('')
const searchMode = ref(false)
const searchResults = ref<SessionEventListItem[]>([])
const searchTotal = ref(0)
const searchHasMore = ref(false)
const searchLoading = ref(false)
const searchError = ref('')
const searchActiveIndex = ref(-1)
let searchOffset = 0
let searchGeneration = 0
let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null

const runSearch = async (reset: boolean) => {
  const key = activeSessionKey.value
  const query = searchInput.value.trim()
  if (!key || !fulltextEnabled.value || !query) return
  const generation = ++searchGeneration
  if (reset) {
    searchOffset = 0
    searchResults.value = []
    searchTotal.value = 0
    searchHasMore.value = false
    searchError.value = ''
    searchActiveIndex.value = -1
    searchMode.value = true
  }
  searchLoading.value = true
  try {
    const page = await searchSessionActivity(store.settings, key, query, searchOffset, 100)
    if (generation !== searchGeneration) return
    searchResults.value = reset ? page.items : [...searchResults.value, ...page.items]
    searchTotal.value = page.total
    searchHasMore.value = page.hasMore
    searchOffset += page.items.length
  } catch (e) {
    if (generation !== searchGeneration) return
    searchError.value = e instanceof Error ? e.message : String(e)
  } finally {
    if (generation === searchGeneration) searchLoading.value = false
  }
}

const clearSearch = () => {
  searchGeneration += 1
  searchInput.value = ''
  searchMode.value = false
  searchResults.value = []
  searchTotal.value = 0
  searchHasMore.value = false
  searchLoading.value = false
  searchError.value = ''
  searchActiveIndex.value = -1
  searchOffset = 0
}

/** 回车立即搜索；输入防抖自动搜索（400ms）。 */
const triggerSearch = () => {
  if (searchDebounceTimer) {
    clearTimeout(searchDebounceTimer)
    searchDebounceTimer = null
  }
  if (!searchInput.value.trim()) {
    clearSearch()
    return
  }
  void runSearch(true)
}

watch(searchInput, () => {
  if (!fulltextEnabled.value) return
  if (searchDebounceTimer) clearTimeout(searchDebounceTimer)
  searchDebounceTimer = setTimeout(() => {
    searchDebounceTimer = null
    if (!searchInput.value.trim()) {
      clearSearch()
      return
    }
    void runSearch(true)
  }, 400)
})

/** 上一项/下一项导航（循环），选中并滚动到可见区。 */
const searchStep = (delta: number) => {
  const n = searchResults.value.length
  if (n === 0) return
  const base = searchActiveIndex.value < 0 ? (delta > 0 ? -1 : 0) : searchActiveIndex.value
  const next = (base + delta + n) % n
  searchActiveIndex.value = next
  selectEvent(searchResults.value[next])
  requestAnimationFrame(() => {
    document.getElementById(`search-hit-${next}`)?.scrollIntoView({ block: 'nearest' })
  })
}

// ============ 跨会话全文搜索（9.8：入口在会话选择器旁，结果跳转该会话活动页） ============
const globalSearchOpen = ref(false)
const globalQuery = ref('')
const globalResults = ref<GlobalSearchHit[]>([])
const globalTotal = ref(0)
const globalHasMore = ref(false)
const globalLoading = ref(false)
const globalError = ref('')
let globalOffset = 0
let globalGeneration = 0

const runGlobalSearch = async (reset: boolean) => {
  const query = globalQuery.value.trim()
  if (!query) return
  const generation = ++globalGeneration
  if (reset) {
    globalOffset = 0
    globalResults.value = []
    globalTotal.value = 0
    globalHasMore.value = false
    globalError.value = ''
  }
  globalLoading.value = true
  try {
    const page = await searchActivityGlobal(store.settings, query, globalOffset, 50)
    if (generation !== globalGeneration) return
    globalResults.value = reset ? page.items : [...globalResults.value, ...page.items]
    globalTotal.value = page.total
    globalHasMore.value = page.hasMore
    globalOffset += page.items.length
  } catch (e) {
    if (generation !== globalGeneration) return
    globalError.value = e instanceof Error ? e.message : String(e)
  } finally {
    if (generation === globalGeneration) globalLoading.value = false
  }
}

const triggerGlobalSearch = () => {
  if (globalQuery.value.trim()) void runGlobalSearch(true)
}

/** 点击命中：跳转到该会话的活动页（#/desktop/activity/<key>，由下方 nav.activeSessionKey watch 消费）。 */
const jumpToSession = (hit: GlobalSearchHit) => {
  globalSearchOpen.value = false
  nav.openActivity(hit.sessionKey)
}

const hitKind = (kind: string): SessionEventKind =>
  kind in KIND_META ? (kind as SessionEventKind) : 'unknown'

// ============ 导出会话活动（15.3 / 21.5：范围预览，默认不勾选正文与工具 payload） ============
const exportDialogOpen = ref(false)
const exportFormat = ref<'json' | 'csv'>('json')
const exportIncludeSummaries = ref(true)
const exportIncludeToolSummaries = ref(true)
const exportIncludeRequestLinks = ref(true)
const exportIncludePayloads = ref(false)
const exportBusy = ref(false)
const exportError = ref('')
const exportResult = ref<ExportResult | null>(null)
const exportCopiedFlash = ref(false)
let exportCopiedTimer: ReturnType<typeof setTimeout> | null = null

const openExportDialog = () => {
  exportDialogOpen.value = true
  exportResult.value = null
  exportError.value = ''
}
const closeExportDialog = () => {
  if (exportBusy.value) return
  exportDialogOpen.value = false
}
const runExport = async () => {
  const key = activeSessionKey.value
  if (!key || exportBusy.value) return
  exportBusy.value = true
  exportError.value = ''
  exportResult.value = null
  try {
    exportResult.value = await exportSessionActivity(key, {
      format: exportFormat.value,
      includeSummaries: exportIncludeSummaries.value,
      includeToolSummaries: exportIncludeToolSummaries.value,
      includeRequestLinks: exportIncludeRequestLinks.value,
      includePayloads: exportIncludePayloads.value
    })
  } catch (e) {
    exportError.value = e instanceof Error ? e.message : String(e)
  } finally {
    exportBusy.value = false
  }
}
const copyExportPath = async () => {
  if (!exportResult.value) return
  try {
    await navigator.clipboard.writeText(exportResult.value.filePath)
    exportCopiedFlash.value = true
    if (exportCopiedTimer) clearTimeout(exportCopiedTimer)
    exportCopiedTimer = setTimeout(() => { exportCopiedFlash.value = false }, 1400)
  } catch { /* 剪贴板不可用时静默失败 */ }
}

const buildFilter = (): SessionEventFilter | null => {
  const kinds = [...selectedKinds.value]
  if (kinds.length === ALL_KINDS.length && !activeAgentKey.value) return null
  const filter: SessionEventFilter = {}
  if (kinds.length !== ALL_KINDS.length) filter.kinds = kinds
  if (activeAgentKey.value) filter.agents = [activeAgentKey.value]
  return Object.keys(filter).length ? filter : null
}

const resetData = () => {
  loadGeneration += 1
  summary.value = null
  agents.value = []
  toolSummary.value = []
  events.value = []
  total.value = 0
  hasMore.value = false
  error.value = ''
  eventsOffset = 0
}

const loadEvents = async (reset: boolean) => {
  const key = activeSessionKey.value
  if (!key || !deepIndexEnabled.value) return
  const generation = loadGeneration
  if (reset) {
    eventsOffset = 0
    events.value = []
    total.value = 0
    loading.value = true
  } else {
    loadingMore.value = true
  }
  try {
    const page = await getSessionEvents(store.settings, key, buildFilter(), eventsOffset, PAGE_SIZE)
    if (generation !== loadGeneration) return
    events.value = reset ? page.items : [...events.value, ...page.items]
    total.value = page.total
    hasMore.value = page.hasMore
    eventsOffset += page.items.length
  } catch (e) {
    if (generation !== loadGeneration) return
    error.value = e instanceof Error ? e.message : String(e)
    if (reset) viewState.value = 'error'
  } finally {
    if (generation === loadGeneration) {
      loading.value = false
      loadingMore.value = false
    }
  }
}

const loadSession = async (key: string | null) => {
  loadGeneration += 1
  activeSessionKey.value = key
  clearSearch()
  resetData()
  if (!key) {
    viewState.value = 'no-session'
    return
  }
  if (!deepIndexEnabled.value) {
    viewState.value = 'privacy-off'
    return
  }
  viewState.value = 'loading'
  const generation = loadGeneration
  try {
    let s = await getSessionActivitySummary(store.settings, key)
    if (generation !== loadGeneration) return
    // 懒索引后仍无深度索引 → 自动重建一次再查。
    if (!s) {
      building.value = true
      try {
        await rebuildSessionActivityIndex(`session:${key}`)
        if (generation !== loadGeneration) return
        s = await getSessionActivitySummary(store.settings, key)
      } finally {
        building.value = false
      }
    }
    if (generation !== loadGeneration) return
    summary.value = s
    if (!s) {
      viewState.value = 'no-data'
      return
    }
    if (s.capability.level === 'none') {
      viewState.value = 'aggregate-only'
      return
    }
    viewState.value = 'ready'
    void Promise.all([
      getSessionAgents(key).then(list => {
        if (generation === loadGeneration) agents.value = list
      }).catch(() => {}),
      getSessionToolSummary(key).then(list => {
        if (generation === loadGeneration) toolSummary.value = list
      }).catch(() => {})
    ])
    await loadEvents(true)
  } catch (e) {
    if (generation !== loadGeneration) return
    error.value = e instanceof Error ? e.message : String(e)
    viewState.value = 'error'
  } finally {
    if (generation === loadGeneration) loading.value = false
  }
}

const rebuildAndReload = async () => {
  const key = activeSessionKey.value
  if (!key || !deepIndexEnabled.value) return
  building.value = true
  error.value = ''
  try {
    await rebuildSessionActivityIndex(`session:${key}`)
    await loadSession(key)
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
    viewState.value = 'error'
  } finally {
    building.value = false
  }
}

const retry = () => {
  if (activeSessionKey.value) void loadSession(activeSessionKey.value)
}

// 过滤变化 → 重置分页（后端 SQL 过滤 + 分页）
watch(selectedKinds, () => {
  if (viewState.value === 'ready') void loadEvents(true)
})
watch(activeAgentKey, () => {
  if (viewState.value === 'ready') void loadEvents(true)
})

const toggleKindGroup = (kinds: SessionEventKind[]) => {
  const next = new Set(selectedKinds.value)
  const allSelected = kinds.every(k => next.has(k))
  for (const kind of kinds) {
    if (allSelected) next.delete(kind)
    else next.add(kind)
  }
  selectedKinds.value = next
}
const groupChecked = (kinds: SessionEventKind[]) => kinds.every(k => selectedKinds.value.has(k))

/** 事件类型过滤组（用户/模型/工具/子代理/错误/系统）。 */
const KIND_GROUPS: Array<{ id: string; kinds: SessionEventKind[]; labelKey: string }> = [
  { id: 'user', kinds: ['userMessage'], labelKey: 'desktop.activity.filterUser' },
  { id: 'model', kinds: ['assistantMessage'], labelKey: 'desktop.activity.filterModel' },
  { id: 'tool', kinds: ['toolInvocation', 'toolResult'], labelKey: 'desktop.activity.filterTool' },
  { id: 'agent', kinds: ['agentStarted', 'agentFinished'], labelKey: 'desktop.activity.filterAgent' },
  { id: 'error', kinds: ['error'], labelKey: 'desktop.activity.filterError' },
  { id: 'system', kinds: ['systemEvent', 'compaction', 'unknown'], labelKey: 'desktop.activity.filterSystem' }
]

// ============ 代理树（设计 9.3 / 9.6：fullTree 才画树，其余按分组列表） ============
interface AgentRow { node: AgentNodeDto; depth: number }

const agentRows = computed<AgentRow[]>(() => {
  const list = agents.value
  if (list.length === 0) return []
  if (relationLevel.value === 'fullTree') {
    const byParent = new Map<string | null, AgentNodeDto[]>()
    for (const node of list) {
      const key = node.parentAgentKey ?? null
      const bucket = byParent.get(key) ?? []
      bucket.push(node)
      byParent.set(key, bucket)
    }
    const rows: AgentRow[] = []
    const walk = (parent: string | null, depth: number) => {
      const children = byParent.get(parent) ?? []
      for (const node of children) {
        rows.push({ node, depth })
        walk(node.agentKey, depth + 1)
      }
    }
    walk(null, 0)
    return rows
  }
  // rootGrouped / flagOnly / none：分组列表，不画伪树
  return list.map(node => ({ node, depth: 0 }))
})

const unlinkedAgents = computed(() =>
  relationLevel.value !== 'fullTree' ? agents.value : []
)

const agentStatusKey = (status: AgentNodeDto['status']) => {
  switch (status) {
    case 'success': return 'desktop.activity.statusSuccess'
    case 'error': return 'desktop.activity.statusError'
    case 'running': return 'desktop.activity.statusRunning'
    case 'pending': return 'desktop.activity.statusPending'
    case 'cancelled': return 'desktop.activity.statusCancelled'
    default: return 'desktop.activity.statusUnknown'
  }
}

const agentStatusMeta = (status: AgentNodeDto['status']) => {
  switch (status) {
    case 'success': return 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'
    case 'error': return 'bg-rose-500/10 text-rose-600 dark:text-rose-300'
    case 'running': return 'bg-sky-500/10 text-sky-600 dark:text-sky-300'
    case 'cancelled': return 'bg-slate-500/10 text-slate-500 dark:text-slate-300'
    default: return 'bg-slate-500/10 text-slate-500 dark:text-slate-300'
  }
}
const shortAgentId = (key: string) => (key.length > 14 ? `${key.slice(0, 8)}…${key.slice(-4)}` : key)

// ============ 时间线展示 ============
const formatAbsTime = (ms: number) =>
  new Date(ms).toLocaleString(locale.value.replace('_', '-'), {
    year: 'numeric', month: 'short', day: 'numeric',
    hour: '2-digit', minute: '2-digit', second: '2-digit'
  })

const formatRelTime = (ms: number) => {
  const diff = Date.now() - ms
  const mins = Math.floor(diff / 60000)
  if (mins < 1) return t(locale.value, 'common.justNow')
  if (mins < 60) return t(locale.value, 'sessions.timeMinutesAgo', { count: mins })
  const hours = Math.floor(mins / 60)
  if (hours < 24) return t(locale.value, 'sessions.timeHoursAgo', { count: hours })
  return formatAbsTime(ms)
}

interface KindMeta { icon: typeof User; cls: string; labelKey: string }
const KIND_META: Record<SessionEventKind, KindMeta> = {
  userMessage: { icon: User, cls: 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-300', labelKey: 'desktop.activity.kindUserMessage' },
  assistantMessage: { icon: MessagesSquare, cls: 'bg-violet-500/10 text-violet-600 dark:text-violet-300', labelKey: 'desktop.activity.kindAssistantMessage' },
  toolInvocation: { icon: Wrench, cls: 'bg-cyan-500/10 text-cyan-600 dark:text-cyan-300', labelKey: 'desktop.activity.kindToolInvocation' },
  toolResult: { icon: FileOutput, cls: 'bg-teal-500/10 text-teal-600 dark:text-teal-300', labelKey: 'desktop.activity.kindToolResult' },
  agentStarted: { icon: GitBranch, cls: 'bg-fuchsia-500/10 text-fuchsia-600 dark:text-fuchsia-300', labelKey: 'desktop.activity.kindAgentStarted' },
  agentFinished: { icon: GitBranch, cls: 'bg-fuchsia-500/10 text-fuchsia-600 dark:text-fuchsia-300', labelKey: 'desktop.activity.kindAgentFinished' },
  systemEvent: { icon: Info, cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300', labelKey: 'desktop.activity.kindSystemEvent' },
  compaction: { icon: Shrink, cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300', labelKey: 'desktop.activity.kindCompaction' },
  error: { icon: TriangleAlert, cls: 'bg-rose-500/10 text-rose-600 dark:text-rose-300', labelKey: 'desktop.activity.kindError' },
  unknown: { icon: CircleHelp, cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300', labelKey: 'desktop.activity.kindUnknown' }
}

const statusMeta = (status: SessionEventListItem['status']) => {
  switch (status) {
    case 'success':
      return { labelKey: 'desktop.activity.statusSuccess', cls: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300' }
    case 'error':
      return { labelKey: 'desktop.activity.statusError', cls: 'bg-rose-500/10 text-rose-600 dark:text-rose-300' }
    case 'running':
      return { labelKey: 'desktop.activity.statusRunning', cls: 'bg-sky-500/10 text-sky-600 dark:text-sky-300' }
    case 'pending':
      return { labelKey: 'desktop.activity.statusPending', cls: 'bg-amber-500/10 text-amber-600 dark:text-amber-300' }
    case 'cancelled':
      return { labelKey: 'desktop.activity.statusCancelled', cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300' }
    default:
      return null
  }
}

const formatDuration = (ms: number | null | undefined) =>
  ms == null ? '—' : formatDurationMs(ms)

const formatBytes = (bytes: number | null | undefined) => {
  if (bytes == null) return '—'
  if (bytes < 1024) return `${bytes}B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`
  return `${(bytes / (1024 * 1024)).toFixed(2)}MB`
}

// ============ 检查器（设计 9.7） ============
const selectedEvent = ref<SessionEventListItem | null>(null)
const payloadSection = ref<'summary' | 'input' | 'output'>('summary')
const PAYLOAD_SECTIONS: readonly ('summary' | 'input' | 'output')[] = [
  'summary',
  'input',
  'output'
]
const payloadSections = computed<readonly ('summary' | 'input' | 'output')[]>(() =>
  selectedEvent.value?.tool ? PAYLOAD_SECTIONS : (['summary'] as const)
)
const payload = ref<RedactedPayloadPage | null>(null)
const payloadLoading = ref(false)
const payloadError = ref('')
const copiedFlash = ref(false)
let copiedTimer: ReturnType<typeof setTimeout> | null = null

const selectEvent = (event: SessionEventListItem) => {
  selectedEvent.value = event
  if (!wideMode.value) inspectorOpen.value = true
}

const closeInspector = () => {
  if (!wideMode.value) inspectorOpen.value = false
  selectedEvent.value = null
  payload.value = null
}

const loadPayload = async () => {
  const event = selectedEvent.value
  if (!event) return
  payloadLoading.value = true
  payloadError.value = ''
  payload.value = null
  try {
    payload.value = await getSessionEventPayload(event.eventKey, payloadSection.value)
  } catch (e) {
    payloadError.value = e instanceof Error ? e.message : String(e)
  } finally {
    payloadLoading.value = false
  }
}

/** M3 payload 分页（设计 12.5：nextCursor 非空时续读，append 到内容区）。 */
const loadMorePayload = async () => {
  const event = selectedEvent.value
  const page = payload.value
  if (!event || !page || !page.nextCursor || payloadLoading.value) return
  payloadLoading.value = true
  payloadError.value = ''
  try {
    const next = await getSessionEventPayload(event.eventKey, payloadSection.value, undefined, page.nextCursor)
    payload.value = { ...next, content: page.content + next.content }
  } catch (e) {
    payloadError.value = e instanceof Error ? e.message : String(e)
  } finally {
    payloadLoading.value = false
  }
}

watch(selectedEvent, event => {
  payload.value = null
  payloadError.value = ''
  if (event) {
    payloadSection.value = event.tool ? 'input' : 'summary'
    void loadPayload()
  }
})

const setPayloadSection = (section: 'summary' | 'input' | 'output') => {
  payloadSection.value = section
  void loadPayload()
}

const copyEventId = async () => {
  const event = selectedEvent.value
  if (!event) return
  try {
    await navigator.clipboard.writeText(event.eventKey)
    copiedFlash.value = true
    if (copiedTimer) clearTimeout(copiedTimer)
    copiedTimer = setTimeout(() => { copiedFlash.value = false }, 1400)
  } catch { /* 剪贴板不可用时静默失败 */ }
}

const linkMeta = (strength: string) =>
  strength === 'timeWindow'
    ? { labelKey: 'desktop.activity.linkInferred', dashed: true }
    : { labelKey: 'desktop.activity.linkExact', dashed: false }

const contentStateLabel = (state: RedactedPayloadPage['contentState']) => {
  switch (state) {
    case 'available': return { key: 'desktop.activity.contentStateAvailable', cls: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300' }
    case 'redacted': return { key: 'desktop.activity.contentStateRedacted', cls: 'bg-amber-500/10 text-amber-600 dark:text-amber-300' }
    case 'truncated': return { key: 'desktop.activity.contentStateTruncated', cls: 'bg-sky-500/10 text-sky-600 dark:text-sky-300' }
    case 'unavailable': return { key: 'desktop.activity.contentStateUnavailable', cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300' }
    default: return { key: 'desktop.activity.contentStateNone', cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300' }
  }
}

// ============ 滚动加载（IntersectionObserver，sentinel 触发） ============
const sentinel = ref<HTMLElement | null>(null)
let observer: IntersectionObserver | null = null
watch(sentinel, element => {
  if (!observer || !element) return
  observer.observe(element)
})

// ============ 生命周期 ============
onMounted(() => {
  observer = new IntersectionObserver(entries => {
    if (entries[0]?.isIntersecting && viewState.value === 'ready'
      && hasMore.value && !loadingMore.value && !loading.value) {
      void loadEvents(false)
    }
  }, { rootMargin: '240px' })
  if (sentinel.value) observer.observe(sentinel.value)

  if (props.fixedSessionKey) {
    void loadSession(props.fixedSessionKey)
  } else if (nav.activeSessionKey) {
    // 深链/跨会话搜索跳转：#/desktop/activity/<key>
    void loadSession(nav.activeSessionKey)
  } else {
    void ensureSessions()
  }
})

watch(() => props.fixedSessionKey, key => {
  if (key) void loadSession(key)
})

// 自由模式：消费 hash 路由携带的会话（跨会话搜索结果跳转、#/desktop/activity/<key> 深链）。
watch(() => nav.activeSessionKey, key => {
  if (!props.fixedSessionKey && key && key !== activeSessionKey.value) {
    void loadSession(key)
  }
})

onUnmounted(() => {
  observer?.disconnect()
  wideQuery.removeEventListener('change', onWideChange)
  if (copiedTimer) clearTimeout(copiedTimer)
  if (searchDebounceTimer) clearTimeout(searchDebounceTimer)
  if (exportCopiedTimer) clearTimeout(exportCopiedTimer)
})

// 固定模式：无会话选择器，但仍在顶部展示能力徽标
const showPicker = computed(() => !props.fixedSessionKey)
</script>

<template>
  <section class="flex min-w-0 flex-col gap-3 pb-4">
    <!-- ============ 顶部：会话选择器 + 能力徽标 ============ -->
    <div class="flex flex-wrap items-center gap-2">
      <!-- 会话选择器（自由模式） -->
      <div v-if="showPicker" class="relative min-w-0 flex-1">
        <button
          type="button"
          class="theme-surface flex h-9 w-full max-w-md items-center gap-2 rounded-lg border px-3 text-left text-[12px] font-medium text-[var(--theme-text-primary)]"
          :aria-label="t(locale, 'desktop.activity.selectSessionLabel')"
          :title="t(locale, 'desktop.activity.selectSessionLabel')"
          :aria-expanded="sessionPickerOpen"
          @click="sessionPickerOpen = !sessionPickerOpen"
        >
          <Search class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
          <span class="min-w-0 flex-1 truncate">
            <template v-if="currentSession">{{ sessionTitle(currentSession) }}</template>
            <template v-else class="text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.selectSessionLabel') }}</template>
          </span>
          <ChevronDown class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        </button>

        <!-- 下拉：搜索 + 会话列表 -->
        <div
          v-if="sessionPickerOpen"
          class="theme-surface-elevated absolute left-0 top-10 z-50 w-full max-w-md rounded-xl border p-1.5 shadow-lg"
        >
          <div class="flex items-center gap-2 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-1.5">
            <Search class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <input
              v-model="sessionSearch"
              type="text"
              class="min-w-0 flex-1 bg-transparent text-[12px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]"
              :placeholder="t(locale, 'desktop.activity.sessionPickerPlaceholder')"
            />
          </div>
          <div class="mt-1 max-h-64 overflow-y-auto">
            <button
              v-for="session in pickerSessions"
              :key="session.sessionId"
              type="button"
              class="flex w-full flex-col gap-0.5 rounded-lg px-2.5 py-2 text-left transition-colors hover:bg-[var(--theme-bg-hover)]"
              :class="activeSessionKey === session.sessionId ? 'bg-[var(--theme-accent-soft)]' : ''"
              @click="selectSession(session)"
            >
              <span class="truncate text-[12px] font-medium text-[var(--theme-text-primary)]">{{ sessionTitle(session) }}</span>
              <span class="truncate font-mono text-[10px] text-[var(--theme-text-quaternary)]">{{ session.projectName || session.sessionId }}</span>
            </button>
            <div v-if="pickerSessions.length === 0" class="px-2.5 py-6 text-center text-[11px] text-[var(--theme-text-tertiary)]">
              {{ t(locale, 'desktop.activity.sessionPickerEmpty') }}
            </div>
            <button
              v-if="!sessionsAllLoaded"
              type="button"
              class="mt-1 flex w-full items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-semibold text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]"
              :disabled="sessionsLoading"
              @click="loadMoreSessions"
            >
              <Loader2 v-if="sessionsLoading" class="h-3 w-3 animate-spin" aria-hidden="true" />
              {{ t(locale, 'desktop.activity.sessionsLoadMore') }}
            </button>
          </div>
        </div>
      </div>
      <div v-else class="min-w-0 flex-1" />

      <!-- 跨会话搜索（9.8：fulltext 档可用；结果跳转该会话活动页） -->
      <div v-if="showPicker" class="relative shrink-0">
        <button
          type="button"
          class="theme-button-secondary inline-flex h-9 items-center gap-1.5 rounded-lg px-3 text-[12px] font-semibold"
          :aria-label="t(locale, 'desktop.activity.globalSearch')"
          :title="t(locale, 'desktop.activity.globalSearch')"
          :aria-expanded="globalSearchOpen"
          @click="globalSearchOpen = !globalSearchOpen"
        >
          <Globe class="h-3.5 w-3.5" aria-hidden="true" />
          <span class="hidden md:inline">{{ t(locale, 'desktop.activity.globalSearch') }}</span>
        </button>

        <div
          v-if="globalSearchOpen"
          class="theme-surface-elevated absolute right-0 top-11 z-50 w-[min(560px,90vw)] rounded-xl border p-2 shadow-lg"
          :aria-label="t(locale, 'desktop.activity.globalSearch')"
        >
          <div class="flex items-center gap-2 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-1.5">
            <Search class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <input
              v-model="globalQuery"
              type="text"
              class="min-w-0 flex-1 bg-transparent text-[12px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]"
              :placeholder="t(locale, 'desktop.activity.globalSearchPlaceholder')"
              @keydown.enter="triggerGlobalSearch"
            />
            <button
              type="button"
              class="rounded p-1 text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)]"
              :aria-label="t(locale, 'desktop.activity.globalSearch')"
              :disabled="globalLoading"
              @click="triggerGlobalSearch"
            >
              <Loader2 v-if="globalLoading" class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
              <Search v-else class="h-3.5 w-3.5" aria-hidden="true" />
            </button>
          </div>
          <div class="mt-1.5 max-h-72 overflow-y-auto">
            <div v-if="globalLoading && globalResults.length === 0" class="py-6 text-center text-[11px] text-[var(--theme-text-tertiary)]">
              {{ t(locale, 'desktop.activity.timelineLoading') }}
            </div>
            <div v-else-if="globalError" class="px-2.5 py-4 text-center">
              <p class="break-all text-[10.5px] text-rose-500">{{ backendErrorLabel(locale, globalError) }}</p>
              <button
                type="button"
                class="theme-button-secondary mt-2 inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[11px] font-semibold"
                @click="triggerGlobalSearch"
              >
                <RefreshCw class="h-3 w-3" aria-hidden="true" />
                {{ t(locale, 'desktop.activity.retry') }}
              </button>
            </div>
            <div v-else-if="globalResults.length === 0 && globalQuery.trim()" class="px-2.5 py-6 text-center text-[11px] text-[var(--theme-text-tertiary)]">
              {{ t(locale, 'desktop.activity.globalSearchEmpty') }}
            </div>
            <template v-else>
              <div v-if="globalTotal > 0" class="px-2 pt-1 text-[10px] font-semibold text-[var(--theme-accent-primary)]">
                {{ t(locale, 'desktop.activity.searchHitCount', { count: globalTotal }) }}
              </div>
              <button
                v-for="hit in globalResults"
                :key="hit.eventKey"
                type="button"
                class="flex w-full flex-col gap-1 rounded-lg px-2.5 py-2 text-left transition-colors hover:bg-[var(--theme-bg-hover)]"
                @click="jumpToSession(hit)"
              >
                <span class="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                  <span class="min-w-0 flex-1 truncate text-[11px] font-semibold text-[var(--theme-text-primary)]">{{ hit.sessionTitle || t(locale, 'sessions.untitled') }}</span>
                  <span class="shrink-0 rounded px-1.5 py-px text-[9px] font-bold leading-none" :class="KIND_META[hitKind(hit.kind)].cls">
                    {{ t(locale, KIND_META[hitKind(hit.kind)].labelKey) }}
                  </span>
                  <span v-if="hit.toolName" class="shrink-0 inline-flex items-center gap-1 rounded bg-[var(--theme-border-subtle)] px-1.5 py-px font-mono text-[9px] text-[var(--theme-text-tertiary)]">
                    <Wrench class="h-2.5 w-2.5" aria-hidden="true" />
                    {{ hit.toolName }}
                  </span>
                  <span v-if="hit.timestampMs != null" class="ml-auto shrink-0 text-[9.5px] text-[var(--theme-text-quaternary)]">{{ formatRelTime(hit.timestampMs) }}</span>
                </span>
                <span v-if="hit.summary" class="line-clamp-2 break-words text-[11px] leading-relaxed text-[var(--theme-text-secondary)]">{{ hit.summary }}</span>
                <span class="inline-flex items-center gap-0.5 text-[9.5px] font-semibold text-[var(--theme-accent-primary)]">
                  {{ t(locale, 'desktop.activity.globalSearchJump') }}
                  <ChevronRight class="h-3 w-3" aria-hidden="true" />
                </span>
              </button>
              <button
                v-if="globalHasMore"
                type="button"
                class="mt-1 flex w-full items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[10.5px] font-semibold text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]"
                :disabled="globalLoading"
                @click="runGlobalSearch(false)"
              >
                <Loader2 v-if="globalLoading" class="h-3 w-3 animate-spin" aria-hidden="true" />
                {{ t(locale, 'desktop.activity.loadMore') }}
              </button>
            </template>
          </div>
        </div>
      </div>

      <!-- 能力状态徽标 -->
      <span
        v-if="badgeMeta"
        class="inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[10.5px] font-bold leading-none"
        :class="badgeMeta.cls"
      >
        <span class="h-1.5 w-1.5 rounded-full bg-current" aria-hidden="true"></span>
        {{ badgeMeta.label }}
      </span>

      <!-- 窄屏：左栏折叠按钮 + 检查器按钮 -->
      <div v-if="!wideMode && viewState === 'ready'" class="flex shrink-0 items-center gap-1.5">
        <button
          type="button"
          class="theme-button-secondary inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[11.5px] font-semibold"
          :aria-label="t(locale, 'desktop.activity.sidebarToggle')"
          :title="t(locale, 'desktop.activity.sidebarToggle')"
          @click="sidebarOpen = !sidebarOpen"
        >
          <PanelLeft class="h-3.5 w-3.5" aria-hidden="true" />
          <span class="hidden sm:inline">{{ t(locale, 'desktop.activity.filtersLabel') }}</span>
        </button>
        <button
          type="button"
          class="theme-button-secondary inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[11.5px] font-semibold"
          :aria-label="t(locale, 'desktop.activity.inspectorToggle')"
          :title="t(locale, 'desktop.activity.inspectorToggle')"
          @click="inspectorOpen = !inspectorOpen"
        >
          <Eye class="h-3.5 w-3.5" aria-hidden="true" />
          <span class="hidden sm:inline">{{ t(locale, 'desktop.activity.inspectorTitle') }}</span>
        </button>
      </div>
    </div>

    <!-- ============ 状态机 ============ -->
    <!-- 未选择会话 -->
    <div
      v-if="viewState === 'no-session'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <Bot class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <h3 class="mt-3 text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.noSessionTitle') }}</h3>
      <p class="mt-1.5 max-w-md text-[11.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.noSessionDesc') }}</p>
    </div>

    <!-- 隐私关闭（deep_index_level=off） -->
    <div
      v-else-if="viewState === 'privacy-off'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <EyeOff class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <h3 class="mt-3 text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.privacyDisabledTitle') }}</h3>
      <p class="mt-1.5 max-w-md text-[11.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.privacyDisabledDesc') }}</p>
      <button
        type="button"
        class="theme-button-secondary mt-5 inline-flex items-center gap-1.5 rounded-lg px-4 py-2 text-[12px] font-semibold"
        @click="nav.openSettingsSection('privacy')"
      >
        {{ t(locale, 'desktop.activity.openSettings') }}
      </button>
    </div>

    <!-- 初始/切换加载骨架 -->
    <div v-else-if="viewState === 'loading'" class="space-y-2">
      <div v-for="i in 5" :key="i" class="theme-surface animate-pulse rounded-xl border px-4 py-3">
        <div class="h-3 w-1/3 rounded bg-[var(--theme-border-subtle)]"></div>
        <div class="mt-2 h-2.5 w-2/3 rounded bg-[var(--theme-border-subtle)]"></div>
      </div>
    </div>

    <!-- 仅聚合数据（capability.level=none） -->
    <div
      v-else-if="viewState === 'aggregate-only'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <Layers class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <h3 class="mt-3 text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.aggregateOnlyTitle') }}</h3>
      <p class="mt-1.5 max-w-md text-[11.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.aggregateOnlyDesc') }}</p>
      <button
        type="button"
        class="theme-button-secondary mt-5 inline-flex items-center gap-1.5 rounded-lg px-4 py-2 text-[12px] font-semibold"
        @click="nav.openSettingsSection('dataSources')"
      >
        {{ t(locale, 'desktop.activity.manageDataSources') }}
      </button>
    </div>

    <!-- 无深度数据（rebuild 后仍无） -->
    <div
      v-else-if="viewState === 'no-data'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <FileOutput class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <h3 class="mt-3 text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.emptyTitle') }}</h3>
      <p class="mt-1.5 max-w-md text-[11.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.emptyDesc') }}</p>
      <button
        type="button"
        class="theme-button-secondary mt-5 inline-flex items-center gap-1.5 rounded-lg px-4 py-2 text-[12px] font-semibold"
        :disabled="building"
        @click="rebuildAndReload"
      >
        <Loader2 v-if="building" class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
        <RefreshCw v-else class="h-3.5 w-3.5" aria-hidden="true" />
        {{ building ? t(locale, 'desktop.activity.buildingIndex') : t(locale, 'desktop.activity.rebuildIndex') }}
      </button>
    </div>

    <!-- 加载失败 -->
    <div
      v-else-if="viewState === 'error'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <TriangleAlert class="h-7 w-7 text-rose-500" aria-hidden="true" />
      <h3 class="mt-3 text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.errorTitle') }}</h3>
      <p class="mt-1.5 max-w-md break-all text-[11.5px] leading-relaxed text-rose-500">{{ backendErrorLabel(locale, error) }}</p>
      <button
        type="button"
        class="theme-button-secondary mt-5 inline-flex items-center gap-1.5 rounded-lg px-4 py-2 text-[12px] font-semibold"
        @click="retry"
      >
        <RefreshCw class="h-3.5 w-3.5" aria-hidden="true" />
        {{ t(locale, 'desktop.activity.retry') }}
      </button>
    </div>

    <!-- 非 fulltext 档：会话内搜索不可用提示（9.8：不静默降级为本地过滤） -->
    <div
      v-if="viewState === 'ready' && !fulltextEnabled"
      class="flex flex-wrap items-center gap-2 rounded-xl border border-amber-500/20 bg-amber-500/5 px-3 py-2"
    >
      <TriangleAlert class="h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-300" aria-hidden="true" />
      <span class="min-w-0 flex-1 text-[11px] text-amber-600 dark:text-amber-300">{{ t(locale, 'desktop.activity.searchFtsDisabled') }}</span>
      <button
        type="button"
        class="shrink-0 rounded-md border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-[10.5px] font-semibold text-amber-600 transition-colors hover:bg-amber-500/15 dark:text-amber-300"
        @click="nav.openSettingsSection('privacy')"
      >
        {{ t(locale, 'desktop.activity.openSettings') }}
      </button>
    </div>

    <!-- ============ 三栏布局（设计 9.2） ============ -->
    <div v-else-if="viewState === 'ready'" class="relative flex min-h-0 min-w-0 flex-1 items-stretch gap-4">
      <!-- 左栏：章节与代理树（240px；窄屏 overlay） -->
      <aside
        v-if="wideMode || sidebarOpen"
        class="theme-surface flex w-60 shrink-0 flex-col overflow-hidden rounded-xl border"
        :class="wideMode ? '' : 'absolute inset-y-0 left-0 z-40 w-72 shadow-2xl'"
        :aria-label="t(locale, 'desktop.activity.filtersLabel')"
      >
        <div class="flex items-center justify-between border-b border-[var(--theme-border-subtle)] px-3 py-2">
          <span class="text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.filtersLabel') }}</span>
          <button
            v-if="!wideMode"
            type="button"
            class="rounded p-1 text-[var(--theme-text-quaternary)] hover:bg-[var(--theme-bg-hover)]"
            :aria-label="t(locale, 'common.close')"
            :title="t(locale, 'common.close')"
            @click="sidebarOpen = false"
          >
            <X class="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>
        <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-3">
          <!-- 事件类型过滤（复选） -->
          <div class="space-y-1">
            <span class="px-0.5 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.filtersLabel') }}</span>
            <label
              v-for="group in KIND_GROUPS"
              :key="group.id"
              class="flex cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 transition-colors hover:bg-[var(--theme-bg-hover)]"
            >
              <input
                type="checkbox"
                class="h-3.5 w-3.5 shrink-0 accent-[var(--theme-accent-primary)]"
                :checked="groupChecked(group.kinds)"
                :aria-label="t(locale, group.labelKey)"
                @change="toggleKindGroup(group.kinds)"
              />
              <span class="text-[11.5px] font-medium text-[var(--theme-text-secondary)]">{{ t(locale, group.labelKey) }}</span>
            </label>
          </div>

          <!-- 代理（设计 9.3 / 9.6） -->
          <div class="space-y-1">
            <div class="flex items-center justify-between px-0.5">
              <span class="text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.agentsLabel') }}</span>
              <button
                v-if="activeAgentKey"
                type="button"
                class="rounded px-1 text-[10px] font-semibold text-[var(--theme-accent-primary)] hover:underline"
                @click="activeAgentKey = null"
              >
                {{ t(locale, 'desktop.activity.clearFilter') }}
              </button>
            </div>
            <p v-if="relationLevel !== 'fullTree'" class="px-0.5 text-[10px] leading-relaxed text-[var(--theme-text-quaternary)]">
              {{ t(locale, 'desktop.activity.agentsRelationHint') }}
            </p>
            <div v-if="agentRows.length === 0 && unlinkedAgents.length === 0" class="px-0.5 py-2 text-[11px] text-[var(--theme-text-tertiary)]">
              {{ t(locale, 'desktop.activity.agentsEmpty') }}
            </div>
            <template v-else>
              <!-- fullTree：父子树（缩进层级） -->
              <button
                v-for="row in agentRows"
                :key="row.node.agentKey"
                type="button"
                class="flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1 text-left transition-colors hover:bg-[var(--theme-bg-hover)]"
                :style="{ paddingLeft: `${8 + row.depth * 14}px` }"
                :class="activeAgentKey === row.node.agentKey ? 'bg-[var(--theme-accent-soft)]' : ''"
                @click="activeAgentKey = activeAgentKey === row.node.agentKey ? null : row.node.agentKey"
              >
                <ChevronRight
                  v-if="row.depth === 0"
                  class="h-3 w-3 shrink-0 text-[var(--theme-text-quaternary)]"
                  aria-hidden="true"
                />
                <Bot class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                <span class="min-w-0 flex-1 truncate text-[11px] font-medium text-[var(--theme-text-secondary)]">
                  {{ row.node.displayKind || shortAgentId(row.node.agentKey) }}
                </span>
                <span class="shrink-0 rounded px-1 py-px text-[9px] font-bold leading-none" :class="agentStatusMeta(row.node.status)">
                  {{ t(locale, agentStatusKey(row.node.status)) }}
                </span>
              </button>
              <!-- 非 fullTree：分组列表（不画伪树） -->
              <button
                v-for="node in unlinkedAgents"
                :key="node.agentKey"
                type="button"
                class="flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1 text-left transition-colors hover:bg-[var(--theme-bg-hover)]"
                :class="activeAgentKey === node.agentKey ? 'bg-[var(--theme-accent-soft)]' : ''"
                @click="activeAgentKey = activeAgentKey === node.agentKey ? null : node.agentKey"
              >
                <Bot class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                <span class="min-w-0 flex-1 truncate text-[11px] font-medium text-[var(--theme-text-secondary)]">
                  {{ node.displayKind || shortAgentId(node.agentKey) }}
                </span>
                <span class="shrink-0 rounded px-1 py-px text-[9px] font-bold leading-none" :class="agentStatusMeta(node.status)">
                  {{ t(locale, agentStatusKey(node.status)) }}
                </span>
              </button>
            </template>
          </div>

          <!-- 工具汇总 -->
          <div class="space-y-1">
            <span class="px-0.5 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.toolsLabel') }}</span>
            <div v-if="toolSummary.length === 0" class="px-0.5 py-2 text-[11px] text-[var(--theme-text-tertiary)]">
              {{ t(locale, 'desktop.activity.toolsEmpty') }}
            </div>
            <div v-for="row in toolSummary" :key="row.toolName" class="flex items-center gap-2 rounded-lg px-2 py-1.5">
              <Wrench class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
              <div class="min-w-0 flex-1">
                <div class="truncate text-[11px] font-medium text-[var(--theme-text-secondary)]" :title="row.toolName">{{ row.toolName }}</div>
                <div class="text-[9.5px] text-[var(--theme-text-quaternary)]">
                  {{ t(locale, 'desktop.activity.invocationsCount', { count: row.invocationCount }) }}
                  <span v-if="row.errorCount > 0" class="text-rose-500"> · {{ t(locale, 'desktop.activity.failuresCount', { count: row.errorCount }) }}</span>
                  <span class="font-mono"> · {{ formatDuration(row.avgDurationMs) }}</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </aside>

      <!-- 中栏：活动脉络时间线（min 480px） -->
      <main class="theme-surface flex min-w-0 flex-1 flex-col overflow-hidden rounded-xl border" :aria-label="t(locale, 'desktop.activity.timelineLabel')">
        <div class="flex shrink-0 items-center gap-2 border-b border-[var(--theme-border-subtle)] px-4 py-2">
          <span class="shrink-0 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.timelineLabel') }}</span>

          <!-- 会话内搜索（M3 FTS；非 fulltext 档禁用执行，9.8） -->
          <div class="ml-1 flex min-w-0 max-w-xs flex-1 items-center gap-1.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-1">
            <Search class="h-3 w-3 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <input
              v-model="searchInput"
              type="text"
              class="min-w-0 flex-1 bg-transparent text-[11px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)] disabled:opacity-50"
              :placeholder="t(locale, 'desktop.activity.searchPlaceholder')"
              :disabled="!fulltextEnabled"
              @keydown.enter="triggerSearch"
            />
            <button
              v-if="searchInput.trim()"
              type="button"
              class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:bg-[var(--theme-bg-hover)]"
              :aria-label="t(locale, 'desktop.activity.searchClear')"
              :title="t(locale, 'desktop.activity.searchClear')"
              @click="clearSearch"
            >
              <X class="h-3 w-3" aria-hidden="true" />
            </button>
          </div>

          <span class="ml-auto shrink-0 text-[10.5px] text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.activity.eventsCount', { count: total }) }}
            <span v-if="activeAgentKey" class="ml-1.5 text-[var(--theme-accent-primary)]">{{ t(locale, 'desktop.activity.filterActive', { label: activeAgentKey }) }}</span>
          </span>

          <!-- 导出（15.3：范围预览对话框，默认不勾选 payload） -->
          <button
            type="button"
            class="theme-button-secondary inline-flex h-7 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-[11px] font-semibold"
            :aria-label="t(locale, 'desktop.activity.exportButton')"
            :title="t(locale, 'desktop.activity.exportButton')"
            @click="openExportDialog"
          >
            <Download class="h-3 w-3" aria-hidden="true" />
            <span class="hidden sm:inline">{{ t(locale, 'desktop.activity.exportButton') }}</span>
          </button>
        </div>

        <!-- 搜索模式控制条：命中数 + 上一项/下一项 + 清除（设计 9.8） -->
        <div
          v-if="searchMode"
          class="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-4 py-1.5"
        >
          <span class="text-[10.5px] font-semibold text-[var(--theme-accent-primary)]">{{ t(locale, 'desktop.activity.searchHitCount', { count: searchTotal }) }}</span>
          <span v-if="searchResults.length > 0 && searchActiveIndex >= 0" class="font-mono text-[10px] text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.activity.searchPosition', { current: searchActiveIndex + 1, total: searchResults.length }) }}
          </span>
          <div class="ml-auto flex items-center gap-1">
            <button
              type="button"
              class="rounded p-1 text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:opacity-40"
              :aria-label="t(locale, 'desktop.activity.searchPrev')"
              :title="t(locale, 'desktop.activity.searchPrev')"
              :disabled="searchLoading || searchResults.length === 0"
              @click="searchStep(-1)"
            >
              <ChevronLeft class="h-3.5 w-3.5" aria-hidden="true" />
            </button>
            <button
              type="button"
              class="rounded p-1 text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:opacity-40"
              :aria-label="t(locale, 'desktop.activity.searchNext')"
              :title="t(locale, 'desktop.activity.searchNext')"
              :disabled="searchLoading || searchResults.length === 0"
              @click="searchStep(1)"
            >
              <ChevronRight class="h-3.5 w-3.5" aria-hidden="true" />
            </button>
            <button
              type="button"
              class="ml-1 rounded px-1.5 py-0.5 text-[10.5px] font-semibold text-[var(--theme-accent-primary)] hover:underline"
              @click="clearSearch"
            >
              {{ t(locale, 'desktop.activity.searchClear') }}
            </button>
          </div>
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto p-3" style="max-height: 62vh">
          <!-- ===== 搜索模式：命中列表（替代时间线，设计 9.8） ===== -->
          <template v-if="searchMode">
            <div v-if="searchLoading && searchResults.length === 0" class="flex items-center justify-center gap-1.5 py-8 text-[10.5px] text-[var(--theme-text-tertiary)]">
              <Loader2 class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
              {{ t(locale, 'desktop.activity.timelineLoading') }}
            </div>
            <div v-else-if="searchError" class="flex flex-col items-center justify-center px-6 py-16 text-center">
              <TriangleAlert class="h-6 w-6 text-rose-500" aria-hidden="true" />
              <p class="mt-2 max-w-md break-all text-[11px] leading-relaxed text-rose-500">{{ backendErrorLabel(locale, searchError) }}</p>
              <button
                type="button"
                class="theme-button-secondary mt-4 inline-flex items-center gap-1.5 rounded-lg px-3.5 py-1.5 text-[11px] font-semibold"
                @click="runSearch(true)"
              >
                <RefreshCw class="h-3 w-3" aria-hidden="true" />
                {{ t(locale, 'desktop.activity.retry') }}
              </button>
            </div>
            <div v-else-if="searchResults.length === 0" class="flex flex-col items-center justify-center px-6 py-16 text-center">
              <Search class="h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
              <p class="mt-2 text-[11.5px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.timelineEmpty') }}</p>
            </div>
            <div v-else class="space-y-1.5">
              <button
                v-for="(event, index) in searchResults"
                :id="`search-hit-${index}`"
                :key="event.eventKey"
                type="button"
                class="block w-full rounded-xl border px-3 py-2.5 text-left transition-colors"
                :class="selectedEvent?.eventKey === event.eventKey
                  ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-soft)]'
                  : 'border-[var(--theme-border-subtle)] hover:bg-[var(--theme-bg-hover)]'"
                @click="selectEvent(event)"
              >
                <div class="flex items-start gap-2.5">
                  <span class="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg" :class="KIND_META[event.kind].cls">
                    <component :is="KIND_META[event.kind].icon" class="h-3.5 w-3.5" aria-hidden="true" />
                  </span>
                  <span class="min-w-0 flex-1">
                    <span class="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                      <span class="text-[11px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, KIND_META[event.kind].labelKey) }}</span>
                      <span class="text-[10px] text-[var(--theme-text-tertiary)]" :title="event.timestampMs != null ? formatAbsTime(event.timestampMs) : undefined">
                        {{ event.timestampMs != null ? formatRelTime(event.timestampMs) : t(locale, 'desktop.activity.timeUnknown') }}
                      </span>
                      <span
                        v-if="statusMeta(event.status)"
                        class="inline-flex items-center rounded-full px-1.5 py-px text-[9px] font-bold leading-none"
                        :class="statusMeta(event.status)!.cls"
                      >
                        {{ t(locale, statusMeta(event.status)!.labelKey) }}
                      </span>
                      <span v-if="event.tool" class="inline-flex items-center gap-0.5 rounded bg-cyan-500/10 px-1.5 py-px font-mono text-[9px] text-cyan-600 dark:text-cyan-300">
                        <Wrench class="h-2.5 w-2.5" aria-hidden="true" />
                        {{ event.tool.normalizedName || event.tool.rawName }}
                      </span>
                    </span>
                    <span v-if="event.summary" class="mt-1 line-clamp-2 block break-words text-[11.5px] leading-relaxed text-[var(--theme-text-secondary)]">
                      {{ event.summary }}
                    </span>
                  </span>
                  <ChevronRight class="mt-1 h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                </div>
              </button>
              <div v-if="searchHasMore" class="flex items-center justify-center py-2">
                <button
                  type="button"
                  class="theme-button-secondary inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[10.5px] font-semibold disabled:opacity-60"
                  :disabled="searchLoading"
                  @click="runSearch(false)"
                >
                  <Loader2 v-if="searchLoading" class="h-3 w-3 animate-spin" aria-hidden="true" />
                  {{ t(locale, 'desktop.activity.loadMore') }}
                </button>
              </div>
            </div>
          </template>

          <!-- ===== 常规时间线 ===== -->
          <template v-else>
            <!-- 空态 -->
            <div v-if="!loading && events.length === 0" class="flex flex-col items-center justify-center px-6 py-16 text-center">
              <Search v-if="selectedKinds.size !== ALL_KINDS.length || activeAgentKey" class="h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
              <FileOutput v-else class="h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
              <p class="mt-2 text-[11.5px] text-[var(--theme-text-tertiary)]">
                {{ (selectedKinds.size !== ALL_KINDS.length || activeAgentKey) ? t(locale, 'desktop.activity.timelineEmpty') : t(locale, 'desktop.activity.timelineNoEvents') }}
              </p>
            </div>

            <!-- 事件流（按 sequence 升序） -->
            <div v-else class="space-y-1.5">
              <button
                v-for="event in events"
                :key="event.eventKey"
                type="button"
                class="block w-full rounded-xl border px-3 py-2.5 text-left transition-colors"
                :class="selectedEvent?.eventKey === event.eventKey
                  ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-soft)]'
                  : 'border-[var(--theme-border-subtle)] hover:bg-[var(--theme-bg-hover)]'"
                @click="selectEvent(event)"
              >
                <div class="flex items-start gap-2.5">
                  <span
                    class="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg"
                    :class="KIND_META[event.kind].cls"
                  >
                    <component :is="KIND_META[event.kind].icon" class="h-3.5 w-3.5" aria-hidden="true" />
                  </span>
                  <span class="min-w-0 flex-1">
                    <span class="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                      <span class="text-[11px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, KIND_META[event.kind].labelKey) }}</span>
                      <span class="text-[10px] text-[var(--theme-text-tertiary)]" :title="event.timestampMs != null ? formatAbsTime(event.timestampMs) : undefined">
                        {{ event.timestampMs != null ? formatRelTime(event.timestampMs) : t(locale, 'desktop.activity.timeUnknown') }}
                      </span>
                      <span
                        v-if="statusMeta(event.status)"
                        class="inline-flex items-center rounded-full px-1.5 py-px text-[9px] font-bold leading-none"
                        :class="statusMeta(event.status)!.cls"
                      >
                        {{ t(locale, statusMeta(event.status)!.labelKey) }}
                      </span>
                      <span v-if="event.actorAgentKey" class="inline-flex items-center gap-0.5 rounded bg-slate-500/10 px-1.5 py-px font-mono text-[9px] text-slate-500 dark:text-slate-300">
                        <Bot class="h-2.5 w-2.5" aria-hidden="true" />
                        {{ shortAgentId(event.actorAgentKey) }}
                      </span>
                    </span>
                    <!-- 摘要（最多 2 行截断，默认折叠长文本） -->
                    <span v-if="event.summary" class="mt-1 line-clamp-2 block break-words text-[11.5px] leading-relaxed text-[var(--theme-text-secondary)]">
                      {{ event.summary }}
                    </span>
                  </span>
                  <ChevronRight class="mt-1 h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                </div>

                <!-- 工具调用卡（设计 9.5） -->
                <div
                  v-if="event.tool"
                  class="mt-2 ml-9 flex flex-wrap items-center gap-1.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2"
                >
                  <span class="font-mono text-[11px] font-semibold text-[var(--theme-text-primary)]">{{ event.tool.normalizedName || event.tool.rawName }}</span>
                  <span v-if="event.tool.family" class="rounded bg-cyan-500/10 px-1.5 py-px text-[9px] font-bold text-cyan-600 dark:text-cyan-300">{{ event.tool.family }}</span>
                  <span v-if="event.tool.durationMs != null" class="inline-flex items-center gap-0.5 text-[10px] text-[var(--theme-text-tertiary)]">
                    <Clock class="h-3 w-3" aria-hidden="true" />
                    {{ formatDuration(event.tool.durationMs) }}
                  </span>
                  <span
                    v-for="key in event.tool.inputKeys.slice(0, 6)"
                    :key="key"
                    class="max-w-28 truncate rounded bg-[var(--theme-border-subtle)] px-1.5 py-px font-mono text-[9px] text-[var(--theme-text-tertiary)]"
                    :title="key"
                  >
                    {{ key }}
                  </span>
                  <span v-if="event.tool.inputKeys.length > 6" class="text-[9px] text-[var(--theme-text-quaternary)]">+{{ event.tool.inputKeys.length - 6 }}</span>
                  <span class="ml-auto inline-flex items-center gap-0.5 text-[9.5px] font-semibold text-[var(--theme-accent-primary)]">
                    {{ t(locale, 'desktop.activity.expand') }}
                    <ChevronDown class="h-3 w-3" aria-hidden="true" />
                  </span>
                </div>
              </button>

              <!-- 分页 sentinel + 底部状态 -->
              <div ref="sentinel" class="flex items-center justify-center py-2">
                <span v-if="loadingMore" class="inline-flex items-center gap-1.5 text-[10.5px] text-[var(--theme-text-tertiary)]">
                  <Loader2 class="h-3 w-3 animate-spin" aria-hidden="true" />
                  {{ t(locale, 'desktop.activity.timelineLoading') }}
                </span>
                <span v-else-if="!hasMore" class="text-[10.5px] text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.noMore') }}</span>
              </div>
            </div>
          </template>
        </div>
      </main>

      <!-- 右栏：事件检查器（360px；宽屏可折叠，窄屏 overlay drawer） -->
      <aside
        v-if="(wideMode && !inspectorCollapsed) || (!wideMode && inspectorOpen)"
        class="theme-surface flex shrink-0 flex-col overflow-hidden rounded-xl border"
        :class="wideMode ? '' : 'absolute inset-y-0 right-0 z-40 shadow-2xl'"
        style="width: min(360px, 86vw)"
        :aria-label="t(locale, 'desktop.activity.inspectorTitle')"
      >
        <!-- 检查器头部 -->
        <div class="flex shrink-0 items-center justify-between border-b border-[var(--theme-border-subtle)] px-3 py-2">
          <span class="flex items-center gap-1.5 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
            <Eye class="h-3.5 w-3.5" aria-hidden="true" />
            {{ t(locale, 'desktop.activity.inspectorTitle') }}
          </span>
          <button
            type="button"
            class="rounded p-1 text-[var(--theme-text-quaternary)] hover:bg-[var(--theme-bg-hover)]"
            :aria-label="t(locale, 'desktop.activity.inspectorClose')"
            :title="t(locale, 'desktop.activity.inspectorClose')"
            @click="wideMode ? (inspectorCollapsed = true) : closeInspector()"
          >
            <X class="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>

        <div class="min-h-0 flex-1 overflow-y-auto p-3">
          <!-- 未选中 -->
          <div v-if="!selectedEvent" class="flex flex-col items-center justify-center px-4 py-16 text-center">
            <EyeOff class="h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <p class="mt-2 text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.inspectorEmpty') }}</p>
          </div>

          <div v-else class="space-y-3">
            <!-- 类型 / 时间 / 状态 -->
            <div class="flex items-start gap-2.5">
              <span class="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg" :class="KIND_META[selectedEvent.kind].cls">
                <component :is="KIND_META[selectedEvent.kind].icon" class="h-4 w-4" aria-hidden="true" />
              </span>
              <div class="min-w-0 flex-1">
                <div class="text-[12.5px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, KIND_META[selectedEvent.kind].labelKey) }}</div>
                <div class="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[10.5px] text-[var(--theme-text-tertiary)]">
                  <span class="inline-flex items-center gap-1" :title="selectedEvent.timestampMs != null ? formatAbsTime(selectedEvent.timestampMs) : undefined">
                    <Clock class="h-3 w-3" aria-hidden="true" />
                    {{ selectedEvent.timestampMs != null ? formatAbsTime(selectedEvent.timestampMs) : t(locale, 'desktop.activity.timeUnknown') }}
                  </span>
                  <span
                    v-if="statusMeta(selectedEvent.status)"
                    class="inline-flex items-center rounded-full px-1.5 py-px text-[9px] font-bold leading-none"
                    :class="statusMeta(selectedEvent.status)!.cls"
                  >
                    {{ t(locale, statusMeta(selectedEvent.status)!.labelKey) }}
                  </span>
                </div>
              </div>
            </div>

            <!-- 来源文件（脱敏路径，直接显示） -->
            <div class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
              <div class="text-[9.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inspectorSourceFile') }}</div>
              <div class="mt-1 flex items-center gap-1.5 break-all font-mono text-[10.5px] leading-relaxed text-[var(--theme-text-secondary)]">
                <FolderOpen class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                {{ selectedEvent.sourceRef.sourceFilePath || '—' }}
              </div>
            </div>

            <!-- 摘要全文（不截断） -->
            <div v-if="selectedEvent.summary" class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
              <div class="text-[9.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inspectorSummary') }}</div>
              <p class="mt-1 break-words text-[11.5px] leading-relaxed text-[var(--theme-text-secondary)]">{{ selectedEvent.summary }}</p>
            </div>

            <!-- 请求关联（设计 9.9） -->
            <div v-if="selectedEvent.requestLinks.length > 0" class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
              <div class="text-[9.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inspectorRequestLinks') }}</div>
              <ul class="mt-1 space-y-1">
                <li v-for="link in selectedEvent.requestLinks" :key="link.requestKey" class="flex items-center gap-1.5">
                  <Link2 class="h-3 w-3 shrink-0 text-[var(--theme-text-quaternary)]" :class="{ 'opacity-40': linkMeta(link.strength).dashed }" aria-hidden="true" />
                  <span class="min-w-0 flex-1 truncate font-mono text-[10px] text-[var(--theme-text-secondary)]" :title="link.requestKey">{{ link.requestKey }}</span>
                  <span
                    class="shrink-0 rounded px-1.5 py-px text-[9px] font-bold leading-none"
                    :class="linkMeta(link.strength).dashed
                      ? 'bg-amber-500/10 text-amber-600 dark:text-amber-300'
                      : 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'"
                  >
                    {{ t(locale, linkMeta(link.strength).labelKey) }}
                  </span>
                </li>
              </ul>
            </div>

            <!-- 工具详情 -->
            <div v-if="selectedEvent.tool" class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
              <div class="text-[9.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inspectorTool') }}</div>
              <div class="mt-1.5 space-y-1 text-[10.5px] text-[var(--theme-text-secondary)]">
                <div class="flex items-center gap-1.5">
                  <Wrench class="h-3 w-3 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                  <span class="truncate font-mono font-semibold">{{ selectedEvent.tool.normalizedName || selectedEvent.tool.rawName }}</span>
                  <span v-if="selectedEvent.tool.family" class="rounded bg-cyan-500/10 px-1.5 py-px text-[9px] font-bold text-cyan-600 dark:text-cyan-300">{{ selectedEvent.tool.family }}</span>
                </div>
                <div class="flex flex-wrap gap-x-3 gap-y-0.5">
                  <span class="inline-flex items-center gap-1"><Clock class="h-3 w-3 text-[var(--theme-text-quaternary)]" aria-hidden="true" />{{ t(locale, 'desktop.activity.durationLabel') }}: {{ formatDuration(selectedEvent.tool.durationMs) }}</span>
                  <span v-if="selectedEvent.tool.inputBytes != null">{{ t(locale, 'desktop.activity.inspectorInput') }}: {{ formatBytes(selectedEvent.tool.inputBytes) }}</span>
                  <span v-if="selectedEvent.tool.outputBytes != null">{{ t(locale, 'desktop.activity.inspectorOutput') }}: {{ formatBytes(selectedEvent.tool.outputBytes) }}</span>
                </div>
                <div v-if="selectedEvent.tool.inputKeys.length > 0" class="flex flex-wrap items-center gap-1">
                  <span class="text-[9.5px] text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inputKeysLabel') }}:</span>
                  <span v-for="key in selectedEvent.tool.inputKeys" :key="key" class="max-w-32 truncate rounded bg-[var(--theme-border-subtle)] px-1.5 py-px font-mono text-[9px] text-[var(--theme-text-tertiary)]" :title="key">{{ key }}</span>
                </div>
                <div v-if="selectedEvent.tool.resultKind" class="break-all font-mono text-[10px] text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.resultKind', { kind: selectedEvent.tool.resultKind }) }}</div>
              </div>
            </div>

            <!-- payload（设计 12.5：内容全部来自后端脱敏 payload） -->
            <div class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
              <div class="flex flex-wrap items-center gap-1">
                <button
                  v-for="section in payloadSections"
                  :key="section"
                  type="button"
                  class="rounded-md px-2 py-1 text-[10px] font-semibold capitalize transition-colors"
                  :class="payloadSection === section
                    ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]'
                    : 'text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]'"
                  @click="setPayloadSection(section)"
                >
                  {{ t(locale, section === 'summary' ? 'desktop.activity.inspectorSummary' : section === 'input' ? 'desktop.activity.inspectorInput' : 'desktop.activity.inspectorOutput') }}
                </button>
                <span
                  v-if="payload"
                  class="ml-auto inline-flex items-center rounded px-1.5 py-px text-[9px] font-bold leading-none"
                  :class="contentStateLabel(payload.contentState).cls"
                >
                  {{ t(locale, contentStateLabel(payload.contentState).key) }}
                </span>
              </div>
              <div class="mt-2">
                <div v-if="payloadLoading" class="flex items-center justify-center gap-1.5 py-6 text-[10.5px] text-[var(--theme-text-tertiary)]">
                  <Loader2 class="h-3 w-3 animate-spin" aria-hidden="true" />
                  {{ t(locale, 'desktop.activity.timelineLoading') }}
                </div>
                <div v-else-if="payloadError" class="break-all py-2 text-[10.5px] text-rose-500">{{ backendErrorLabel(locale, payloadError) }}</div>
                <template v-else-if="payload">
                  <p v-if="payload.truncated" class="mb-1.5 text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.payloadTruncated') }}</p>
                  <pre
                    v-if="payload.content"
                    class="max-h-72 overflow-y-auto whitespace-pre-wrap break-all rounded-lg bg-[var(--theme-bg-workspace)] p-2.5 font-mono text-[10px] leading-relaxed text-[var(--theme-text-secondary)]"
                  >{{ payload.content }}</pre>
                  <p v-else class="py-4 text-center text-[10.5px] text-[var(--theme-text-quaternary)]">
                    {{ payload.contentState === 'unavailable' ? t(locale, 'desktop.activity.payloadUnavailable') : t(locale, 'desktop.activity.payloadEmpty') }}
                  </p>
                  <!-- M3 payload 分页（设计 12.5：nextCursor 续读 append） -->
                  <button
                    v-if="payload.nextCursor"
                    type="button"
                    class="mt-2 inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-[var(--theme-border-subtle)] px-2.5 py-1.5 text-[10.5px] font-semibold text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:opacity-60"
                    :disabled="payloadLoading"
                    @click="loadMorePayload"
                  >
                    <Loader2 v-if="payloadLoading" class="h-3 w-3 animate-spin" aria-hidden="true" />
                    {{ t(locale, 'desktop.activity.payloadLoadMore') }}
                  </button>
                </template>
              </div>
            </div>

            <!-- 动作：复制 ID（不复制敏感内容） -->
            <div class="flex items-center gap-2 pt-1">
              <button
                type="button"
                class="theme-button-secondary inline-flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-[11px] font-semibold"
                :aria-label="t(locale, 'desktop.activity.copyId')"
                :title="t(locale, 'desktop.activity.copyId')"
                @click="copyEventId"
              >
                <Copy class="h-3 w-3" aria-hidden="true" />
                {{ t(locale, 'desktop.activity.copyId') }}
              </button>
              <span v-if="copiedFlash" class="text-[10.5px] font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.activity.copied') }}</span>
              <span class="ml-auto max-w-36 truncate font-mono text-[9.5px] text-[var(--theme-text-quaternary)]" :title="selectedEvent.eventKey">{{ selectedEvent.eventKey }}</span>
            </div>
          </div>
        </div>
      </aside>

      <!-- 宽屏检查器折叠按钮 -->
      <button
        v-if="wideMode && inspectorCollapsed"
        type="button"
        class="theme-surface absolute right-0 top-1/2 z-10 -translate-y-1/2 rounded-xl border p-2 text-[var(--theme-text-tertiary)] shadow-lg transition-colors hover:text-[var(--theme-text-primary)]"
        :aria-label="t(locale, 'desktop.activity.inspectorToggle')"
        :title="t(locale, 'desktop.activity.inspectorToggle')"
        @click="inspectorCollapsed = false"
      >
        <Eye class="h-4 w-4" aria-hidden="true" />
      </button>
    </div>

    <!-- 重建索引中的遮罩提示 -->
    <div v-if="building" class="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
      <div class="theme-surface-elevated flex items-center gap-2 rounded-xl border px-4 py-3 text-[12px] font-medium text-[var(--theme-text-secondary)] shadow-xl">
        <Loader2 class="h-4 w-4 animate-spin" aria-hidden="true" />
        {{ t(locale, 'desktop.activity.buildingIndex') }}
      </div>
    </div>

    <!-- 导出对话框（15.3 / 21.5：范围预览，默认不勾选正文与工具 payload；成功显示路径可复制） -->
    <div
      v-if="exportDialogOpen"
      class="fixed inset-0 z-50 flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      :aria-label="t(locale, 'desktop.activity.exportDialogTitle')"
    >
      <div class="absolute inset-0 bg-black/40" @click="closeExportDialog"></div>
      <div class="theme-surface-elevated relative w-full max-w-md rounded-xl border p-4 shadow-xl">
        <div class="flex items-center justify-between gap-2">
          <h3 class="text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.exportDialogTitle') }}</h3>
          <button
            type="button"
            class="rounded p-1 text-[var(--theme-text-quaternary)] hover:bg-[var(--theme-bg-hover)]"
            :aria-label="t(locale, 'common.close')"
            :title="t(locale, 'common.close')"
            @click="closeExportDialog"
          >
            <X class="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        <!-- 范围预览 -->
        <div class="mt-3 space-y-2">
          <div class="flex items-center gap-2">
            <span class="w-24 shrink-0 text-[10.5px] font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.exportFormat') }}</span>
            <label class="flex cursor-pointer items-center gap-1.5 text-[11.5px] text-[var(--theme-text-secondary)]">
              <input
                v-model="exportFormat"
                type="radio"
                value="json"
                class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]"
              />
              {{ t(locale, 'desktop.activity.exportFormatJson') }}
            </label>
            <label class="flex cursor-pointer items-center gap-1.5 text-[11.5px] text-[var(--theme-text-secondary)]">
              <input
                v-model="exportFormat"
                type="radio"
                value="csv"
                class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]"
              />
              {{ t(locale, 'desktop.activity.exportFormatCsv') }}
            </label>
          </div>

          <div class="space-y-1.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-3 py-2.5">
            <div class="text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.exportScopePreview') }}</div>
            <label class="flex cursor-pointer items-center gap-2">
              <input v-model="exportIncludeSummaries" type="checkbox" class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]" />
              <span class="text-[11.5px] text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.activity.exportIncludeSummaries') }}</span>
            </label>
            <label class="flex cursor-pointer items-center gap-2">
              <input v-model="exportIncludeToolSummaries" type="checkbox" class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]" />
              <span class="text-[11.5px] text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.activity.exportIncludeToolSummaries') }}</span>
            </label>
            <label class="flex cursor-pointer items-center gap-2">
              <input v-model="exportIncludeRequestLinks" type="checkbox" class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]" />
              <span class="text-[11.5px] text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.activity.exportIncludeRequestLinks') }}</span>
            </label>
            <label class="flex cursor-pointer items-center gap-2">
              <input v-model="exportIncludePayloads" type="checkbox" class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]" />
              <span class="text-[11.5px] text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.activity.exportIncludePayloads') }}</span>
            </label>
            <p
              v-if="exportIncludePayloads"
              class="flex items-start gap-1.5 rounded-lg border border-amber-500/20 bg-amber-500/5 px-2.5 py-2 text-[10.5px] leading-relaxed text-amber-600 dark:text-amber-300"
            >
              <TriangleAlert class="mt-0.5 h-3 w-3 shrink-0" aria-hidden="true" />
              {{ t(locale, 'desktop.activity.exportPayloadWarning') }}
            </p>
          </div>
        </div>

        <!-- 结果 / 错误 -->
        <div class="mt-3">
          <p v-if="exportError" class="break-all text-[10.5px] leading-relaxed text-rose-500">{{ backendErrorLabel(locale, exportError) }}</p>
          <div v-else-if="exportResult" class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
            <div class="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[10.5px] text-[var(--theme-text-tertiary)]">
              <CheckCircle2 class="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-300" aria-hidden="true" />
              <span>{{ t(locale, 'desktop.activity.exportSuccess') }}: {{ t(locale, 'desktop.activity.exportRows', { count: exportResult.rowCount }) }}</span>
              <span v-if="exportResult.truncated" class="text-amber-600 dark:text-amber-300">{{ t(locale, 'desktop.activity.exportTruncated') }}</span>
            </div>
            <div class="mt-1.5 flex items-center gap-1.5">
              <span class="min-w-0 flex-1 truncate font-mono text-[10px] text-[var(--theme-text-secondary)]" :title="exportResult.filePath">{{ exportResult.filePath }}</span>
              <button
                type="button"
                class="rounded p-1 text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)]"
                :aria-label="t(locale, 'desktop.activity.exportCopyPath')"
                :title="t(locale, 'desktop.activity.exportCopyPath')"
                @click="copyExportPath"
              >
                <Copy class="h-3 w-3" aria-hidden="true" />
              </button>
              <span v-if="exportCopiedFlash" class="text-[10px] font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.activity.copied') }}</span>
            </div>
          </div>
        </div>

        <!-- 操作 -->
        <div class="mt-4 flex justify-end gap-2">
          <button
            type="button"
            class="theme-button-secondary inline-flex h-8 items-center rounded-lg px-3 text-[11.5px] font-semibold disabled:opacity-60"
            :disabled="exportBusy"
            @click="closeExportDialog"
          >
            {{ t(locale, 'common.cancel') }}
          </button>
          <button
            type="button"
            class="inline-flex h-8 items-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3.5 text-[11.5px] font-semibold text-[var(--theme-accent-contrast)] transition-opacity hover:opacity-90 disabled:opacity-60"
            :disabled="exportBusy"
            @click="runExport"
          >
            <Loader2 v-if="exportBusy" class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
            {{ exportBusy ? t(locale, 'desktop.activity.exportBusy') : t(locale, 'desktop.activity.exportButton') }}
          </button>
        </div>
      </div>
    </div>
  </section>
</template>
