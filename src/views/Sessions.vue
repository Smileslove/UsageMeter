<script setup lang="ts">
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { onMounted, onUnmounted, ref, watch } from 'vue'
import type { RequestRecord, SessionStats } from '../types'
import SessionDetailModal from '../components/SessionDetailModal.vue'
import RequestDetailModal from '../components/RequestDetailModal.vue'
import SessionSourceFilter from '../components/SessionSourceFilter.vue'
import SegmentedControl from '../components/SegmentedControl.vue'
import LobeIcon from '../components/LobeIcon.vue'
import { useSessionDisplay } from '../composables/useSessionDisplay'
import { SESSION_SOURCE_TOOLS, useSessionViewData } from '../composables/useSessionViewData'

const store = useMonitorStore()
const {
  formatTime,
  formatTokens,
  displayRequestCost,
  displayRequestTokens,
  displaySessionCost,
  formatDuration,
  requestModelLabel,
  sessionModelLabel,
  requestStatusLabel,
  requestStatusClasses,
  requestCoverageLabel,
  requestProjectLabel,
  requestSourceLabel,
  requestToolLabel,
  requestCacheTokens,
  requestHasProxyPerformance,
  localRequests,
  coveredRequests,
  sessionUsageVisible,
  projectUsageVisible,
  projectToolUsageVisible,
  sessionHasPartialCoverage,
  displayTokens,
  displayCost,
  displayRequestValue,
  displaySessionCacheHitRate,
  displaySessionPrimaryLabel,
  displaySessionPrimaryValue,
  displayProjectCoverageHint,
  displayToolCoverageHint,
  displaySessionTitle,
  displaySessionProjectBadge,
  projectBadgeClasses,
  displayProjectName,
  displayProjectHint,
  displayProjectWslBadge,
  displayProjectWslTitle,
  displayProxyTokenValue,
  displayProxyRateValue,
  getToolIcon,
  projectToolRows,
  projectTotalTokens,
  shouldShowProjectTotalRow,
} = useSessionDisplay(store)
// 视图切换状态
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

const selectedSession = ref<SessionStats | null>(null)
const showModal = ref(false)
const selectedRequest = ref<RequestRecord | null>(null)
const showRequestModal = ref(false)

const copiedProjectPath = ref<string | null>(null)
let copiedProjectPathTimer: ReturnType<typeof setTimeout> | null = null
let loadTriggerObserveTimer: ReturnType<typeof setTimeout> | null = null

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

const scheduleObserveLoadTriggers = () => {
  if (loadTriggerObserveTimer) {
    clearTimeout(loadTriggerObserveTimer)
  }
  loadTriggerObserveTimer = setTimeout(() => {
    loadTriggerObserveTimer = null
    observeLoadTriggers()
  }, 50)
}

watch(activeTab, () => {
  scheduleObserveLoadTriggers()
})

watch(() => store.requestRecords.length, () => {
  scheduleObserveLoadTriggers()
})

watch(() => store.sessions.length, () => {
  scheduleObserveLoadTriggers()
})

onMounted(async () => {
  await initializeSessionView()

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
  disposeSessionView()
  if (observer) {
    observer.disconnect()
  }
  if (copiedProjectPathTimer) {
    clearTimeout(copiedProjectPathTimer)
    copiedProjectPathTimer = null
  }
  if (loadTriggerObserveTimer) {
    clearTimeout(loadTriggerObserveTimer)
    loadTriggerObserveTimer = null
  }
})
</script>

