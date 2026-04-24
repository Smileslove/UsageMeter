<script setup lang="ts">
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { onMounted, onUnmounted, ref, watch } from 'vue'
import type { SessionStats } from '../types'
import SessionDetailModal from '../components/SessionDetailModal.vue'

const store = useMonitorStore()

// 视图切换状态
const activeTab = ref<'recent' | 'projects'>('recent')

// 选中的会话（用于模态框）
const selectedSession = ref<SessionStats | null>(null)
const showModal = ref(false)

// 切换 tab 时加载项目统计
watch(activeTab, async (newTab) => {
  if (newTab === 'projects' && store.projectStats.length === 0) {
    await store.fetchProjectStats()
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
  if (diffMins < 1) return '刚刚'
  if (diffMins < 60) return `${diffMins}分钟前`
  if (diffHours < 24) return `${diffHours}小时前`

  // 超过24小时就直接显示具体的月日+时间(如果是当年)，否则带上年份
  return date.toLocaleString(locale, {
    year: isSameYear ? undefined : 'numeric',
    month: 'numeric',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit'
  })
}

// 格式化 Token 数量（1000以下显示整数，以上保留2位小数）
const formatTokens = (tokens: number) => {
  if (!tokens) return '0'
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(2)}M`
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(2)}K`
  return Math.round(tokens).toString()
}

// 格式化费用（统一4位小数）
const formatCost = (cost: number | undefined) => {
  if (cost === undefined || cost === null) return '-'
  if (cost > 0) return `$${cost.toFixed(4)}`
  return '$0.0000'
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

  const count = await store.fetchSessions(pageSize, currentPage.value * pageSize, true)
  if (count < pageSize) {
    hasMore.value = false
  }
  loadingMore.value = false
}

// 监听数据源变化，重新加载会话列表
watch(() => store.settings.dataSource, async () => {
  currentPage.value = 0
  hasMore.value = true
  const count = await store.fetchSessions(pageSize, 0, false)
  if (count < pageSize) {
    hasMore.value = false
  }
})

// 触底加载触发元素
const loadMoreTrigger = ref<HTMLElement | null>(null)
let observer: IntersectionObserver | null = null

// 初始加载
onMounted(async () => {
  currentPage.value = 0
  hasMore.value = true
  const count = await store.fetchSessions(pageSize, 0, false)
  if (count < pageSize) {
    hasMore.value = false
  }

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

    <!-- 数据来源提示 -->
    <div v-if="!store.isProxyMode" class="text-[10px] text-gray-400 px-1">
      {{ t(store.settings.locale, 'sessions.jsonlSource') }}
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
            {{ session.topic || session.sessionName || session.sessionId.split('::').pop() }}
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
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ formatTokens((session.totalInputTokens || 0) + (session.totalOutputTokens || 0)) }}</span>
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
            <span class="text-[#00E5FF] font-medium">${{ session.estimatedCost === undefined ? '0.0000' : session.estimatedCost.toFixed(4) }}</span>
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
        <div v-for="project in store.projectStats" :key="project.name" class="bg-white dark:bg-[#1E2024] rounded-xl border border-gray-100 dark:border-white/5 py-2.5 px-2.5 hover:bg-gray-50 dark:hover:bg-white/5 transition-colors flex flex-col gap-1.5">
        <!-- 顶部：项目名称与最后活跃时间 -->
        <div class="flex items-center justify-between w-full px-0.5">
          <div class="flex items-center gap-2">
            <span class="text-[11px] font-bold px-2 py-0.5 rounded-md bg-indigo-50 text-indigo-600 dark:bg-indigo-500/20 dark:text-indigo-300 border border-indigo-100 dark:border-indigo-500/30">
              {{ project.name }}
            </span>
          </div>
          <div class="flex items-center gap-1 shrink-0 text-[11px] text-gray-400">
            <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
            <span>{{ t(store.settings.locale, 'sessions.lastActive') }}: {{ formatTime(project.lastActive).split(' ')[0] }}</span>
          </div>
        </div>

        <!-- 数据明细网格 -->
        <div class="grid grid-cols-5 w-full items-start text-[10px] pt-1.5 border-t border-gray-50 dark:border-white/5 gap-y-0.5">
          <!-- 会话总数 -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-blue-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z" />
              </svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.requests') }}</span>
            </div>
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ project.sessionCount }}</span>
          </div>

          <!-- 总输入 -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-green-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.input') }}</span>
            </div>
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ formatTokens(project.totalInputTokens) }}</span>
          </div>

          <!-- 总输出 -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-purple-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l-4-4m0 0L8 8m4-4v12" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.output') }}</span>
            </div>
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ formatTokens(project.totalOutputTokens) }}</span>
          </div>

          <!-- 总Token -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-orange-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" /></svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'common.totalTokens') }}</span>
            </div>
            <span class="text-gray-700 dark:text-gray-300 font-medium">{{ formatTokens(project.totalInputTokens + project.totalOutputTokens) }}</span>
          </div>

          <!-- 总成本 -->
          <div class="flex flex-col gap-0.5 min-w-0 items-center">
            <div class="flex items-center gap-0.5">
              <svg class="w-[11px] h-[11px] text-[#00E5FF] shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8c-1.657 0-3 .895-3 2s1.343 2 3 2 3 .895 3 2-1.343 2-3 2m0-8c1.11 0 2.08.402 2.599 1M12 8V7m0 1v8m0 0v1m0-1c-1.11 0-2.08-.402-2.599-1M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
              </svg>
              <span class="text-gray-400">{{ t(store.settings.locale, 'sessions.cost') }}</span>
            </div>
            <span class="text-[#00E5FF] font-medium">{{ formatCost(project.totalCost) }}</span>
          </div>
        </div>
      </div>
      </template>
    </template>

    <!-- 会话详情模态框 -->
    <SessionDetailModal :visible="showModal" :session="selectedSession" @close="closeModal" />
  </div>
</template>
