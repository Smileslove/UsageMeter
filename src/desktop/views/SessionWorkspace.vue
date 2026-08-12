<script setup lang="ts">
/**
 * 桌面主窗口「会话工作区」全页面（设计文档 8.7）。
 * hash 深链（#/desktop/sessions/session/<opaqueKey>）已由 desktopNavigation 路由处理；
 * 本组件只负责按 sessionKey 拉取详情并展示 5 个 tab：摘要 / 活动 / 请求 / 模型与用量 / 文件。
 * 返回列表调用 desktopNavigation.backToSessions()（列表恢复筛选与滚动由 DesktopSessions 消费）。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { ArrowLeft, Copy, FileQuestionMark, Folder, Globe, MoreHorizontal, RefreshCw, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t } from '../../i18n'
import type { RequestRecord } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import LobeIcon from '../../components/LobeIcon.vue'

const props = defineProps<{
  sessionKey: string
}>()

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

const {
  formatTime,
  formatTokens,
  formatCost,
  formatDuration,
  requestModelLabel,
  requestStatusLabel,
  requestStatusClasses,
  requestCoverageLabel,
  requestSourceLabel,
  requestToolLabel,
  requestCacheTokens,
  sessionUsageVisible,
  displaySessionTitle,
  displaySessionProjectBadge,
  projectBadgeClasses,
  getToolIcon,
} = useSessionDisplay(store)

const session = computed(() => store.selectedSession)
const detailLoading = ref(false)
const detailError = ref('')

const loadDetail = async () => {
  detailLoading.value = true
  detailError.value = ''
  try {
    await store.fetchSessionDetail(props.sessionKey)
  } catch (error) {
    detailError.value = error instanceof Error ? error.message : String(error)
  } finally {
    detailLoading.value = false
  }
}

onMounted(loadDetail)
watch(() => props.sessionKey, loadDetail)

const goBack = () => nav.backToSessions()

// —— 顶部：复制 ID / 更多菜单 ——
const copiedFlash = ref('')
let copiedTimer: ReturnType<typeof setTimeout> | null = null
const copyText = async (value: string, label: string) => {
  try {
    await navigator.clipboard.writeText(value)
    copiedFlash.value = label
    if (copiedTimer) clearTimeout(copiedTimer)
    copiedTimer = setTimeout(() => { copiedFlash.value = '' }, 1400)
  } catch { /* 剪贴板不可用时静默失败 */ }
}
const moreMenuOpen = ref(false)

const fullTime = (epoch?: number) => {
  if (!epoch) return '—'
  return new Date(epoch * 1000).toLocaleString(store.settings.locale.replace('_', '-'), {
    year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit'
  })
}

// —— tabs ——
type WorkspaceTab = 'summary' | 'activity' | 'requests' | 'models' | 'files'
const activeTab = ref<WorkspaceTab>('summary')

// —— 摘要：覆盖度条（本地 / 代理 / 合并） ——
const coverageSegments = computed(() => {
  const current = session.value
  if (!current) return { local: 0, proxy: 0, merged: 0, total: 0 }
  const local = current.uncoveredRequests ?? 0
  const proxy = current.coveredRequests ?? 0
  const total = Math.max(0, current.totalRequests ?? 0)
  const merged = Math.max(0, total - local - proxy)
  return { local, proxy, merged, total: Math.max(1, local + proxy + merged) }
})
const coveragePercent = (value: number) => `${Math.min(100, Math.round((value / coverageSegments.value.total) * 100))}%`

// —— 摘要：Token 输入/输出比例条 ——
const ioRatio = computed(() => {
  const current = session.value
  if (!current) return { input: 50, output: 50 }
  const input = current.totalInputTokens || 0
  const output = current.totalOutputTokens || 0
  const total = input + output
  if (total === 0) return { input: 50, output: 50 }
  return { input: (input / total) * 100, output: (output / total) * 100 }
})

// —— 请求 tab：仅该会话关联请求（后端按最近全局记录返回，这里本地过滤并滚动补齐） ——
const wsRequestsLoading = ref(false)
const wsRequestsHasMore = ref(true)
let wsOffset = 0
const MAX_WS_REQUESTS = 500

