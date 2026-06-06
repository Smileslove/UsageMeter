<script setup lang="ts">
import { LayoutGrid } from 'lucide-vue-next'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import type { ProjectStats, ProjectToolStats, SessionStats } from '../types'
import SessionDetailModal from '../components/SessionDetailModal.vue'
import LobeIcon from '../components/LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'
import { formatCost as formatCostUtil, formatTokenValue, formatRequestCount } from '../utils/format'

const store = useMonitorStore()
const SESSION_SOURCE_TOOLS = new Set(['claude_code', 'codex', 'opencode', 'reasonix'])
const normalizeSessionTool = (tool: string | null | undefined) => (
  tool && SESSION_SOURCE_TOOLS.has(tool) ? tool : null
)
const uuidLikePattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

// 视图切换状态
const activeTab = ref<'recent' | 'projects'>('recent')
const selectedTool = ref<string | null>(normalizeSessionTool(store.settings.clientTools.activeToolFilter))
const lastGlobalTool = ref<string | null>(normalizeSessionTool(store.settings.clientTools.activeToolFilter))

// 选中的会话（用于模态框）
const selectedSession = ref<SessionStats | null>(null)
const showModal = ref(false)

interface SessionCacheEntry {
  items: SessionStats[]
  currentPage: number
  hasMore: boolean
}

const sessionCache = new Map<string, SessionCacheEntry>()
const projectCache = new Map<string, ProjectStats[]>()
const lastProxyRecordCount = ref<number | null>(null)
const proxyRefreshDebounceMs = 1500
const copiedProjectPath = ref<string | null>(null)
let proxyRefreshTimer: ReturnType<typeof setTimeout> | null = null
let copiedProjectPathTimer: ReturnType<typeof setTimeout> | null = null

const cacheKey = () => `${selectedTool.value ?? '__all__'}`

const sourceOptions = computed(() => {
  const profiles = store.settings.clientTools.profiles
    .filter(profile => SESSION_SOURCE_TOOLS.has(profile.tool))
    .sort((a, b) => {
      if (a.tool === 'claude_code') return -1
      if (b.tool === 'claude_code') return 1
      return (a.displayName || a.tool).localeCompare(b.displayName || b.tool)
    })

  return [
    {
      key: '__all__',
      tool: null,
      label: t(store.settings.locale, 'tools.all'),
      icon: null
    },
    ...profiles.map(profile => ({
      key: profile.tool,
      tool: profile.tool,
      label: profile.displayName || profile.tool,
      icon: profile.icon || TOOL_LOBE_ICONS[profile.tool] || null
    }))
  ]
})

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
  projectCache.clear()
}

const projectCacheKey = () => `projects:${selectedTool.value ?? '__all__'}`

const triggerSessionViewRefresh = async () => {
  clearViewCaches()
  await reloadSessions(true)
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
  if (newTab === 'projects') {
    await reloadProjectStats()
  }
})

// 分页状态
const currentPage = ref(0)
const pageSize = 30
const hasMore = ref(true)
const loadingMore = ref(false)

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

const getToolProfile = (tool: string) => (
  store.settings.clientTools.profiles.find(profile => profile.tool === tool)
)

const getToolIcon = (tool: string) => {
  const profile = getToolProfile(tool)
  return profile?.icon || TOOL_LOBE_ICONS[tool] || null
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

// 监听工具筛选变化
watch([selectedTool], async () => {
  await reloadSessions()
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
let observer: IntersectionObserver | null = null

// 初始加载
onMounted(async () => {
  await reloadSessions()

  // 监听触底加载
  setTimeout(() => {
    observer = new IntersectionObserver(
      entries => {
        if (entries[0].isIntersecting && hasMore.value && !loadingMore.value) {
          loadMore()
        }
      },
      { root: null, rootMargin: '100px' }
    )
    if (loadMoreTrigger.value) {
      observer.observe(loadMoreTrigger.value)
    }
  }, 100)
})

onUnmounted(() => {
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
        :class="{ 'session-tabs__item--on': activeTab === 'projects' }"
        @click="activeTab = 'projects'"
      >
        {{ t(store.settings.locale, 'sessions.tabs.projects') }}
      </button>
    </div>

    <div v-if="activeTab === 'recent'" class="tool-filter">
      <button
        v-for="option in sourceOptions"
        :key="option.key"
        type="button"
        class="tool-filter__item"
        :class="{ 'tool-filter__item--on': selectedTool === option.tool }"
        @click="selectedTool = option.tool"
      >
        <LayoutGrid
          v-if="!option.icon"
          class="h-3.5 w-3.5 shrink-0"
        />
        <LobeIcon
          v-else
          :slug="option.icon"
          :size="14"
          @error="() => {}"
        />
        <span class="min-w-0 truncate">{{ option.label }}</span>
      </button>
    </div>



    <!-- 加载状态 -->
    <div v-if="store.sessionsLoading" class="flex justify-center py-8">
      <div class="animate-spin w-5 h-5 border-2 border-blue-500 border-t-transparent rounded-full"></div>
    </div>

    <!-- 空状态 -->
    <div v-else-if="store.sessions.length === 0" class="text-center py-8 text-gray-400 text-sm">
      {{ t(store.settings.locale, 'sessions.noData') }}
    </div>

    <!-- 1. 会话列表视图 -->
    <template v-else-if="activeTab === 'recent'">
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

    <!-- 2. 项目维度的聚合视图 -->
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

/* Tool filter — theme-adaptive pills that wrap instead of scrolling,
   so it scales as more tools are added. Mirrors the app's segmented controls. */
.tool-filter {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 2px 2px 4px;
}

.tool-filter__item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 100%;
  padding: 4px 10px;
  border-radius: 9999px;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.1;
  cursor: pointer;
  border: 1px solid var(--theme-border-default);
  background: var(--theme-bg-elevated);
  color: var(--theme-text-tertiary);
  transition: color 0.18s ease, background 0.18s ease, border-color 0.18s ease, box-shadow 0.18s ease;
}

.tool-filter__item:hover:not(.tool-filter__item--on) {
  color: var(--theme-text-primary);
  border-color: var(--theme-border-strong);
}

.tool-filter__item--on {
  color: var(--theme-accent-contrast);
  background: var(--theme-accent-primary);
  border-color: transparent;
  box-shadow: 0 2px 6px color-mix(in srgb, var(--theme-accent-primary) 30%, transparent);
}

:root[data-appearance='dark'] .tool-filter__item {
  background: var(--theme-dark-item-bg);
  border-color: var(--theme-divider-default);
  color: var(--theme-dark-idle-label);
}

:root[data-appearance='dark'] .tool-filter__item:hover:not(.tool-filter__item--on) {
  color: var(--theme-text-primary);
}

:root[data-appearance='dark'] .tool-filter__item--on {
  background: var(--theme-accent-primary);
  color: var(--theme-accent-contrast);
  border-color: transparent;
  box-shadow: 0 2px 8px color-mix(in srgb, var(--theme-accent-primary) 35%, transparent);
}
</style>
