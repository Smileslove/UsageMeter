<script setup lang="ts">
import { LayoutGrid, ChevronDown } from 'lucide-vue-next'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import type { ProjectStats, ProjectToolStats, RequestRecord, SessionStats } from '../types'
import SessionDetailModal from '../components/SessionDetailModal.vue'
import LobeIcon from '../components/LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'
import { getFamilyForTool, getFamilyHead } from '../toolFamilies'
import { formatCost as formatCostUtil, formatTokenValue, formatRequestCount } from '../utils/format'
import { formatModelDisplayName } from '../utils/modelDisplay'
import { formatToolDisplayName, formatToolFilterDisplayName, getToolProfileByTool } from '../utils/toolDisplay'

const store = useMonitorStore()
const SESSION_SOURCE_TOOLS = new Set([
  'claude_code', 'codex', 'hermes', 'openclaw', 'opencode',
  'qoder_ide', 'qoder_ide_cn', 'qoder_cli', 'qoder_work', 'qoder_work_cn',
  'reasonix', 'copilot'
])
const normalizeSessionTool = (tool: string | null | undefined) => (
  tool && SESSION_SOURCE_TOOLS.has(tool) ? tool : null
)
const uuidLikePattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

// 视图切换状态
const activeTab = ref<'recent' | 'requests' | 'projects'>('recent')
const selectedTool = ref<string | null>(normalizeSessionTool(store.settings.clientTools.activeToolFilter))
const lastGlobalTool = ref<string | null>(normalizeSessionTool(store.settings.clientTools.activeToolFilter))
// 已展开子菜单的家族 head ID
const expandedFamily = ref<string | null>(null)
const filterDropdownOpen = ref(false)
const filterDropdownRef = ref<HTMLElement | null>(null)

// 选中的会话（用于模态框）
const selectedSession = ref<SessionStats | null>(null)
const showModal = ref(false)
const selectedRequest = ref<RequestRecord | null>(null)
const showRequestModal = ref(false)

interface SessionCacheEntry {
  items: SessionStats[]
  currentPage: number
  hasMore: boolean
}

interface RequestCacheEntry {
  items: RequestRecord[]
  currentPage: number
  hasMore: boolean
}

const sessionCache = new Map<string, SessionCacheEntry>()
const requestCache = new Map<string, RequestCacheEntry>()
const projectCache = new Map<string, ProjectStats[]>()
const lastProxyRecordCount = ref<number | null>(null)
const proxyRefreshDebounceMs = 1500
const copiedProjectPath = ref<string | null>(null)
let proxyRefreshTimer: ReturnType<typeof setTimeout> | null = null
let copiedProjectPathTimer: ReturnType<typeof setTimeout> | null = null

const cacheKey = () => `${selectedTool.value ?? '__all__'}`

interface SourceOption {
  key: string
  tool: string | null
  label: string
  icon: string | null
  /** 若非 null，点击此条目展开/收起子菜单而非直接筛选 */
  familyHead: string | null
  children: SourceOption[]
}

const sourceOptions = computed<SourceOption[]>(() => {
  const profiles = store.settings.clientTools.profiles
    .filter(profile => SESSION_SOURCE_TOOLS.has(profile.tool))
    .sort((a, b) => {
      if (a.tool === 'claude_code') return -1
      if (b.tool === 'claude_code') return 1
      return (a.displayName || a.tool).localeCompare(b.displayName || b.tool)
    })

  const items: SourceOption[] = [
    { key: '__all__', tool: null, label: t(store.settings.locale, 'tools.all'), icon: null, familyHead: null, children: [] }
  ]

  for (const profile of profiles) {
    const family = getFamilyForTool(profile.tool)
    // 只有家族 head 本身出现在一级列表
    if (family && profile.tool === family.head) {
      const subItems: SourceOption[] = family.members.map(memberId => ({
        key: memberId,
        tool: memberId,
        label: formatToolDisplayName(memberId, store.settings.locale, profiles),
        icon: TOOL_LOBE_ICONS[memberId] || null,
        familyHead: null,
        children: [],
      }))
      items.push({
        key: profile.tool,
        tool: profile.tool,       // 点击 head → 家族整体过滤
        label: formatToolFilterDisplayName(profile.tool, store.settings.locale, profiles),
        icon: profile.icon || TOOL_LOBE_ICONS[profile.tool] || null,
        familyHead: family.head,  // 标记为家族 head，模板据此渲染展开箭头
        children: subItems,
      })
    } else if (!family) {
      // 非家族成员，直接一级条目
      items.push({
        key: profile.tool,
        tool: profile.tool,
        label: formatToolDisplayName(profile.tool, store.settings.locale, profiles),
        icon: profile.icon || TOOL_LOBE_ICONS[profile.tool] || null,
        familyHead: null,
        children: [],
      })
    }
    // 家族非 head 成员不出现在一级，由 head 的 children 承载
  }
  return items
})

const menuSourceOptions = computed(() => sourceOptions.value.filter(option => option.key !== '__all__'))

const activeFamilyHead = computed(() => (
  selectedTool.value ? getFamilyHead(selectedTool.value) : null
))

const currentSourceOption = computed<SourceOption>(() => {
  if (selectedTool.value === null) {
    return sourceOptions.value[0]
  }

  for (const option of sourceOptions.value) {
    if (option.tool === selectedTool.value) {
      return option
    }
    const child = option.children.find(item => item.tool === selectedTool.value)
    if (child) {
      return child
    }
  }

  return sourceOptions.value[0]
})

const closeFilterDropdown = () => {
  filterDropdownOpen.value = false
  expandedFamily.value = null
}

const toggleFilterDropdown = () => {
  const next = !filterDropdownOpen.value
  filterDropdownOpen.value = next
  if (!next) {
    expandedFamily.value = null
    return
  }
  expandedFamily.value = activeFamilyHead.value && activeFamilyHead.value !== selectedTool.value
    ? activeFamilyHead.value
    : null
}

const selectSourceTool = (tool: string | null) => {
  selectedTool.value = tool
  closeFilterDropdown()
}

const toggleFamilyMenu = (headId: string) => {
  expandedFamily.value = expandedFamily.value === headId ? null : headId
}

const handleFilterClickOutside = (event: MouseEvent) => {
  if (!filterDropdownRef.value?.contains(event.target as Node)) {
    closeFilterDropdown()
  }
}

const applySessionCache = (entry: SessionCacheEntry) => {
  store.sessions = entry.items
  currentPage.value = entry.currentPage
  hasMore.value = entry.hasMore
  store.sessionsLoading = false
}

const rememberSessionCache = (key: string) => {
  sessionCache.set(key, {
    items: [...store.sessions],
    currentPage: currentPage.value,
    hasMore: hasMore.value
  })
}

const clearViewCaches = () => {
  sessionCache.clear()
  requestCache.clear()
  projectCache.clear()
}

const projectCacheKey = () => `projects:${selectedTool.value ?? '__all__'}`

const triggerSessionViewRefresh = async () => {
  clearViewCaches()
  if (activeTab.value === 'requests') {
    await reloadRequestRecords(true)
  } else {
    await reloadSessions(true)
  }
  if (activeTab.value === 'projects') {
    await reloadProjectStats(true)
  } else {
    store.projectStats = []
  }
}

const scheduleProxyRefresh = () => {
  if (proxyRefreshTimer) {
    clearTimeout(proxyRefreshTimer)
  }
  proxyRefreshTimer = setTimeout(() => {
    proxyRefreshTimer = null
    void triggerSessionViewRefresh()
  }, proxyRefreshDebounceMs)
}

const reloadSessions = async (force = false) => {
  const key = cacheKey()
  if (!force) {
    const cached = sessionCache.get(key)
    if (cached) {
      applySessionCache(cached)
      return
    }
  }

  currentPage.value = 0
  hasMore.value = true
  const count = await store.fetchSessionsForTool(selectedTool.value, pageSize, 0, false)
  if (count < pageSize) {
    hasMore.value = false
  }
  rememberSessionCache(key)
}