const sessionRequests = computed(() => store.requestRecords.filter(request => request.sessionId === props.sessionKey))

const loadMoreSessionRequests = async (): Promise<void> => {
  if (wsRequestsLoading.value || !wsRequestsHasMore.value) return
  wsRequestsLoading.value = true
  try {
    const count = await store.fetchRecentRequestRecordsForTool(null, 100, wsOffset, true)
    wsOffset += count
    if (count < 100 || store.requestRecords.length >= MAX_WS_REQUESTS) wsRequestsHasMore.value = false
  } finally {
    wsRequestsLoading.value = false
  }
}

// 进入请求 tab 时若本地还没有该会话记录，则滚动补齐（最多 500 条全局记录）
watch(activeTab, async tab => {
  if (tab !== 'requests') return
  while (sessionRequests.value.length === 0 && wsRequestsHasMore.value && !wsRequestsLoading.value && store.requestRecords.length < MAX_WS_REQUESTS) {
    await loadMoreSessionRequests()
    if (store.requestRecords.length === 0) break
  }
})

// —— 模型与用量 tab：按模型聚合 ——
interface ModelAggRow {
  model: string
  requests: number
  inputTokens: number
  outputTokens: number
  cacheTokens: number
  cost: number
  rateSum: number
  rateCount: number
}
const modelAgg = computed<ModelAggRow[]>(() => {
  const map = new Map<string, ModelAggRow>()
  for (const request of sessionRequests.value) {
    let row = map.get(request.model)
    if (!row) {
      row = { model: request.model, requests: 0, inputTokens: 0, outputTokens: 0, cacheTokens: 0, cost: 0, rateSum: 0, rateCount: 0 }
      map.set(request.model, row)
    }
    row.requests += 1
    row.inputTokens += request.inputTokens || 0
    row.outputTokens += request.outputTokens || 0
    row.cacheTokens += requestCacheTokens(request)
    row.cost += request.estimatedCost || 0
    if (request.outputTokensPerSecond != null) {
      row.rateSum += request.outputTokensPerSecond
      row.rateCount += 1
    }
  }
  return [...map.values()].sort((a, b) => b.cost - a.cost)
})

// —— 文件 tab：SessionStats 无文件元数据，显示“暂不可用”占位 ——
// （M2 深度索引上线后补充 角色/路径/大小/修改时间/扫描状态 的安全元数据）

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
const shortId = (value: string) => (value.length > 12 ? `${value.slice(0, 6)}…${value.slice(-4)}` : value)

onUnmounted(() => {
  if (copiedTimer) clearTimeout(copiedTimer)
})
</script>

