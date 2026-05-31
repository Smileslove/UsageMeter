<script setup lang="ts">
import { LayoutGrid } from 'lucide-vue-next'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import type { ProjectStats, SessionStats } from '../types'
import SessionDetailModal from '../components/SessionDetailModal.vue'
import LobeIcon from '../components/LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'
import { formatCost as formatCostUtil, formatTokenValue, formatRequestCount } from '../utils/format'

const store = useMonitorStore()
const SESSION_SOURCE_TOOLS = new Set(['claude_code', 'codex'])
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

const projectCacheKey = () => `projects:all-tools`

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

  await store.fetchProjectStatsForTool(null)
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

const displaySessionTitle = (session: SessionStats) => {
  const sessionName = session.sessionName?.trim()
  if (session.topic?.trim()) return session.topic
  if (sessionName && !uuidLikePattern.test(sessionName)) return sessionName
  if (session.lastPrompt?.trim()) return session.lastPrompt
  if (session.projectName?.trim()) return session.projectName
  return t(store.settings.locale, 'sessions.untitled')
}

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
    <div class="flex p-0.5 bg-gray-100/80 dark:bg-[#1E2024]/80 rounded-lg backdrop-blur-md sticky top-0 z-10 mb-2">
      <button @click="activeTab = 'recent'" :class="['flex-1 py-1.5 text-[12px] font-medium rounded-md transition-all', activeTab === 'recent' ? 'bg-white dark:bg-[#2A2D32] shadow-sm text-gray-800 dark:text-gray-100' : 'text-gray-500 hover:text-gray-700 dark:hover:text-gray-300']">
        {{ t(store.settings.locale, 'sessions.tabs.recent') }}
      </button>
      <button @click="activeTab = 'projects'" :class="['flex-1 py-1.5 text-[12px] font-medium rounded-md transition-all', activeTab === 'projects' ? 'bg-white dark:bg-[#2A2D32] shadow-sm text-gray-800 dark:text-gray-100' : 'text-gray-500 hover:text-gray-700 dark:hover:text-gray-300']">
        {{ t(store.settings.locale, 'sessions.tabs.projects') }}
      </button>
    </div>

    <div v-if="activeTab === 'recent'" class="flex items-center gap-1.5 overflow-x-auto px-0.5 pb-0.5">
      <button
        v-for="option in sourceOptions"
        :key="option.key"
        @click="selectedTool = option.tool"
        :class="[
          'inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] font-medium transition-colors',
          selectedTool === option.tool
            ? 'border-sky-200 bg-sky-50 text-sky-700 dark:border-sky-500/30 dark:bg-sky-500/15 dark:text-sky-300'
            : 'border-gray-200 bg-white text-gray-500 dark:border-white/8 dark:bg-[#1E2024] dark:text-gray-400'
        ]"
      >
        <LayoutGrid
          v-if="!option.icon"
          class="h-3.5 w-3.5"
        />
        <LobeIcon
          v-else
          :slug="option.icon"
          :size="14"
          @error="() => {}"
        />
        <span>{{ option.label }}</span>
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
      <div v-for="session in store.sessions" :key="session.sessionId" class="bg-white dark:bg-[#1E2024] rounded-xl border border-gray-100 dark:border-white/5 p-3 hover:bg-gray-50 dark:hover:bg-white/5 transition-colors cursor-pointer flex flex-col gap-2.5" @click="openSessionDetail(session)">
        <!-- 1. 顶部信息行 (项目名称 + 模型名称 + 时间 + 话题) -->
        <div class="flex flex-col gap-1 w-full pb-0.5">
          <!-- 首行: 项目 + 模型 + 时间 -->
          <div class="flex items-center justify-between w-full h-5">
            <!-- 左侧项目与模型 -->
            <div class="flex items-center gap-2">
              <!-- 项目名 -->
              <span v-if="session.projectName" class="text-[11px] font-bold px-2 py-0.5 rounded-md bg-indigo-50 text-indigo-600 dark:bg-indigo-500/20 dark:text-indigo-300 border border-indigo-100 dark:border-indigo-500/30">
                {{ session.projectName }}
              </span>

              <!-- 模型名称 -->
              <div class="flex items-center gap-1 font-medium text-[11px] text-gray-500 dark:text-gray-400">
                <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    stroke-width="2"
                    d="M19.428 15.428a2 2 0 00-1.022-.547l-2.387-.477a6 6 0 00-3.86.517l-.318.158a6 6 0 01-3.86.517L6.05 15.21a2 2 0 00-1.806.547M8 4h8l-1 1v5.172a2 2 0 00.586 1.414l5 5c1.26 1.26.367 3.414-1.415 3.414H4.828c-1.782 0-2.674-2.154-1.414-3.414l5-5A2 2 0 009 10.172V5L8 4z"
                  />
                </svg>
                <span>{{ session.models[0] || 'Unknown' }}</span>
              </div>
            </div>

            <!-- 右侧时间 (水平靠右，垂直居中) -->
            <div class="flex items-center gap-1 shrink-0 text-[11px] text-gray-400 dark:text-gray-500">
              <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" /></svg>
              <span>{{ formatTime(session.lastRequestTime) }}</span>
            </div>
          </div>

          <!-- 话题内容 -->
          <span class="text-sm text-gray-800 dark:text-gray-200 line-clamp-2 min-w-0 font-medium leading-snug">
            {{ displaySessionTitle(session) }}
          </span>
        </div>

        <!-- 2. 底部数据行 (输入、输出、总Token、生成速率、金额) -->
        <div class="grid grid-cols-5 w-full items-start text-[10px] pt-1.5 border-t border-gray-100 dark:border-white/5 gap-y-0.5">
          <!-- 输入 Token -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-green-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.input') }}</span>
            </div>
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ formatTokens(session.totalInputTokens) }}</span>
          </div>

          <!-- 输出 Token -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-purple-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l-4-4m0 0L8 8m4-4v12" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.output') }}</span>
            </div>
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ formatTokens(session.totalOutputTokens) }}</span>
          </div>

          <!-- 总 Token -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-orange-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'common.totalTokens') }}</span>
            </div>
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ formatTokens((session.totalInputTokens || 0) + (session.totalOutputTokens || 0) + (session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)) }}</span>
          </div>

          <!-- 平均 Token 速率 -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-yellow-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z" />
              </svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.avgRate') }}</span>
            </div>
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ session.avgOutputTokensPerSecond > 0 ? `${session.avgOutputTokensPerSecond.toFixed(2)} t/s` : '-' }}</span>
          </div>

          <!-- 估算费用 -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-[#00E5FF] shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8c-1.657 0-3 .895-3 2s1.343 2 3 2 3 .895 3 2-1.343 2-3 2m0-8c1.11 0 2.08.402 2.599 1M12 8V7m0 1v8m0 0v1m0-1c-1.11 0-2.08-.402-2.599-1M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.cost') }}</span>
            </div>
            <span class="font-medium text-[var(--theme-chart-cost)]">{{ formatCost(session.estimatedCost) }}</span>
          </div>
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
        <div v-for="project in store.projectStats" :key="project.projectPath || project.name" class="bg-white dark:bg-[#1E2024] rounded-xl border border-gray-100 dark:border-white/5 py-2 px-2 hover:bg-gray-50 dark:hover:bg-white/5 transition-colors flex flex-col gap-1">
        <!-- 顶部：项目名称与最后活跃时间 -->
        <div class="flex items-center justify-between gap-2 w-full px-0.5">
          <div class="flex min-w-0 flex-1 items-center gap-1.5">
            <span class="shrink-0 text-[11px] font-bold px-2 py-0.5 rounded-md bg-indigo-50 text-indigo-600 dark:bg-indigo-500/20 dark:text-indigo-300 border border-indigo-100 dark:border-indigo-500/30 leading-none">
              {{ project.name }}
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
            <span class="text-center font-semibold leading-none text-gray-700 dark:text-gray-200">{{ formatRequestCount(project.requestCount) }}</span>
            <span class="text-center font-semibold leading-none text-gray-700 dark:text-gray-200">{{ formatTokens(project.totalInputTokens) }}</span>
            <span class="text-center font-semibold leading-none text-gray-700 dark:text-gray-200">{{ formatTokens(project.totalOutputTokens) }}</span>
            <span class="text-center font-semibold leading-none text-gray-700 dark:text-gray-200">{{ formatTokens(projectTotalTokens(project)) }}</span>
            <span class="text-center font-semibold leading-none text-[var(--theme-chart-cost)]">{{ formatCost(project.totalCost) }}</span>
          </div>
          <div
            v-for="tool in projectToolRows(project)"
            :key="`${project.name}-${tool.tool}`"
            class="grid grid-cols-[36px_0.68fr_0.92fr_0.92fr_1fr_1fr] items-center border-t border-white/80 px-2 py-1 text-[9.5px] dark:border-white/6"
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
            <span class="text-center font-medium leading-none text-gray-700 dark:text-gray-300">{{ formatRequestCount(tool.requestCount) }}</span>
            <span class="text-center font-medium leading-none text-gray-700 dark:text-gray-300">{{ formatTokens(tool.totalInputTokens) }}</span>
            <span class="text-center font-medium leading-none text-gray-700 dark:text-gray-300">{{ formatTokens(tool.totalOutputTokens) }}</span>
            <span class="text-center font-medium leading-none text-gray-700 dark:text-gray-300">{{ formatTokens(tool.totalInputTokens + tool.totalOutputTokens + tool.totalCacheCreateTokens + tool.totalCacheReadTokens) }}</span>
            <span class="text-center font-medium leading-none text-[var(--theme-chart-cost)]">{{ formatCost(tool.totalCost) }}</span>
          </div>
        </div>
      </div>
      </template>
    </template>

    <!-- 会话详情模态框 -->
    <SessionDetailModal :visible="showModal" :session="selectedSession" @close="closeModal" />
  </div>
</template>