const reloadProjectStats = async (force = false) => {
  const key = projectCacheKey()
  if (!force) {
    const cached = projectCache.get(key)
    if (cached) {
      store.projectStats = [...cached]
      store.projectStatsLoading = false
      return
    }
  }

  await store.fetchProjectStatsForTool(selectedTool.value)
  projectCache.set(key, [...store.projectStats])
}

// 切换 tab 时加载项目统计
watch(activeTab, async (newTab) => {
  closeFilterDropdown()
  if (newTab === 'requests') {
    await reloadRequestRecords()
  }
  if (newTab === 'projects') {
    await reloadProjectStats()
  }
})

// 分页状态
const currentPage = ref(0)
const pageSize = 30
const hasMore = ref(true)
const loadingMore = ref(false)
const requestCurrentPage = ref(0)
const requestPageSize = 30
const requestHasMore = ref(true)
const loadingMoreRequests = ref(false)

const applyRequestCache = (entry: RequestCacheEntry) => {
  store.requestRecords = entry.items
  requestCurrentPage.value = entry.currentPage
  requestHasMore.value = entry.hasMore
  store.requestRecordsLoading = false
}

const rememberRequestCache = (key: string) => {
  requestCache.set(key, {
    items: [...store.requestRecords],
    currentPage: requestCurrentPage.value,
    hasMore: requestHasMore.value
  })
}

const reloadRequestRecords = async (force = false) => {
  const key = cacheKey()
  if (!force) {
    const cached = requestCache.get(key)
    if (cached) {
      applyRequestCache(cached)
      return
    }
  }

  requestCurrentPage.value = 0
  requestHasMore.value = true
  const count = await store.fetchRecentRequestRecordsForTool(selectedTool.value, requestPageSize, 0, false)
  if (count < requestPageSize) {
    requestHasMore.value = false
  }
  rememberRequestCache(key)
}

// 格式化时间
const formatTime = (epoch: number) => {
  if (!epoch) return '-'
  const date = new Date(epoch * 1000)
  const now = new Date()
  const diffMs = now.getTime() - date.getTime()
  const diffMins = Math.floor(diffMs / 60000)
  const diffHours = Math.floor(diffMs / 3600000)

  const isSameYear = date.getFullYear() === now.getFullYear()
  const locale = store.settings.locale.replace('_', '-') // 确保传入合法的语言标签(如 zh-CN)

  // 1小时内显示分钟前，24小时内显示小时前
  if (diffMins < 1) return t(store.settings.locale, 'common.justNow')
  if (diffMins < 60) return t(store.settings.locale, 'sessions.timeMinutesAgo', { count: diffMins })
  if (diffHours < 24) return t(store.settings.locale, 'sessions.timeHoursAgo', { count: diffHours })

  // 超过24小时就直接显示具体的月日+时间(如果是当年)，否则带上年份
  return date.toLocaleString(locale, {
    year: isSameYear ? undefined : 'numeric',
    month: 'numeric',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit'
  })
}

// 格式化 Token 数量（保留2位小数，超过K/M/B自动换算单位）
const formatTokens = (tokens: number) => {
  if (!tokens) return '0'
  return formatTokenValue(tokens)
}

// 格式化费用（统一4位小数，支持多货币）
const formatCost = (cost: number | undefined) => {
  if (cost === undefined || cost === null) return '-'
  return formatCostUtil(cost, store.settings.currency, 4)
}