<template>
  <div class="space-y-2 animate-in fade-in zoom-in-95 duration-300 pb-4 min-h-full">
    <!-- 顶部视图切换 Tabs -->
    <SegmentedControl
      :active-index="Math.max(0, ['recent', 'requests', 'projects'].indexOf(activeTab))"
      :aria-label="t(store.settings.locale, 'sessions.title')"
      class="session-tabs sticky top-0 z-10 mb-2 backdrop-blur-md"
    >
      <button
        type="button"
        :aria-pressed="activeTab === 'recent'"
        class="session-tabs__item"
        :class="{ 'session-tabs__item--on': activeTab === 'recent' }"
        @click="activeTab = 'recent'"
      >
        {{ t(store.settings.locale, 'sessions.tabs.recent') }}
      </button>
      <button
        type="button"
        :aria-pressed="activeTab === 'requests'"
        class="session-tabs__item"
        :class="{ 'session-tabs__item--on': activeTab === 'requests' }"
        @click="activeTab = 'requests'"
      >
        {{ t(store.settings.locale, 'sessions.tabs.requests') }}
      </button>
      <button
        type="button"
        :aria-pressed="activeTab === 'projects'"
        class="session-tabs__item"
        :class="{ 'session-tabs__item--on': activeTab === 'projects' }"
        @click="activeTab = 'projects'"
      >
        {{ t(store.settings.locale, 'sessions.tabs.projects') }}
      </button>
    </SegmentedControl>

    <SessionSourceFilter
      v-if="activeTab === 'recent' || activeTab === 'requests'"
      v-model="selectedTool"
      :source-tools="SESSION_SOURCE_TOOLS"
    />
    <!-- 1. 会话列表视图 -->
    <template v-if="activeTab === 'recent'">
      <div v-if="store.sessionsLoading && store.sessions.length === 0" class="flex justify-center py-8">
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
              <span class="truncate">{{ sessionModelLabel(session) }}</span>
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
            <span class="font-semibold text-[var(--theme-chart-cost)]">{{ displaySessionCost(session) }}</span>
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
      <div class="flex items-center justify-between gap-2 rounded-xl border border-gray-100 bg-white px-2.5 py-1.5 text-[10px] text-gray-400 shadow-[0_2px_10px_rgba(0,0,0,0.02)] dark:border-white/5 dark:bg-[#1E2024] dark:text-gray-500">
        <span class="font-medium">{{ t(store.settings.locale, 'sessions.requestsRecentHint') }}</span>
        <span class="rounded-md bg-gray-50 px-1.5 py-0.5 font-mono font-semibold text-gray-600 dark:bg-white/[0.04] dark:text-gray-300">
          {{ store.requestRecords.length }}/200
        </span>
      </div>

      <div v-if="store.requestRecordsLoading && store.requestRecords.length === 0" class="flex justify-center py-8">
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
          class="w-full cursor-pointer rounded-xl border border-gray-100 bg-white px-2.5 py-2 text-left shadow-[0_2px_10px_rgba(0,0,0,0.02)] transition-colors hover:bg-gray-50 dark:border-white/5 dark:bg-[#1E2024] dark:hover:bg-white/5"
          @click="openRequestDetail(request)"
        >
          <div class="flex min-w-0 items-center justify-between gap-2">
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="max-w-[128px] shrink-0 truncate rounded px-1.5 py-px text-[10px] font-semibold text-indigo-600 border border-indigo-100 bg-indigo-50 dark:border-indigo-500/30 dark:bg-indigo-500/20 dark:text-indigo-300">
                {{ requestProjectLabel(request) }}
              </span>
              <div class="flex min-w-0 items-center gap-1 text-[10px] text-gray-400 dark:text-gray-500">
                <LobeIcon
                  v-if="getToolIcon(request.tool)"
                  :slug="getToolIcon(request.tool) ?? 'claudecode'"
                  :size="12"
                  @error="() => {}"
                />
                <span v-else class="h-1.5 w-1.5 shrink-0 rounded-full bg-gray-400"></span>
                <span class="truncate">{{ requestToolLabel(request.tool) }}</span>
              </div>
            </div>
            <div class="flex shrink-0 items-center gap-1 text-[10px] text-gray-400 dark:text-gray-500">
              <svg class="h-2.5 w-2.5 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
              <span>{{ formatTime(request.timestampSec) }}</span>
            </div>
          </div>

          <div class="mt-1.5 flex items-start justify-between gap-2">
            <div class="min-w-0 flex-1">
              <p class="truncate text-[12px] font-medium leading-snug text-gray-800 dark:text-gray-200" :title="requestModelLabel(request)">
                {{ requestModelLabel(request) }}
              </p>
              <div class="mt-0.5 flex min-w-0 items-center gap-1 text-[10px] text-gray-400 dark:text-gray-500">
                <svg class="h-2.5 w-2.5 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                </svg>
                <span class="truncate">{{ requestSourceLabel(request) }}</span>
              </div>
            </div>
            <div class="flex shrink-0 flex-col items-end gap-1">
              <span class="request-card__status" :class="requestStatusClasses(request)">
                {{ requestStatusLabel(request) }}
              </span>
              <div class="flex items-center gap-2">
                <span class="font-mono text-[10px] font-semibold text-gray-700 dark:text-gray-200">
                  {{ displayRequestTokens(request, request.totalTokens) }}
                </span>
                <span class="font-mono text-[10px] font-semibold text-[var(--theme-chart-cost)]">
                  {{ displayRequestCost(request) }}
                </span>
              </div>
            </div>
          </div>

          <div class="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1.5 border-t border-gray-100 pt-1.5 text-[10px] dark:border-white/5">
            <div class="flex items-center gap-0.5">
              <svg class="h-[10px] w-[10px] shrink-0 text-emerald-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
              </svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.input') }}</span>
              <span class="font-mono font-semibold text-gray-700 dark:text-gray-300">{{ displayRequestTokens(request, request.inputTokens) }}</span>
            </div>
            <div class="flex items-center gap-0.5">
              <svg class="h-[10px] w-[10px] shrink-0 text-sky-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l-4-4m0 0L8 8m4-4v12" />
              </svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.output') }}</span>
              <span class="font-mono font-semibold text-gray-700 dark:text-gray-300">{{ displayRequestTokens(request, request.outputTokens) }}</span>
            </div>
            <div class="flex items-center gap-0.5">
              <svg class="h-[10px] w-[10px] shrink-0 text-violet-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z" />
              </svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'statistics.cache') }}</span>
              <span class="font-mono font-semibold text-gray-700 dark:text-gray-300">{{ formatTokens(requestCacheTokens(request)) }}</span>
            </div>
            <div class="ml-auto flex items-center gap-0.5">
              <svg class="h-[10px] w-[10px] shrink-0 text-amber-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z" />
              </svg>
              <span class="text-gray-400">{{ requestHasProxyPerformance(request) ? t(store.settings.locale, 'sessions.duration') : t(store.settings.locale, 'sessions.requestCoverage') }}</span>
              <span class="font-mono font-semibold text-gray-700 dark:text-gray-300">
                {{ requestHasProxyPerformance(request) ? formatDuration(request.durationMs) : requestCoverageLabel(request.coverageOrigin) }}
              </span>
            </div>
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
      <div v-if="store.projectStatsLoading && store.projectStats.length === 0" class="flex justify-center py-8">
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

    <RequestDetailModal :visible="showRequestModal" :request="selectedRequest" @close="closeRequestModal" />
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
  background: transparent;
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

</style>