<template>
  <div class="flex flex-col gap-3 pb-4">
    <!-- 顶部：← 返回 + 标题/项目/工具/时间 + 复制 ID + 更多 -->
    <div class="flex flex-wrap items-center gap-2">
      <button
        type="button"
        class="theme-button-secondary inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-3 text-[12px] font-semibold"
        :aria-label="t(locale, 'desktop.workspace.backToSessions')"
        :title="t(locale, 'desktop.workspace.backToSessions')"
        @click="goBack"
      >
        <ArrowLeft class="h-4 w-4" aria-hidden="true" />
        {{ t(locale, 'desktop.workspace.backToSessions') }}
      </button>

      <div v-if="session" class="min-w-0 flex-1">
        <div class="flex min-w-0 items-center gap-2">
          <h1 class="truncate text-[15px] font-bold text-[var(--theme-text-primary)]">{{ displaySessionTitle(session) }}</h1>
          <span v-if="displaySessionProjectBadge(session)" class="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-semibold leading-none" :class="projectBadgeClasses(session.projectIdentity)">
            {{ displaySessionProjectBadge(session) }}
          </span>
          <span v-if="session.wslDistro" class="shrink-0 rounded bg-cyan-500/10 px-1.5 py-0.5 text-[10px] font-semibold leading-none text-cyan-600 dark:text-cyan-300">{{ session.wslDistro }}</span>
        </div>
        <p class="mt-0.5 flex items-center gap-1.5 text-[11px] text-[var(--theme-text-tertiary)]">
          <LobeIcon v-if="getToolIcon(session.tool)" :slug="getToolIcon(session.tool) ?? 'claudecode'" :size="12" @error="() => {}" />
          <span v-else class="h-1.5 w-1.5 rounded-full bg-[var(--theme-border-strong)]"></span>
          {{ requestToolLabel(session.tool) }} · {{ formatTime(session.lastRequestTime) }}
        </p>
      </div>
      <div v-else class="min-w-0 flex-1 truncate font-mono text-[13px] text-[var(--theme-text-tertiary)]">{{ sessionKey }}</div>

      <!-- 复制 ID -->
      <button
        type="button"
        class="theme-button-secondary inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-3 text-[12px] font-semibold"
        :aria-label="t(locale, 'desktop.workspace.copySessionId')"
        :title="t(locale, 'desktop.workspace.copySessionId')"
        @click="copyText(sessionKey, 'id')"
      >
        <Copy class="h-3.5 w-3.5" aria-hidden="true" />
        {{ t(locale, 'desktop.workspace.copySessionId') }}
      </button>

      <!-- 更多 -->
      <div class="relative shrink-0">
        <button
          type="button"
          class="theme-button-secondary inline-flex h-8 items-center gap-1 rounded-lg px-2.5 text-[12px] font-semibold"
          :aria-label="t(locale, 'desktop.workspace.moreMenu')"
          :title="t(locale, 'desktop.workspace.moreMenu')"
          :aria-expanded="moreMenuOpen"
          @click="moreMenuOpen = !moreMenuOpen"
        >
          <MoreHorizontal class="h-4 w-4" aria-hidden="true" />
        </button>
        <div v-if="moreMenuOpen" class="theme-surface-elevated absolute right-0 top-9 z-30 w-64 rounded-xl border p-1.5 shadow-lg" role="menu">
          <button
            type="button"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[11.5px] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
            role="menuitem"
            @click="copyText(sessionKey, 'id'); moreMenuOpen = false"
          >
            <Copy class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <span class="truncate">{{ t(locale, 'desktop.workspace.menuCopySessionId') }}</span>
          </button>
          <button
            type="button"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[11.5px] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
            role="menuitem"
            :disabled="!session?.cwd"
            :title="session?.cwd ?? undefined"
            @click="session?.cwd && copyText(session.cwd, 'cwd'); moreMenuOpen = false"
          >
            <Folder class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <span class="truncate">{{ t(locale, 'desktop.workspace.menuCopyCwd') }}</span>
          </button>
          <button
            type="button"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[11.5px] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
            role="menuitem"
            :disabled="!session?.topic"
            :title="session?.topic ?? undefined"
            @click="session?.topic && copyText(session.topic, 'topic'); moreMenuOpen = false"
          >
            <Copy class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <span class="truncate">{{ t(locale, 'desktop.workspace.menuCopyTopic') }}</span>
          </button>
          <button
            type="button"
            class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-[11.5px] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
            role="menuitem"
            :disabled="!session?.models || session.models.length === 0"
            @click="session?.models?.length ? copyText(session.models.join(', '), 'models') : undefined; moreMenuOpen = false"
          >
            <Copy class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <span class="truncate">{{ t(locale, 'desktop.workspace.menuCopyModels') }}</span>
          </button>
        </div>
      </div>

      <span v-if="copiedFlash" class="shrink-0 text-[11px] font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.workspace.copied') }}</span>
    </div>

    <!-- tabs -->
    <div class="flex shrink-0 gap-0.5 self-start rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] p-0.5" role="tablist" :aria-label="t(locale, 'desktop.workspace.tabsLabel')">
      <button
        v-for="tab in [['summary', 'desktop.workspace.tabSummary'], ['activity', 'desktop.workspace.tabActivity'], ['requests', 'desktop.workspace.tabRequests'], ['models', 'desktop.workspace.tabModels'], ['files', 'desktop.workspace.tabFiles']] as const"
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

    <!-- 加载 / 错误 -->
    <div v-if="detailLoading && !session" class="flex justify-center py-16">
      <div class="h-5 w-5 animate-spin rounded-full border-2 border-[var(--theme-border-strong)] border-t-[var(--theme-accent-primary)]"></div>
    </div>
    <div v-else-if="detailError" class="theme-surface rounded-xl border px-4 py-8 text-center text-[12px] text-red-500">
      {{ detailError }}
      <button type="button" class="mt-3 inline-flex items-center gap-1.5 rounded-lg border border-[var(--theme-border-default)] px-3 py-1.5 text-[11.5px] font-semibold text-[var(--theme-text-secondary)]" @click="loadDetail">
        <RefreshCw class="h-3.5 w-3.5" aria-hidden="true" />
        {{ t(locale, 'common.refresh') }}
      </button>
    </div>

    <template v-else-if="session">
      <!-- ============ 摘要 tab ============ -->
      <div v-if="activeTab === 'summary'" class="space-y-3">
        <!-- 元信息 -->
        <div class="theme-surface rounded-xl border px-4 py-3">
          <div class="grid grid-cols-1 gap-x-6 gap-y-2 sm:grid-cols-2 lg:grid-cols-3">
            <div>
              <div class="text-[10px] uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.title') }}</div>
              <div class="mt-0.5 truncate text-[12px] font-medium text-[var(--theme-text-primary)]" :title="displaySessionTitle(session)">{{ displaySessionTitle(session) }}</div>
            </div>
            <div>
              <div class="text-[10px] uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.project') }}</div>
              <div class="mt-0.5 truncate text-[12px] font-medium text-[var(--theme-text-primary)]">{{ displaySessionProjectBadge(session) || '—' }}</div>
            </div>
            <div>
              <div class="text-[10px] uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.cwd') }}</div>
              <div class="mt-0.5 truncate font-mono text-[11px] text-[var(--theme-text-secondary)]" :title="session.cwd">{{ session.cwd || '—' }}</div>
            </div>
            <div>
              <div class="text-[10px] uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.startTime') }}</div>
              <div class="mt-0.5 text-[12px] text-[var(--theme-text-secondary)]">{{ fullTime(session.firstRequestTime) }}</div>
            </div>
            <div>
              <div class="text-[10px] uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.endTime') }}</div>
              <div class="mt-0.5 text-[12px] text-[var(--theme-text-secondary)]">{{ fullTime(session.lastRequestTime) }}</div>
            </div>
            <div>
              <div class="text-[10px] uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.models') }}</div>
              <div class="mt-0.5 truncate font-mono text-[11px] text-[var(--theme-text-secondary)]" :title="session.models.join(', ')">{{ session.models.join(', ') || '—' }}</div>
            </div>
          </div>
        </div>

        <!-- 统计网格 -->
        <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
          <div class="theme-surface rounded-xl border px-3 py-2.5">
            <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.requests') }}</div>
            <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ sessionUsageVisible(session) ? (session.totalRequests ?? 0) : '—' }}</div>
          </div>
          <div class="theme-surface rounded-xl border px-3 py-2.5">
            <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.tokens') }}</div>
            <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ sessionUsageVisible(session) ? formatTokens((session.totalInputTokens || 0) + (session.totalOutputTokens || 0) + (session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)) : '—' }}</div>
          </div>
          <div class="theme-surface rounded-xl border px-3 py-2.5">
            <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.cache') }}</div>
            <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ sessionUsageVisible(session) ? formatTokens((session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)) : '—' }}</div>
          </div>
          <div class="theme-surface rounded-xl border px-3 py-2.5">
            <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.cost') }}</div>
            <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-chart-cost)]">{{ sessionUsageVisible(session) ? formatCost(session.estimatedCost) : '—' }}</div>
          </div>
          <div class="theme-surface rounded-xl border px-3 py-2.5">
            <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.rate') }}</div>
            <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ sessionUsageVisible(session) && (session.avgOutputTokensPerSecond || 0) > 0 ? `${session.avgOutputTokensPerSecond.toFixed(1)}t/s` : '—' }}</div>
          </div>
          <div class="theme-surface rounded-xl border px-3 py-2.5">
            <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.ttft') }}</div>
            <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ sessionUsageVisible(session) && session.avgTtftMs ? `${session.avgTtftMs.toFixed(0)}ms` : '—' }}</div>
          </div>
          <div class="theme-surface rounded-xl border px-3 py-2.5">
            <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.success') }}</div>
            <div class="mt-0.5 font-mono text-[15px] font-semibold text-emerald-600 dark:text-emerald-300">{{ sessionUsageVisible(session) ? (session.successRequests ?? 0) : '—' }}</div>
          </div>
          <div class="theme-surface rounded-xl border px-3 py-2.5">
            <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.errors') }}</div>
            <div class="mt-0.5 font-mono text-[15px] font-semibold text-red-500">{{ sessionUsageVisible(session) ? (session.errorRequests ?? 0) : '—' }}</div>
          </div>
        </div>

        <!-- 覆盖度条：本地 / 代理 / 合并 -->
        <div class="theme-surface rounded-xl border px-4 py-3">
          <div class="text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.coverageLabel') }}</div>
          <div class="mt-2 flex h-2.5 w-full overflow-hidden rounded-full bg-[var(--theme-border-subtle)]">
            <div class="h-full bg-amber-400" :style="{ width: coveragePercent(coverageSegments.local) }" :title="t(locale, 'desktop.workspace.coverageLocal')"></div>
            <div class="h-full bg-cyan-400" :style="{ width: coveragePercent(coverageSegments.proxy) }" :title="t(locale, 'desktop.workspace.coverageProxy')"></div>
            <div class="h-full bg-violet-400" :style="{ width: coveragePercent(coverageSegments.merged) }" :title="t(locale, 'desktop.workspace.coverageMerged')"></div>
          </div>
          <div class="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[10.5px] text-[var(--theme-text-secondary)]">
            <span class="inline-flex items-center gap-1"><span class="h-2 w-2 rounded-full bg-amber-400"></span>{{ t(locale, 'desktop.workspace.coverageLocal') }} <b class="font-mono">{{ coverageSegments.local }}</b></span>
            <span class="inline-flex items-center gap-1"><span class="h-2 w-2 rounded-full bg-cyan-400"></span>{{ t(locale, 'desktop.workspace.coverageProxy') }} <b class="font-mono">{{ coverageSegments.proxy }}</b></span>
            <span class="inline-flex items-center gap-1"><span class="h-2 w-2 rounded-full bg-violet-400"></span>{{ t(locale, 'desktop.workspace.coverageMerged') }} <b class="font-mono">{{ coverageSegments.merged }}</b></span>
          </div>
        </div>

        <!-- Token 输入/输出分布 -->
        <div class="theme-surface rounded-xl border px-4 py-3">
          <div class="text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.inputOutput') }}</div>
          <div class="mt-2 flex h-2.5 w-full overflow-hidden rounded-full bg-[var(--theme-border-subtle)]" :class="{ 'opacity-40': !sessionUsageVisible(session) }">
            <div class="h-full bg-cyan-400" :style="{ width: `${ioRatio.input}%` }"></div>
            <div class="h-full bg-fuchsia-400" :style="{ width: `${ioRatio.output}%` }"></div>
          </div>
          <div class="mt-1.5 flex justify-between text-[10.5px] text-[var(--theme-text-tertiary)]">
            <span>{{ t(locale, 'common.inputTokens') }}: {{ sessionUsageVisible(session) ? formatTokens(session.totalInputTokens || 0) : '—' }}</span>
            <span>{{ t(locale, 'common.outputTokens') }}: {{ sessionUsageVisible(session) ? formatTokens(session.totalOutputTokens || 0) : '—' }}</span>
          </div>
        </div>

        <!-- 子代理摘要：无关系数据显示“不支持”，不显示 0 -->
        <div class="theme-surface rounded-xl border px-4 py-3">
          <div class="flex items-center justify-between">
            <div class="text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.subagentSummary') }}</div>
            <span class="rounded bg-slate-500/10 px-1.5 py-0.5 text-[10px] font-semibold text-slate-500 dark:text-slate-300">{{ t(locale, 'desktop.workspace.subagentUnsupported') }}</span>
          </div>
          <p class="mt-1.5 text-[11px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.subagentUnsupportedDesc') }}</p>
        </div>
      </div>

      <!-- ============ 活动 tab（M2 占位） ============ -->
      <div v-else-if="activeTab === 'activity'" class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center">
        <Globe class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <h3 class="mt-3 text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.workspace.activityComingSoon') }}</h3>
        <p class="mt-1.5 max-w-md text-[11.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.activityComingSoonDesc') }}</p>
      </div>

      <!-- ============ 请求 tab ============ -->
      <div v-else-if="activeTab === 'requests'" class="space-y-2">
        <div class="flex items-center justify-between">
          <span class="text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.requestsCount', { count: sessionRequests.length }) }}</span>
          <button
            type="button"
            class="theme-button-secondary inline-flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-[11px] font-semibold"
            :disabled="wsRequestsLoading || !wsRequestsHasMore"
            @click="loadMoreSessionRequests"
          >
            <RefreshCw class="h-3 w-3" :class="{ 'animate-spin': wsRequestsLoading }" aria-hidden="true" />
            {{ wsRequestsLoading ? t(locale, 'desktop.workspace.loading') : t(locale, 'desktop.workspace.loadMore') }}
          </button>
        </div>
        <div v-if="sessionRequests.length === 0 && !wsRequestsLoading" class="theme-surface rounded-xl border px-4 py-10 text-center text-[11.5px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.workspace.requestsEmpty') }}
        </div>
        <!-- 请求表格（该会话关联请求） -->
        <div v-else class="theme-surface overflow-x-auto rounded-xl border">
          <table class="w-full min-w-[720px] border-collapse text-[11.5px]">
            <thead>
              <tr class="border-b border-[var(--theme-border-default)] text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnTime') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnModel') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnSource') }}</th>
                <th class="px-2.5 py-2 text-left">{{ t(locale, 'desktop.sessions.columnStatus') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnInput') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnOutput') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnCache') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnTotalTokens') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnCost') }}</th>
                <th class="px-2.5 py-2 text-right">{{ t(locale, 'desktop.sessions.columnDuration') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="request in sessionRequests"
                :key="request.requestKey"
                class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)]"
                :class="selectedRequest?.requestKey === request.requestKey ? 'bg-[var(--theme-accent-soft)]' : ''"
                @click="openRequestDrawer(request)"
              >
                <td class="whitespace-nowrap px-2.5 py-1.5 text-[var(--theme-text-secondary)]">{{ formatTime(request.timestampSec) }}</td>
                <td class="max-w-40 truncate px-2.5 py-1.5 font-mono text-[10.5px] text-[var(--theme-text-secondary)]" :title="request.model">{{ requestModelLabel(request) }}</td>
                <td class="max-w-28 truncate px-2.5 py-1.5 text-[var(--theme-text-secondary)]" :title="requestSourceLabel(request)">{{ requestSourceLabel(request) }}</td>
                <td class="px-2.5 py-1.5">
                  <span v-if="request.coverageOrigin === 'local_only'" class="text-[10px] text-[var(--theme-text-quaternary)]">—</span>
                  <span v-else class="inline-flex items-center rounded-full border px-1.5 py-px text-[9.5px] font-bold leading-none" :class="requestStatusClasses(request)">{{ requestStatusLabel(request) }}</span>
                </td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.inputTokens) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.outputTokens) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(requestCacheTokens(request)) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(request.totalTokens) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(request.estimatedCost) }}</td>
                <td class="whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ request.durationMs != null ? formatDuration(request.durationMs) : '—' }}</td>
              </tr>
            </tbody>
          </table>
        </div>

        <!-- 请求详情抽屉（复用请求视图抽屉布局） -->
        <Transition name="drawer-slide">
          <aside v-if="requestDrawerOpen && selectedRequest" class="theme-surface-elevated sticky bottom-0 z-20 mt-2 rounded-xl border p-3 shadow-lg" :aria-label="t(locale, 'desktop.sessions.drawerTitle')">
            <div class="flex items-start justify-between gap-2">
              <div class="min-w-0">
                <div class="mb-1 flex items-center gap-1.5">
                  <span v-if="selectedRequest.coverageOrigin !== 'local_only'" class="inline-flex items-center rounded-full border px-1.5 py-px text-[9.5px] font-bold leading-none" :class="requestStatusClasses(selectedRequest)">{{ requestStatusLabel(selectedRequest) }}</span>
                  <span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ formatTime(selectedRequest.timestampSec) }}</span>
                  <span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ requestModelLabel(selectedRequest) }}</span>
                </div>
                <div class="flex flex-wrap gap-x-4 gap-y-0.5 text-[10.5px] text-[var(--theme-text-secondary)]">
                  <span>{{ t(locale, 'desktop.sessions.columnTotalTokens') }}: <b class="font-mono">{{ formatTokens(selectedRequest.totalTokens) }}</b></span>
                  <span>{{ t(locale, 'sessions.cost') }}: <b class="font-mono text-[var(--theme-chart-cost)]">{{ formatCost(selectedRequest.estimatedCost) }}</b></span>
                  <span>{{ t(locale, 'sessions.ttft') }}: <b class="font-mono">{{ selectedRequest.ttftMs != null ? formatDuration(selectedRequest.ttftMs) : '—' }}</b></span>
                  <span>{{ t(locale, 'sessions.requestCoverage') }}: <b>{{ requestCoverageLabel(selectedRequest.coverageOrigin) }}</b></span>
                  <span>{{ t(locale, 'sessions.requestKey') }}: <b class="font-mono">{{ shortId(selectedRequest.requestKey) }}</b></span>
                </div>
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
          </aside>
        </Transition>
      </div>

      <!-- ============ 模型与用量 tab ============ -->
      <div v-else-if="activeTab === 'models'" class="theme-surface overflow-x-auto rounded-xl border">
        <div v-if="modelAgg.length === 0" class="px-4 py-12 text-center text-[11.5px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.workspace.modelsUnavailable') }}
        </div>
        <table v-else class="w-full min-w-[640px] border-collapse text-[11.5px]">
          <thead>
            <tr class="border-b border-[var(--theme-border-default)] text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
              <th class="px-3 py-2 text-left">{{ t(locale, 'desktop.workspace.colModel') }}</th>
              <th class="px-3 py-2 text-right">{{ t(locale, 'desktop.workspace.colRequests') }}</th>
              <th class="px-3 py-2 text-right">{{ t(locale, 'desktop.workspace.colTokens') }}</th>
              <th class="px-3 py-2 text-right">{{ t(locale, 'desktop.workspace.colCache') }}</th>
              <th class="px-3 py-2 text-right">{{ t(locale, 'desktop.workspace.colCost') }}</th>
              <th class="px-3 py-2 text-right">{{ t(locale, 'desktop.workspace.colRate') }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="row in modelAgg" :key="row.model" class="border-b border-[var(--theme-border-subtle)] last:border-0">
              <td class="max-w-64 truncate px-3 py-2 font-mono text-[10.5px] text-[var(--theme-text-secondary)]" :title="row.model">{{ row.model }}</td>
              <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-text-primary)]">{{ row.requests }}</td>
              <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(row.inputTokens + row.outputTokens) }}</td>
              <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(row.cacheTokens) }}</td>
              <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(row.cost) }}</td>
              <td class="whitespace-nowrap px-3 py-2 text-right font-mono text-[var(--theme-text-secondary)]">{{ row.rateCount > 0 ? `${(row.rateSum / row.rateCount).toFixed(1)}t/s` : '—' }}</td>
            </tr>
          </tbody>
        </table>
      </div>

      <!-- ============ 文件 tab（暂不可用占位） ============ -->
      <div v-else class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center">
        <FileQuestionMark class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <h3 class="mt-3 text-[13px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.workspace.filesUnavailable') }}</h3>
        <p class="mt-1.5 max-w-md text-[11.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.filesUnavailableDesc') }}</p>
      </div>
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
  transform: translateY(8px);
  opacity: 0;
}
</style>