const formatDuration = (ms?: number | null) => {
  if (!ms) return '—'
  if (ms < 1000) return `${ms}ms`
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`
  const minutes = Math.floor(ms / 60000)
  const seconds = Math.round((ms % 60000) / 1000)
  return `${minutes}m ${seconds}s`
}

const compactModelName = (request: RequestRecord) => {
  const value = formatModelDisplayName(request.model, request.tool, store.settings.locale, store.settings.clientTools.profiles)
  return value
    .replace(/^claude-/, '')
    .replace(/^gpt-/, 'gpt-')
    .replace(/-20\d{6}$/, '')
}

const requestModelLabel = (request: RequestRecord) => (
  formatModelDisplayName(request.model, request.tool, store.settings.locale, store.settings.clientTools.profiles)
)

const requestStatusLabel = (request: RequestRecord) => {
  if (request.coverageOrigin === 'local_only') return t(store.settings.locale, 'sessions.requestLocalOnly')
  const statusCode = request.statusCode
  if (!statusCode) return t(store.settings.locale, 'common.unknown')
  if (statusCode < 400) return t(store.settings.locale, 'common.success')
  return t(store.settings.locale, 'common.error')
}

const requestStatusClasses = (request: RequestRecord) => {
  if (request.coverageOrigin === 'local_only') {
    return 'bg-slate-50 text-slate-500 border-slate-100 dark:bg-white/[0.04] dark:text-slate-300 dark:border-white/8'
  }
  const statusCode = request.statusCode || 0
  if (statusCode >= 400) {
    return 'bg-rose-50 text-rose-600 border-rose-100 dark:bg-rose-500/15 dark:text-rose-300 dark:border-rose-400/20'
  }
  return 'bg-emerald-50 text-emerald-600 border-emerald-100 dark:bg-emerald-500/15 dark:text-emerald-300 dark:border-emerald-400/20'
}

const requestCoverageLabel = (origin: RequestRecord['coverageOrigin']) => {
  if (origin === 'proxy_only') return t(store.settings.locale, 'sessions.requestCoverageProxy')
  if (origin === 'merged_proxy_preferred') return t(store.settings.locale, 'sessions.requestCoverageMerged')
  return t(store.settings.locale, 'sessions.requestCoverageLocal')
}

const requestProjectLabel = (request: RequestRecord) => {
  const pathParts = request.projectPath?.split('/').filter(Boolean) || []
  return request.projectName?.trim()
    || pathParts[pathParts.length - 1]
    || t(store.settings.locale, 'common.unknownProject')
}

const requestSourceLabel = (request: RequestRecord) => (
  request.sourceLabel?.trim()
    || request.requestBaseUrl?.trim()
    || t(store.settings.locale, 'sources.unknown')
)

const requestToolLabel = (tool: string) => {
  return formatToolDisplayName(tool, store.settings.locale, store.settings.clientTools.profiles)
}

const requestCacheTokens = (request: RequestRecord) => (
  (request.cacheCreateTokens || 0) + (request.cacheReadTokens || 0)
)

const requestHasProxyPerformance = (request: RequestRecord) => (
  request.durationMs !== null && request.durationMs !== undefined
)

const sessionCacheHitRate = (session: SessionStats): string => {
  const total = (session.totalInputTokens || 0) + (session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)
  if (total === 0) return '—'
  return `${((session.totalCacheReadTokens || 0) / total * 100).toFixed(1)}%`
}

const coveredRequests = (value?: number) => value || 0
const uncoveredRequests = (value?: number) => value || 0
const localRequests = (session: SessionStats) => coveredRequests(session.coveredRequests) + uncoveredRequests(session.uncoveredRequests)
const hasReasonixCoverageData = (covered?: number, uncovered?: number, usageFullyCovered?: boolean) => (
  (covered || 0) > 0 || (uncovered || 0) > 0 || usageFullyCovered === false
)
const sessionUsageVisible = (session: SessionStats) => (
  session.tool !== 'reasonix' || hasReasonixCoverageData(session.coveredRequests, session.uncoveredRequests, session.usageFullyCovered)
)
const projectUsageVisible = (project: ProjectStats) => (
  !project.toolBreakdown?.some(tool => tool.tool === 'reasonix')
  || hasReasonixCoverageData(project.coveredRequests, project.uncoveredRequests, project.usageFullyCovered)
)
const projectToolUsageVisible = (tool: ProjectToolStats) => (
  tool.tool !== 'reasonix'
  || hasReasonixCoverageData(tool.coveredRequests, tool.uncoveredRequests, tool.usageFullyCovered)
)
const sessionHasPartialCoverage = (session: SessionStats) => (
  session.tool === 'reasonix' && uncoveredRequests(session.uncoveredRequests) > 0
)
const displayTokens = (value: number, visible: boolean) => (visible ? formatTokens(value) : '—')
const displayCost = (value: number | undefined, visible: boolean) => (visible ? formatCost(value) : '—')
const displayRequestValue = (value: number) => formatRequestCount(value)
const displaySessionCacheHitRate = (session: SessionStats) => (
  sessionUsageVisible(session) ? sessionCacheHitRate(session) : '—'
)
const displaySessionPrimaryLabel = (session: SessionStats) => (
  sessionHasPartialCoverage(session)
    ? t(store.settings.locale, 'common.covered')
    : t(store.settings.locale, 'common.totalTokens')
)
const displaySessionPrimaryValue = (session: SessionStats) => (
  sessionHasPartialCoverage(session)
    ? displayRequestValue(coveredRequests(session.coveredRequests))
    : displayTokens(
      (session.totalInputTokens || 0)
        + (session.totalOutputTokens || 0)
        + (session.totalCacheCreateTokens || 0)
        + (session.totalCacheReadTokens || 0),
      sessionUsageVisible(session)
    )
)
const displayProjectCoverageHint = (project: ProjectStats) => (
  uncoveredRequests(project.uncoveredRequests) > 0
    ? t(store.settings.locale, 'sessions.uncoveredRequests', { count: uncoveredRequests(project.uncoveredRequests) })
    : ''
)
const displayToolCoverageHint = (tool: ProjectToolStats) => (
  uncoveredRequests(tool.uncoveredRequests) > 0
    ? t(store.settings.locale, 'sessions.uncoveredRequests', { count: uncoveredRequests(tool.uncoveredRequests) })
    : ''
)

const displaySessionTitle = (session: SessionStats) => {
  const sessionName = session.sessionName?.trim()
  if (session.topic?.trim()) return session.topic
  if (sessionName && !uuidLikePattern.test(sessionName)) return sessionName
  if (session.lastPrompt?.trim()) return session.lastPrompt
  if (session.projectName?.trim()) return session.projectName
  return t(store.settings.locale, 'sessions.untitled')
}

const displaySessionProjectBadge = (session: SessionStats) => {
  if (session.projectName?.trim()) return session.projectName
  if (session.projectIdentity === 'global') return t(store.settings.locale, 'common.global')
  if (session.projectIdentity === 'unknown') return t(store.settings.locale, 'common.unknownProject')
  return ''
}

const projectBadgeClasses = (identity?: string) => {
  if (identity === 'global') {
    return 'bg-slate-50 text-slate-500 dark:bg-slate-500/15 dark:text-slate-300 border border-slate-100 dark:border-slate-400/20'
  }
  if (identity === 'unknown') {
    return 'bg-amber-50 text-amber-600 dark:bg-amber-500/15 dark:text-amber-300 border border-amber-100 dark:border-amber-400/20'
  }
  return 'bg-indigo-50 text-indigo-500 dark:bg-indigo-500/20 dark:text-indigo-300 border border-indigo-100 dark:border-indigo-500/30'
}

const displayProjectName = (project: ProjectStats) => {
  if (project.projectIdentity === 'global') return t(store.settings.locale, 'common.global')
  if (project.projectIdentity === 'unknown') return t(store.settings.locale, 'common.unknownProject')
  return project.name
}

const displayProjectHint = (project: ProjectStats) => {
  if (project.projectIdentity === 'global') return t(store.settings.locale, 'sessions.globalSessionHint')
  if (project.projectIdentity === 'unknown') return t(store.settings.locale, 'sessions.unknownProjectHint')
  return ''
}

const projectWslDistros = (project: ProjectStats) => {
  const distros = project.wslDistros?.filter(distro => distro.trim()) || []
  if (distros.length > 0) return distros
  return project.wslDistro ? [project.wslDistro] : []
}

const displayProjectWslBadge = (project: ProjectStats) => {
  const distros = projectWslDistros(project)
  if (distros.length === 0) return ''
  if (distros.length === 1) return distros[0]
  return `${distros[0]} +${distros.length - 1}`
}

const displayProjectWslTitle = (project: ProjectStats) => {
  const distros = projectWslDistros(project)
  if (distros.length === 0) return ''
  return t(store.settings.locale, 'sessions.wslBadgeTitle', { distro: distros.join(', ') })
}

const displayProxyTokenValue = (session: SessionStats) => (
  coveredRequests(session.coveredRequests) > 0
    ? formatTokens(
      (session.totalInputTokens || 0)
        + (session.totalOutputTokens || 0)
        + (session.totalCacheCreateTokens || 0)
        + (session.totalCacheReadTokens || 0)
    )
    : '—'
)

const displayProxyRateValue = (session: SessionStats) => (
  coveredRequests(session.coveredRequests) > 0 && (session.avgOutputTokensPerSecond || 0) > 0
    ? `${session.avgOutputTokensPerSecond.toFixed(1)}t/s`
    : '—'
)

const getToolProfile = (tool: string) => {
  return getToolProfileByTool(store.settings.clientTools.profiles, tool)
}

const getToolIcon = (tool: string) => {
  const profile = getToolProfile(tool)
  return profile?.icon || TOOL_LOBE_ICONS[tool] || TOOL_LOBE_ICONS[getFamilyHead(tool)] || null
}

const projectToolRows = (project: ProjectStats) => {
  const rows = project.toolBreakdown?.length
    ? project.toolBreakdown
    : [{
        tool: 'unknown',
        requestCount: project.requestCount,
        sessionCount: project.sessionCount,
        totalInputTokens: project.totalInputTokens,
        totalOutputTokens: project.totalOutputTokens,
        totalCacheCreateTokens: project.totalCacheCreateTokens,
        totalCacheReadTokens: project.totalCacheReadTokens,
        totalCost: project.totalCost,
        lastActive: project.lastActive
      }]

  return [...rows].sort((a, b) => b.lastActive - a.lastActive)
}

const projectTotalTokens = (project: ProjectStats) => (
  project.totalInputTokens
  + project.totalOutputTokens
  + project.totalCacheCreateTokens
  + project.totalCacheReadTokens
)

const shouldShowProjectTotalRow = (project: ProjectStats) => projectToolRows(project).length > 1

const copyProjectPath = async (projectPath?: string | null) => {
  if (!projectPath) return

  let copied = false

  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(projectPath)
      copied = true
    } else {
      const textarea = document.createElement('textarea')
      textarea.value = projectPath
      textarea.setAttribute('readonly', 'true')
      textarea.style.position = 'fixed'
      textarea.style.opacity = '0'
      document.body.appendChild(textarea)
      textarea.select()
      copied = document.execCommand('copy')
      document.body.removeChild(textarea)
    }
  } catch {
    return
  }

  if (!copied) return

  copiedProjectPath.value = projectPath
  if (copiedProjectPathTimer) {
    clearTimeout(copiedProjectPathTimer)
  }
  copiedProjectPathTimer = setTimeout(() => {
    copiedProjectPath.value = null
    copiedProjectPathTimer = null
  }, 1200)
}

// 打开会话详情
const openSessionDetail = (session: SessionStats) => {
  selectedSession.value = session
  showModal.value = true
}

// 关闭模态框
const closeModal = () => {
  showModal.value = false
  selectedSession.value = null
}

const openRequestDetail = (request: RequestRecord) => {
  selectedRequest.value = request
  showRequestModal.value = true
}

const closeRequestModal = () => {
  showRequestModal.value = false
  selectedRequest.value = null
}

// 加载更多
const loadMore = async () => {
  if (loadingMore.value || !hasMore.value) return

  loadingMore.value = true
  currentPage.value++

  const count = await store.fetchSessionsForTool(selectedTool.value, pageSize, currentPage.value * pageSize, true)
  if (count < pageSize) {
    hasMore.value = false
  }
  rememberSessionCache(cacheKey())
  loadingMore.value = false
}

const loadMoreRequests = async () => {
  if (loadingMoreRequests.value || !requestHasMore.value) return
  const remaining = 200 - store.requestRecords.length
  if (remaining <= 0) {
    requestHasMore.value = false
    return
  }

  loadingMoreRequests.value = true
  requestCurrentPage.value++
  const nextLimit = Math.min(requestPageSize, remaining)

  const count = await store.fetchRecentRequestRecordsForTool(
    selectedTool.value,
    nextLimit,
    requestCurrentPage.value * requestPageSize,
    true
  )
  if (count < nextLimit || store.requestRecords.length >= 200) {
    requestHasMore.value = false
  }
  rememberRequestCache(cacheKey())
  loadingMoreRequests.value = false
}

// 监听工具筛选变化
watch([selectedTool], async () => {
  if (activeTab.value === 'requests') {
    await reloadRequestRecords()
  } else {
    await reloadSessions()
  }
  if (activeTab.value === 'projects') {
    await reloadProjectStats()
  } else {
    store.projectStats = []
  }
})

watch(() => store.settings.clientTools.activeToolFilter, globalTool => {
  const normalizedGlobalTool = normalizeSessionTool(globalTool)
  if (selectedTool.value === lastGlobalTool.value) {
    selectedTool.value = normalizedGlobalTool
  }
  lastGlobalTool.value = normalizedGlobalTool
})

watch(() => store.sessionViewsRevision, async (current, previous) => {
  if (current === previous) {
    return
  }

  await triggerSessionViewRefresh()
})

watch(() => store.proxyStatus?.recordCount ?? null, async (current, previous) => {
  if (current === null) {
    lastProxyRecordCount.value = current
    return
  }

  if (previous === null || lastProxyRecordCount.value === null) {
    lastProxyRecordCount.value = current
    return
  }

  if (current <= lastProxyRecordCount.value) {
    lastProxyRecordCount.value = current
    return
  }

  lastProxyRecordCount.value = current
  scheduleProxyRefresh()
})

// 触底加载触发元素
const loadMoreTrigger = ref<HTMLElement | null>(null)
const requestLoadMoreTrigger = ref<HTMLElement | null>(null)
let observer: IntersectionObserver | null = null

const observeLoadTriggers = () => {
  if (!observer) return
  if (loadMoreTrigger.value) {
    observer.observe(loadMoreTrigger.value)
  }
  if (requestLoadMoreTrigger.value) {
    observer.observe(requestLoadMoreTrigger.value)
  }
}

watch(activeTab, () => {
  setTimeout(observeLoadTriggers, 50)
})

watch(() => store.requestRecords.length, () => {
  setTimeout(observeLoadTriggers, 50)
})

watch(() => store.sessions.length, () => {
  setTimeout(observeLoadTriggers, 50)
})

// 初始加载
onMounted(async () => {
  document.addEventListener('click', handleFilterClickOutside)
  await reloadSessions()

  // 监听触底加载
  setTimeout(() => {
    observer = new IntersectionObserver(
      entries => {
        if (!entries[0].isIntersecting) return
        if (entries[0].target === loadMoreTrigger.value && hasMore.value && !loadingMore.value) {
          loadMore()
        }
        if (entries[0].target === requestLoadMoreTrigger.value && requestHasMore.value && !loadingMoreRequests.value) {
          loadMoreRequests()
        }
      },
      { root: null, rootMargin: '100px' }
    )
    observeLoadTriggers()
  }, 100)
})

onUnmounted(() => {
  document.removeEventListener('click', handleFilterClickOutside)
  if (observer) {
    observer.disconnect()
  }
  if (proxyRefreshTimer) {
    clearTimeout(proxyRefreshTimer)
    proxyRefreshTimer = null
  }
  if (copiedProjectPathTimer) {
    clearTimeout(copiedProjectPathTimer)
    copiedProjectPathTimer = null
  }
})
</script>

<template>
  <div class="space-y-2 animate-in fade-in zoom-in-95 duration-300 pb-4 min-h-full">
    <!-- 顶部视图切换 Tabs -->
    <div class="session-tabs sticky top-0 z-10 mb-2 backdrop-blur-md">
      <button
        type="button"
        class="session-tabs__item"
        :class="{ 'session-tabs__item--on': activeTab === 'recent' }"
        @click="activeTab = 'recent'"
      >
        {{ t(store.settings.locale, 'sessions.tabs.recent') }}
      </button>
      <button
        type="button"
        class="session-tabs__item"
        :class="{ 'session-tabs__item--on': activeTab === 'requests' }"
        @click="activeTab = 'requests'"
      >
        {{ t(store.settings.locale, 'sessions.tabs.requests') }}
      </button>
      <button
        type="button"
        class="session-tabs__item"
        :class="{ 'session-tabs__item--on': activeTab === 'projects' }"
        @click="activeTab = 'projects'"
      >
        {{ t(store.settings.locale, 'sessions.tabs.projects') }}
      </button>
    </div>

    <div v-if="activeTab === 'recent' || activeTab === 'requests'" ref="filterDropdownRef" class="tool-filter">
      <button
        type="button"
        class="tool-filter__trigger"
        :class="{ 'tool-filter__trigger--open': filterDropdownOpen }"
        :aria-expanded="filterDropdownOpen"
        :title="currentSourceOption.label"
        @click="toggleFilterDropdown"
      >
        <LayoutGrid v-if="!currentSourceOption.icon" class="h-3.5 w-3.5 shrink-0" />
        <LobeIcon v-else :slug="currentSourceOption.icon" :size="14" @error="() => {}" />
        <span class="tool-filter__current">{{ currentSourceOption.label }}</span>
        <ChevronDown :class="['h-3.5 w-3.5 shrink-0 transition-transform duration-150', filterDropdownOpen && 'rotate-180']" />
      </button>

      <Transition
        enter-active-class="transition ease-out duration-120"
        enter-from-class="transform opacity-0 -translate-y-1"
        enter-to-class="transform opacity-100 translate-y-0"
        leave-active-class="transition ease-in duration-100"
        leave-from-class="transform opacity-100 translate-y-0"
        leave-to-class="transform opacity-0 -translate-y-1"
      >
        <div v-if="filterDropdownOpen" class="tool-filter__menu">
          <button
            type="button"
            class="tool-filter__menu-item"
            :class="{ 'tool-filter__menu-item--on': selectedTool === null }"
            @click="selectSourceTool(null)"
          >
            <LayoutGrid class="h-3.5 w-3.5 shrink-0" />
            <span class="truncate">{{ t(store.settings.locale, 'tools.all') }}</span>
          </button>

          <div class="tool-filter__menu-list">
            <template v-for="option in menuSourceOptions" :key="option.key">
              <div class="tool-filter__menu-row">
                <button
                  type="button"
                  class="tool-filter__menu-item tool-filter__menu-item--family"
                  :class="{ 'tool-filter__menu-item--on': selectedTool === option.tool }"
                  @click="selectSourceTool(option.tool)"
                >
                  <LobeIcon v-if="option.icon" :slug="option.icon" :size="14" @error="() => {}" />
                  <LayoutGrid v-else class="h-3.5 w-3.5 shrink-0" />
                  <span class="truncate">{{ option.label }}</span>
                </button>
                <button
                  v-if="option.familyHead"
                  type="button"
                  class="tool-filter__menu-expand"
                  :title="expandedFamily === option.familyHead ? t(store.settings.locale, 'tools.collapseVariants') : t(store.settings.locale, 'tools.expandVariants')"
                  @click.stop="toggleFamilyMenu(option.familyHead)"
                >
                  <ChevronDown :class="['h-3 w-3 transition-transform duration-150', expandedFamily === option.familyHead && 'rotate-180']" />
                </button>
              </div>

              <div v-if="option.familyHead && expandedFamily === option.familyHead" class="tool-filter__submenu">
                <button
                  type="button"
                  class="tool-filter__menu-item tool-filter__menu-item--child"
                  :class="{ 'tool-filter__menu-item--on': selectedTool === option.tool }"
                  @click="selectSourceTool(option.tool)"
                >
                  <LayoutGrid class="h-3 w-3 shrink-0" />
                  <span class="truncate">{{ t(store.settings.locale, 'tools.familyAll') }}</span>
                </button>
                <button
                  v-for="child in option.children"
                  :key="child.key"
                  type="button"
                  class="tool-filter__menu-item tool-filter__menu-item--child"
                  :class="{ 'tool-filter__menu-item--on': selectedTool === child.tool }"
                  @click="selectSourceTool(child.tool)"
                >
                  <LobeIcon v-if="child.icon" :slug="child.icon" :size="12" @error="() => {}" />
                  <LayoutGrid v-else class="h-3 w-3 shrink-0" />
                  <span class="truncate">{{ child.label }}</span>
                </button>
              </div>
            </template>
          </div>
        </div>
      </Transition>
    </div>

    <!-- 1. 会话列表视图 -->
    <template v-if="activeTab === 'recent'">
      <div v-if="store.sessionsLoading" class="flex justify-center py-8">
        <div class="animate-spin w-5 h-5 border-2 border-blue-500 border-t-transparent rounded-full"></div>
      </div>

      <div v-else-if="store.sessions.length === 0" class="text-center py-8 text-gray-400 text-sm">
        {{ t(store.settings.locale, 'sessions.noData') }}
      </div>

      <div v-for="session in store.sessions" :key="session.sessionId" class="bg-white dark:bg-[#1E2024] rounded-xl border border-gray-100 dark:border-white/5 px-2.5 py-2 hover:bg-gray-50 dark:hover:bg-white/5 transition-colors cursor-pointer flex flex-col gap-1.5" @click="openSessionDetail(session)">
        <!-- 1. 顶部信息行 -->
        <div class="flex items-center justify-between w-full gap-2 min-w-0">
          <!-- 左侧：项目名 + 模型 -->
          <div class="flex items-center gap-1.5 min-w-0 shrink">
            <span
              v-if="displaySessionProjectBadge(session)"
              class="shrink-0 text-[10px] font-semibold px-1.5 py-px rounded truncate max-w-[130px]"
              :class="projectBadgeClasses(session.projectIdentity)"
            >
              {{ displaySessionProjectBadge(session) }}
            </span>
            <span
              v-if="session.wslDistro"
              class="shrink-0 inline-flex items-center gap-0.5 text-[10px] font-semibold px-1.5 py-px rounded truncate max-w-[110px] bg-cyan-50 text-cyan-600 border border-cyan-100 dark:bg-cyan-500/15 dark:text-cyan-300 dark:border-cyan-400/20"
              :title="t(store.settings.locale, 'sessions.wslBadgeTitle', { distro: session.wslDistro })"
            >
              <svg class="w-2.5 h-2.5 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2m-2-4h.01M17 16h.01" /></svg>
              <span class="truncate">{{ session.wslDistro }}</span>
            </span>
            <div class="flex items-center gap-0.5 text-[10px] text-gray-400 dark:text-gray-500 min-w-0">
              <svg class="w-2.5 h-2.5 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19.428 15.428a2 2 0 00-1.022-.547l-2.387-.477a6 6 0 00-3.86.517l-.318.158a6 6 0 01-3.86.517L6.05 15.21a2 2 0 00-1.806.547M8 4h8l-1 1v5.172a2 2 0 00.586 1.414l5 5c1.26 1.26.367 3.414-1.415 3.414H4.828c-1.782 0-2.674-2.154-1.414-3.414l5-5A2 2 0 009 10.172V5L8 4z" /></svg>
              <span class="truncate">{{ session.models[0] || 'Unknown' }}</span>
            </div>
          </div>
          <!-- 右侧：时间 -->
          <div class="flex items-center gap-1 shrink-0 text-[10px] text-gray-400 dark:text-gray-500">
            <svg class="w-2.5 h-2.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
            <span>{{ formatTime(session.lastRequestTime) }}</span>
          </div>
        </div>

        <!-- 话题标题 -->
        <p class="text-[12px] font-medium text-gray-800 dark:text-gray-200 line-clamp-1 leading-snug">
          {{ displaySessionTitle(session) }}
        </p>
        <!-- 2. 底部数据行：单行横排 -->
        <div class="flex items-center justify-between pt-1.5 border-t border-gray-100 dark:border-white/5 text-[10px]">
          <template v-if="sessionHasPartialCoverage(session)">
            <div class="flex items-center gap-0.5">
              <svg class="w-[10px] h-[10px] text-orange-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 12h16" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.localRecords') }}</span>
              <span class="text-gray-700 dark:text-gray-300 font-semibold ml-0.5">{{ displayRequestValue(localRequests(session)) }}</span>
            </div>
            <div class="flex items-center gap-0.5">
              <svg class="w-[10px] h-[10px] text-cyan-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.proxyRecords') }}</span>
              <span class="text-gray-700 dark:text-gray-300 font-semibold ml-0.5">{{ displayRequestValue(coveredRequests(session.coveredRequests)) }}</span>
            </div>
            <div class="flex items-center gap-0.5">
              <svg class="w-[10px] h-[10px] text-violet-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.proxyTokens') }}</span>
              <span class="text-gray-700 dark:text-gray-300 font-semibold ml-0.5">{{ displayProxyTokenValue(session) }}</span>
            </div>
            <div class="flex items-center gap-0.5">
              <svg class="w-[10px] h-[10px] text-yellow-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.proxyRate') }}</span>
              <span class="text-gray-700 dark:text-gray-300 font-semibold ml-0.5">{{ displayProxyRateValue(session) }}</span>
            </div>
          </template>
          <template v-else>
          <!-- 总 Token -->
          <div class="flex items-center gap-0.5">
            <svg class="w-[10px] h-[10px] text-orange-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" /></svg>
            <span class="text-gray-400">{{ displaySessionPrimaryLabel(session) }}</span>
            <span class="text-gray-700 dark:text-gray-300 font-semibold ml-0.5">{{ displaySessionPrimaryValue(session) }}</span>
          </div>
          <!-- 缓存命中率 -->
          <div class="flex items-center gap-0.5">
            <svg class="w-[10px] h-[10px] text-violet-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z" /></svg>
            <span class="text-gray-400">{{ t(store.settings.locale, 'statistics.cacheHitRate') }}</span>
            <span class="text-violet-500 dark:text-violet-400 font-semibold ml-0.5">{{ displaySessionCacheHitRate(session) }}</span>
          </div>
          <!-- 平均速率 -->
          <div class="flex items-center gap-0.5">
            <svg class="w-[10px] h-[10px] text-yellow-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z" /></svg>
            <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.avgRate') }}</span>
            <span class="text-gray-700 dark:text-gray-300 font-semibold ml-0.5">{{ sessionUsageVisible(session) && session.avgOutputTokensPerSecond > 0 ? `${session.avgOutputTokensPerSecond.toFixed(1)}t/s` : '—' }}</span>
          </div>
          <!-- 费用 -->
          <div class="flex items-center gap-0.5">
            <svg class="w-[10px] h-[10px] text-[#00E5FF] shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8c-1.657 0-3 .895-3 2s1.343 2 3 2 3 .895 3 2-1.343 2-3 2m0-8c1.11 0 2.08.402 2.599 1M12 8V7m0 1v8m0 0v1m0-1c-1.11 0-2.08-.402-2.599-1M21 12a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
            <span class="font-semibold text-[var(--theme-chart-cost)]">{{ displayCost(session.estimatedCost, sessionUsageVisible(session)) }}</span>
          </div>
          </template>
        </div>
      </div>

      <!-- 加载更多指示器 -->
      <div v-if="loadingMore" class="flex justify-center py-4">
        <div class="animate-spin w-4 h-4 border-2 border-gray-300 border-t-gray-500 rounded-full"></div>
      </div>

      <!-- 没有更多 -->
      <div v-else-if="!hasMore && store.sessions.length > 0" class="text-center py-3 text-[10px] text-gray-400">
        {{ t(store.settings.locale, 'common.noMore') }}
      </div>

      <!-- 触底检测点 -->
      <div ref="loadMoreTrigger" class="h-1 w-full"></div>
    </template>

    <!-- 2. 最近请求流 -->
    <template v-else-if="activeTab === 'requests'">
      <div class="request-stream-note">
        <span>{{ t(store.settings.locale, 'sessions.requestsRecentHint') }}</span>
        <span class="font-mono">{{ store.requestRecords.length }}/200</span>
      </div>

      <div v-if="store.requestRecordsLoading" class="flex justify-center py-8">
        <div class="animate-spin w-5 h-5 border-2 border-blue-500 border-t-transparent rounded-full"></div>
      </div>

      <div v-else-if="store.requestRecords.length === 0" class="text-center py-8 text-gray-400 text-sm">
        {{ t(store.settings.locale, 'sessions.noRequestData') }}
      </div>

      <template v-else>
        <button
          v-for="request in store.requestRecords"
          :key="request.requestKey"
          type="button"
          class="request-card"
          @click="openRequestDetail(request)"
        >
          <div class="request-card__top">
            <div class="min-w-0 flex items-center gap-1.5">
              <span class="request-card__time">{{ formatTime(request.timestampSec) }}</span>
              <span class="request-card__model" :title="requestModelLabel(request)">{{ compactModelName(request) }}</span>
            </div>
            <div class="flex shrink-0 items-center gap-1.5">
              <span class="request-card__tokens">{{ formatTokens(request.totalTokens) }}</span>
              <span class="request-card__cost">{{ formatCost(request.estimatedCost) }}</span>
            </div>
          </div>

          <div class="request-card__meta">
            <div class="min-w-0 flex items-center gap-1">
              <LobeIcon
                v-if="getToolIcon(request.tool)"
                :slug="getToolIcon(request.tool) ?? 'claudecode'"
                :size="12"
                @error="() => {}"
              />
              <span v-else class="h-1.5 w-1.5 shrink-0 rounded-full bg-gray-400"></span>
              <span class="truncate">{{ requestProjectLabel(request) }}</span>
              <span class="text-gray-300 dark:text-gray-600">/</span>
              <span class="truncate">{{ requestToolLabel(request.tool) }}</span>
              <span class="text-gray-300 dark:text-gray-600">/</span>
              <span class="truncate">{{ requestSourceLabel(request) }}</span>
            </div>
            <span class="request-card__status" :class="requestStatusClasses(request)">
              {{ requestStatusLabel(request) }}
            </span>
          </div>

          <div class="request-card__metrics">
            <span>{{ t(store.settings.locale, 'sessions.input') }} {{ formatTokens(request.inputTokens) }}</span>
            <span>{{ t(store.settings.locale, 'sessions.output') }} {{ formatTokens(request.outputTokens) }}</span>
            <span>{{ t(store.settings.locale, 'statistics.cache') }} {{ formatTokens(requestCacheTokens(request)) }}</span>
            <span class="ml-auto">{{ requestHasProxyPerformance(request) ? formatDuration(request.durationMs) : requestCoverageLabel(request.coverageOrigin) }}</span>
          </div>
        </button>

        <div v-if="loadingMoreRequests" class="flex justify-center py-4">
          <div class="animate-spin w-4 h-4 border-2 border-gray-300 border-t-gray-500 rounded-full"></div>
        </div>

        <div v-else-if="!requestHasMore && store.requestRecords.length > 0" class="text-center py-3 text-[10px] text-gray-400">
          {{ t(store.settings.locale, 'common.noMore') }}
        </div>

        <div ref="requestLoadMoreTrigger" class="h-1 w-full"></div>
      </template>
    </template>

    <!-- 3. 项目维度的聚合视图 -->
    <template v-else-if="activeTab === 'projects'">
      <!-- 加载状态 -->
      <div v-if="store.projectStatsLoading" class="flex justify-center py-8">
        <div class="animate-spin w-5 h-5 border-2 border-blue-500 border-t-transparent rounded-full"></div>
      </div>

      <!-- 空状态 -->
      <div v-else-if="store.projectStats.length === 0" class="text-center py-8 text-gray-400 text-sm">
        {{ t(store.settings.locale, 'sessions.noData') }}
      </div>

      <!-- 项目列表 -->
      <template v-else>
        <div v-for="project in store.projectStats" :key="project.projectKey || project.projectPath || project.name" class="bg-white dark:bg-[#1E2024] rounded-xl border border-gray-100 dark:border-white/5 py-2 px-2 hover:bg-gray-50 dark:hover:bg-white/5 transition-colors flex flex-col gap-1">
        <!-- 顶部：项目名称与最后活跃时间 -->
        <div class="flex items-center justify-between gap-2 w-full px-0.5">
          <div class="flex min-w-0 flex-1 items-center gap-1.5">
            <span class="shrink-0 max-w-[130px] truncate text-[10px] font-semibold px-1.5 py-px rounded leading-none" :class="projectBadgeClasses(project.projectIdentity)">
              {{ displayProjectName(project) }}
            </span>
            <span
              v-if="displayProjectWslBadge(project)"
              class="shrink-0 inline-flex items-center gap-0.5 text-[10px] font-semibold px-1.5 py-px rounded leading-none truncate max-w-[110px] bg-cyan-50 text-cyan-600 border border-cyan-100 dark:bg-cyan-500/15 dark:text-cyan-300 dark:border-cyan-400/20"
              :title="displayProjectWslTitle(project)"
            >
              <svg class="w-2.5 h-2.5 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2m-2-4h.01M17 16h.01" /></svg>
              <span class="truncate">{{ displayProjectWslBadge(project) }}</span>
            </span>
            <button
              v-if="project.projectPath"
              type="button"
              class="group flex min-w-0 flex-1 items-center gap-1 rounded-md border border-gray-200/90 bg-gray-50 px-1.5 py-0.5 text-left text-[10px] font-medium leading-none text-gray-500 transition-colors hover:border-sky-200 hover:bg-sky-50 hover:text-sky-700 dark:border-white/8 dark:bg-white/[0.03] dark:text-gray-400 dark:hover:border-sky-400/20 dark:hover:bg-sky-400/10 dark:hover:text-sky-300"
              :title="project.projectPath"
              @click.stop="copyProjectPath(project.projectPath)"
            >
              <svg class="h-2.5 w-2.5 shrink-0 text-gray-400 transition-colors group-hover:text-sky-500 dark:text-gray-500 dark:group-hover:text-sky-300" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7.5A2.5 2.5 0 015.5 5H9l2 2h7.5A2.5 2.5 0 0121 9.5v7A2.5 2.5 0 0118.5 19h-13A2.5 2.5 0 013 16.5v-9z" />
              </svg>
              <span class="block min-w-0 truncate font-mono">{{ project.projectPath }}</span>
              <span
                v-if="copiedProjectPath === project.projectPath"
                class="shrink-0 rounded bg-sky-100/90 px-1 py-[1px] text-[8px] font-semibold leading-none text-sky-600 dark:bg-sky-400/15 dark:text-sky-300"
              >
                {{ t(store.settings.locale, 'sessions.copied') }}
              </span>
            </button>
          </div>
          <div class="flex items-center gap-1 shrink-0 text-[10px] leading-none text-gray-400">
            <svg class="w-2.5 h-2.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
            <span>{{ t(store.settings.locale, 'sessions.lastActive') }}: {{ formatTime(project.lastActive).split(' ')[0] }}</span>
          </div>
        </div>
        <p
          v-if="displayProjectHint(project)"
          class="px-0.5 text-[10px] leading-none text-gray-400 dark:text-gray-500"
        >
          {{ displayProjectHint(project) }}
        </p>
        <p
          v-if="!projectUsageVisible(project) && displayProjectCoverageHint(project)"
          class="px-0.5 text-[10px] leading-none text-amber-600 dark:text-amber-300"
        >
          {{ displayProjectCoverageHint(project) }}
        </p>

        <!-- 项目统计表格 -->
        <div class="overflow-hidden rounded-xl border border-gray-100/90 bg-gray-50/40 dark:border-white/6 dark:bg-white/[0.02]">
          <div class="grid grid-cols-[36px_0.68fr_0.92fr_0.92fr_1fr_1fr] bg-gray-50/90 px-2 py-1 text-[8.5px] font-medium uppercase tracking-[0.05em] text-gray-400 dark:bg-white/[0.04] dark:text-gray-500">
            <span class="text-center leading-none">{{ t(store.settings.locale, 'sessions.tool') }}</span>
            <span class="flex items-center justify-center gap-1 leading-none">
              <svg class="h-2.5 w-2.5 text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" /></svg>
              <span>{{ t(store.settings.locale, 'sessions.requests') }}</span>
            </span>
            <span class="flex items-center justify-center gap-1 leading-none">
              <svg class="h-2.5 w-2.5 text-green-500" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" /></svg>
              <span>{{ t(store.settings.locale, 'sessions.input') }}</span>
            </span>
            <span class="flex items-center justify-center gap-1 leading-none">
              <svg class="h-2.5 w-2.5 text-purple-500" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l-4-4m0 0L8 8m4-4v12" /></svg>
              <span>{{ t(store.settings.locale, 'sessions.output') }}</span>
            </span>
            <span class="flex items-center justify-center gap-1 leading-none">
              <svg class="h-2.5 w-2.5 text-orange-500" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" /></svg>
              <span>{{ t(store.settings.locale, 'common.totalTokens') }}</span>
            </span>
            <span class="flex items-center justify-center gap-1 leading-none">
              <svg class="h-2.5 w-2.5 text-[var(--theme-chart-cost)]" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8c-1.657 0-3 .895-3 2s1.343 2 3 2 3 .895 3 2-1.343 2-3 2m0-8c1.11 0 2.08.402 2.599 1M12 8V7m0 1v8m0 0v1m0-1c-1.11 0-2.08-.402-2.599-1M21 12a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
              <span>{{ t(store.settings.locale, 'sessions.cost') }}</span>
            </span>
          </div>
          <div
            v-if="shouldShowProjectTotalRow(project)"
            class="grid grid-cols-[36px_0.68fr_0.92fr_0.92fr_1fr_1fr] items-center border-t border-white/60 bg-white/80 px-2 py-1 text-[9.5px] dark:border-white/6 dark:bg-white/[0.03]"
          >
            <div class="flex items-center justify-center">
              <span class="inline-flex h-4 min-w-7 items-center justify-center rounded-md border border-sky-100 bg-sky-50 px-1 text-[8px] font-semibold leading-none text-sky-600 dark:border-sky-400/20 dark:bg-sky-400/10 dark:text-sky-300">
                {{ t(store.settings.locale, 'sessions.totalRow') }}
              </span>
            </div>
            <span class="text-center font-semibold leading-none text-gray-700 dark:text-gray-200">{{ displayRequestValue(project.requestCount) }}</span>
            <span class="text-center font-semibold leading-none text-gray-700 dark:text-gray-200">{{ displayTokens(project.totalInputTokens, projectUsageVisible(project)) }}</span>
            <span class="text-center font-semibold leading-none text-gray-700 dark:text-gray-200">{{ displayTokens(project.totalOutputTokens, projectUsageVisible(project)) }}</span>
            <span class="text-center font-semibold leading-none text-gray-700 dark:text-gray-200">{{ displayTokens(projectTotalTokens(project), projectUsageVisible(project)) }}</span>
            <span class="text-center font-semibold leading-none text-[var(--theme-chart-cost)]">{{ displayCost(project.totalCost, projectUsageVisible(project)) }}</span>
          </div>
          <div
            v-for="tool in projectToolRows(project)"
            :key="`${project.name}-${tool.tool}`"
            class="grid grid-cols-[36px_0.68fr_0.92fr_0.92fr_1fr_1fr] items-center border-t border-white/80 px-2 py-1 text-[9.5px] dark:border-white/6"
            :title="!projectToolUsageVisible(tool) ? displayToolCoverageHint(tool) : undefined"
          >
            <div class="flex items-center justify-center">
              <LobeIcon
                v-if="getToolIcon(tool.tool)"
                :slug="getToolIcon(tool.tool) ?? 'claudecode'"
                :size="12"
                @error="() => {}"
              />
              <span v-else class="h-2 w-2 shrink-0 rounded-full bg-gray-400"></span>
            </div>
            <span class="text-center font-medium leading-none text-gray-700 dark:text-gray-300">{{ displayRequestValue(tool.requestCount) }}</span>
            <span class="text-center font-medium leading-none text-gray-700 dark:text-gray-300">{{ displayTokens(tool.totalInputTokens, projectToolUsageVisible(tool)) }}</span>
            <span class="text-center font-medium leading-none text-gray-700 dark:text-gray-300">{{ displayTokens(tool.totalOutputTokens, projectToolUsageVisible(tool)) }}</span>
            <span class="text-center font-medium leading-none text-gray-700 dark:text-gray-300">{{ displayTokens(tool.totalInputTokens + tool.totalOutputTokens + tool.totalCacheCreateTokens + tool.totalCacheReadTokens, projectToolUsageVisible(tool)) }}</span>
            <span class="text-center font-medium leading-none text-[var(--theme-chart-cost)]">{{ displayCost(tool.totalCost, projectToolUsageVisible(tool)) }}</span>
          </div>
        </div>
      </div>
      </template>
    </template>

    <!-- 会话详情模态框 -->
    <SessionDetailModal :visible="showModal" :session="selectedSession" @close="closeModal" />

    <Teleport to="#app">
      <div
        v-if="showRequestModal && selectedRequest"
        class="fixed inset-0 z-[80] flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm"
        style="-webkit-app-region: no-drag; app-region: no-drag"
        @click.self="closeRequestModal"
      >
        <div class="w-full max-w-md overflow-hidden rounded-2xl bg-white shadow-2xl dark:bg-[#1C1C1E]">
          <div class="flex items-start justify-between border-b border-gray-100 p-4 dark:border-neutral-800">
            <div class="min-w-0 pr-3">
              <div class="mb-1 flex items-center gap-1.5">
                <span class="request-card__status" :class="requestStatusClasses(selectedRequest)">
                  {{ requestStatusLabel(selectedRequest) }}
                </span>
                <span class="text-[10px] text-gray-400">{{ formatTime(selectedRequest.timestampSec) }}</span>
              </div>
              <h3 class="truncate text-base font-semibold text-gray-800 dark:text-gray-100">
                {{ requestModelLabel(selectedRequest) }}
              </h3>
              <p class="mt-0.5 truncate text-[10px] text-gray-400">
                {{ requestProjectLabel(selectedRequest) }} / {{ requestToolLabel(selectedRequest.tool) }} / {{ requestSourceLabel(selectedRequest) }}
              </p>
            </div>
            <button
              type="button"
              class="shrink-0 rounded-lg p-1.5 transition-colors hover:bg-gray-100 dark:hover:bg-neutral-800"
              @click="closeRequestModal"
            >
              <svg class="h-4 w-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          <div class="max-h-[calc(80vh-64px)] space-y-3 overflow-y-auto p-4">
            <div class="grid grid-cols-3 gap-2">
              <div class="request-detail-stat">
                <span>{{ t(store.settings.locale, 'common.totalTokens') }}</span>
                <strong>{{ formatTokens(selectedRequest.totalTokens) }}</strong>
              </div>
              <div class="request-detail-stat">
                <span>{{ t(store.settings.locale, 'sessions.cost') }}</span>
                <strong class="text-[var(--theme-chart-cost)]">{{ formatCost(selectedRequest.estimatedCost) }}</strong>
              </div>
              <div class="request-detail-stat">
                <span>{{ t(store.settings.locale, 'sessions.duration') }}</span>
                <strong>{{ formatDuration(selectedRequest.durationMs) }}</strong>
              </div>
            </div>

            <div class="request-detail-section">
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'sessions.input') }}</span>
                <strong>{{ formatTokens(selectedRequest.inputTokens) }}</strong>
              </div>
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'sessions.output') }}</span>
                <strong>{{ formatTokens(selectedRequest.outputTokens) }}</strong>
              </div>
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'statistics.cacheCreate') }}</span>
                <strong>{{ formatTokens(selectedRequest.cacheCreateTokens) }}</strong>
              </div>
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'statistics.cacheRead') }}</span>
                <strong>{{ formatTokens(selectedRequest.cacheReadTokens) }}</strong>
              </div>
            </div>

            <div class="request-detail-section">
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'sessions.ttft') }}</span>
                <strong>{{ formatDuration(selectedRequest.ttftMs) }}</strong>
              </div>
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'metrics.tokensPerSecond') }}</span>
                <strong>{{ selectedRequest.outputTokensPerSecond ? selectedRequest.outputTokensPerSecond.toFixed(1) : '—' }}</strong>
              </div>
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'statistics.status') }}</span>
                <strong>{{ selectedRequest.statusCode || '—' }}</strong>
              </div>
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'sessions.requestCoverage') }}</span>
                <strong>{{ requestCoverageLabel(selectedRequest.coverageOrigin) }}</strong>
              </div>
            </div>

            <div class="request-detail-section">
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'common.source') }}</span>
                <strong class="truncate text-right">{{ requestSourceLabel(selectedRequest) }}</strong>
              </div>
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'sessions.sessionId') }}</span>
                <strong class="truncate text-right">{{ selectedRequest.sessionId || '—' }}</strong>
              </div>
              <div class="request-detail-row">
                <span>{{ t(store.settings.locale, 'sessions.requestKey') }}</span>
                <strong class="truncate text-right">{{ selectedRequest.requestKey }}</strong>
              </div>
            </div>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
/* Recent / Projects switcher — theme-adaptive segmented control, accent-filled
   active segment (consistent with the main nav). Opaque track for sticky use. */
.session-tabs {
  display: flex;
  gap: 2px;
  padding: 2px;
  border-radius: 10px;
  background: var(--theme-bg-surface);
  border: 1px solid var(--theme-border-subtle);
}

.session-tabs__item {
  flex: 1;
  padding: 6px 0;
  border-radius: 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--theme-text-tertiary);
  background: transparent;
  border: none;
  cursor: pointer;
  transition: color 0.18s ease, background 0.18s ease, box-shadow 0.18s ease;
}

.session-tabs__item:hover:not(.session-tabs__item--on) {
  color: var(--theme-text-primary);
}

.session-tabs__item--on {
  color: var(--theme-accent-contrast);
  background: var(--theme-accent-primary);
  box-shadow: 0 2px 6px color-mix(in srgb, var(--theme-accent-primary) 30%, transparent);
}

/* Tool filter — compact dropdown keeps the session view stable as the tool list grows. */
.tool-filter {
  position: relative;
  padding: 2px 0 4px;
}

.tool-filter__trigger {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  border-radius: 10px;
  border: 1px solid var(--theme-border-default);
  background: var(--theme-bg-elevated);
  text-align: left;
  cursor: pointer;
  outline: none;
  transition: border-color 0.18s ease, background 0.18s ease, box-shadow 0.18s ease;
}

.tool-filter__trigger:hover,
.tool-filter__trigger--open {
  border-color: var(--theme-border-strong);
}

.tool-filter__current {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.3;
  color: var(--theme-text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tool-filter__menu {
  position: absolute;
  top: calc(100% + 5px);
  left: 0;
  right: 0;
  z-index: 40;
  max-height: 216px;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 4px;
  border-radius: 12px;
  border: 1px solid var(--theme-border-default);
  background: var(--theme-bg-overlay);
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.16);
  backdrop-filter: blur(14px);
}

.tool-filter__menu-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.tool-filter__menu-row {
  display: flex;
  align-items: stretch;
  gap: 4px;
}

.tool-filter__menu-item {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  padding: 5px 8px;
  border-radius: 8px;
  border: none;
  background: transparent;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.3;
  color: var(--theme-text-secondary);
  text-align: left;
  cursor: pointer;
  transition: background 0.14s ease, color 0.14s ease;
}

.tool-filter__menu-item:hover {
  background: var(--theme-bg-hover);
  color: var(--theme-text-primary);
}

.tool-filter__menu-item--on {
  background: var(--theme-accent-soft);
  color: var(--theme-accent-primary);
}

.tool-filter__menu-item--family {
  flex: 1;
}

.tool-filter__menu-expand {
  width: 28px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--theme-text-tertiary);
  cursor: pointer;
  transition: background 0.14s ease, color 0.14s ease;
}

.tool-filter__menu-expand:hover {
  background: var(--theme-bg-hover);
  color: var(--theme-text-primary);
}

.tool-filter__submenu {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 1px 0 3px;
  padding-left: 12px;
  position: relative;
}

.tool-filter__submenu::before {
  content: '';
  position: absolute;
  left: 4px;
  top: 2px;
  bottom: 2px;
  width: 1px;
  background: color-mix(in srgb, var(--theme-accent-primary) 18%, var(--theme-border-subtle));
}

.tool-filter__menu-item--child {
  font-size: 10px;
  padding: 5px 8px;
}

.request-stream-note {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  border: 1px solid var(--theme-border-subtle);
  border-radius: 12px;
  background:
    linear-gradient(135deg, color-mix(in srgb, var(--theme-accent-primary) 8%, transparent), transparent 58%),
    var(--theme-bg-elevated);
  padding: 7px 10px;
  font-size: 10px;
  font-weight: 600;
  color: var(--theme-text-tertiary);
}

.request-card {
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 7px;
  border: 1px solid var(--theme-border-subtle);
  border-radius: 16px;
  background: var(--theme-bg-elevated);
  padding: 10px;
  text-align: left;
  cursor: pointer;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.018);
  transition: transform 0.16s ease, border-color 0.16s ease, background 0.16s ease;
}

.request-card:hover {
  transform: translateY(-1px);
  border-color: var(--theme-border-strong);
  background: color-mix(in srgb, var(--theme-bg-elevated) 88%, var(--theme-accent-primary) 12%);
}

.request-card__top,
.request-card__meta,
.request-card__metrics {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-width: 0;
  gap: 8px;
}

.request-card__time {
  flex-shrink: 0;
  font-size: 10px;
  font-weight: 700;
  color: var(--theme-text-tertiary);
}

.request-card__model {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  font-weight: 700;
  color: var(--theme-text-primary);
}

.request-card__tokens {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  font-weight: 800;
  color: var(--theme-text-primary);
}

.request-card__cost {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  font-weight: 800;
  color: var(--theme-chart-cost);
}

.request-card__meta {
  font-size: 10px;
  font-weight: 600;
  color: var(--theme-text-tertiary);
}

.request-card__status {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  border-width: 1px;
  border-radius: 9999px;
  padding: 2px 6px;
  font-size: 9px;
  font-weight: 800;
  line-height: 1;
}

.request-card__metrics {
  border-top: 1px solid var(--theme-border-subtle);
  padding-top: 7px;
  font-size: 9.5px;
  font-weight: 700;
  color: var(--theme-text-tertiary);
}

.request-detail-stat {
  display: flex;
  flex-direction: column;
  gap: 3px;
  border-radius: 14px;
  background: var(--theme-bg-surface);
  padding: 9px 8px;
  text-align: center;
}

.request-detail-stat span,
.request-detail-row span {
  font-size: 10px;
  color: var(--theme-text-tertiary);
}

.request-detail-stat strong {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 13px;
  color: var(--theme-text-primary);
}

.request-detail-section {
  border: 1px solid var(--theme-border-subtle);
  border-radius: 14px;
  background: var(--theme-bg-elevated);
  padding: 8px 10px;
}

.request-detail-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 4px 0;
}

.request-detail-row strong {
  min-width: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  color: var(--theme-text-primary);
}
</style>
