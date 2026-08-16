<script setup lang="ts">
/**
 * 桌面主窗口「请求」页（设计文档第 8 章：请求表格 + 右侧抽屉）。
 * 数据加载复用 useSessionViewData（fetchRecentRequestRecordsForTool），activeTab 固定为 'requests'。
 * 请求筛选：状态 / 覆盖来源 / 性能；右侧抽屉展示单条请求详情（复制 source / sessionId / requestKey）。
 */
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Copy, ExternalLink, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t } from '../../i18n'
import type { RequestRecord } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useSessionViewData } from '../../composables/useSessionViewData'
import { useClipboard } from '../composables/useClipboard'
import { useInfiniteScroll } from '../composables/useInfiniteScroll'
import LobeIcon from '../../components/LobeIcon.vue'

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
  requestProjectLabel,
  requestSourceLabel,
  requestToolLabel,
  requestCacheTokens,
  requestHasProxyPerformance,
  getToolIcon,
} = useSessionDisplay(store)

// —— 数据加载（activeTab 固定为 'requests'：hook 内部按该值走请求记录分支） ——
const activeTab = ref<'recent' | 'requests' | 'projects'>('requests')
const {
  selectedTool,
  requestHasMore,
  loadingMoreRequests,
  loadMoreRequests,
  reloadRequestRecords,
  initialize: initializeSessionView,
  dispose: disposeSessionView,
} = useSessionViewData(store, activeTab)

// —— 深链/事件导航带来的全局筛选上下文（sourceId/tool）应用；sessionKey 走 hash 路由由宿主分发 ——
async function applyPendingFilters() {
  const pending = nav.consumePendingFilters()
  if (!pending) return
  if (pending.sourceId && store.settings.sourceAware.activeSourceFilter !== pending.sourceId) {
    await store.setActiveSourceFilter(pending.sourceId)
  }
  if (pending.tool && store.settings.clientTools.activeToolFilter !== pending.tool) {
    await store.setActiveToolFilter(pending.tool)
  }
}

// 同页深链：hash 相同页面不重挂载，onMounted 消费路径不执行；
// pendingConsumeTick 变化时若本页激活则补消费（跨页场景由 onMounted 覆盖，这里幂等）。
watch(
  () => nav.pendingConsumeTick,
  () => {
    if (nav.currentPage === 'requests') void applyPendingFilters()
  }
)

// —— 请求筛选（select 的 v-model 值放宽为 string，比较时按字面量处理） ——
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

// —— 右侧请求详情抽屉 ——
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

// 行键盘操作：Enter/Space 打开抽屉（与 click 一致；行内『在会话中查看』按钮聚焦时
// keydown 冒泡被 target !== currentTarget 检查排除，避免同时触发抽屉与跳转）
const handleRowKeydown = (event: KeyboardEvent, request: RequestRecord) => {
  if (event.target !== event.currentTarget) return
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    openRequestDrawer(request)
  }
}

// 『在会话中查看』：关闭抽屉并深链跳转到会话工作区（hash 路由由 desktopNavigation 分发）
const openInSession = () => {
  const sessionKey = selectedRequest.value?.sessionId
  closeRequestDrawer()
  if (sessionKey) nav.openSession(sessionKey)
}

// 请求 ID 折叠为短值，复制时复制完整值
const shortId = (value: string) => (value.length > 12 ? `${value.slice(0, 6)}…${value.slice(-4)}` : value)
const { copiedValue, copyText } = useClipboard()

// 请求筛选变化时选中行可能消失，清空抽屉避免悬挂引用
watch(filteredRequests, list => {
  if (selectedRequest.value && !list.some(request => request.requestKey === selectedRequest.value?.requestKey)) {
    closeRequestDrawer()
  }
})

// —— 触底续载（请求记录观察器） ——
const requestLoadMoreTrigger = ref<HTMLElement | null>(null)
useInfiniteScroll({
  trigger: requestLoadMoreTrigger,
  onLoadMore: loadMoreRequests,
  hasMore: () => requestHasMore.value,
  loading: () => loadingMoreRequests.value,
  delay: 100,
  rootMargin: '160px',
  reobserve: () => store.requestRecords.length,
  reobserveDelay: 60
})

onMounted(async () => {
  // 深链/事件导航带来的全局筛选上下文（sourceId/tool）先应用
  await applyPendingFilters()
  // hook 的 initialize 只 reloadSessions；请求页必须显式加载请求记录
  await initializeSessionView()
  await reloadRequestRecords()
  await nextTick()
})

onUnmounted(() => {
  disposeSessionView()
})

// —— 表格骨架行 ——
const skeletonRows = [0, 1, 2, 3, 4]

// —— 工具筛选选项（从已见会话与请求记录收集） ——
const toolOptions = computed(() => {
  const tools = new Set<string>()
  for (const session of store.sessions) tools.add(session.tool)
  for (const request of store.requestRecords) tools.add(request.tool)
  return [...tools].sort((a, b) => a.localeCompare(b))
})
</script>

<template>
  <div class="flex flex-col gap-3 pb-4">
    <!-- 工具栏：工具下拉 + 请求筛选（状态 / 覆盖来源 / 性能） -->
    <div class="flex flex-wrap items-center gap-2 text-[11px]">
      <!-- 工具下拉（复用 useSessionViewData 的 selectedTool，切换触发后端按工具重载） -->
      <select
        v-model="selectedTool"
        class="theme-input h-7 rounded-lg px-2 text-[11px] outline-none"
        :aria-label="t(locale, 'desktop.sessions.filterTool')"
      >
        <option :value="null">{{ t(locale, 'desktop.allTools') }}</option>
        <option v-for="tool in toolOptions" :key="tool" :value="tool">{{ requestToolLabel(tool) }}</option>
      </select>
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
        <table class="w-full min-w-[860px] border-collapse text-[11px]">
          <thead>
            <tr class="border-b border-[var(--theme-border-default)] text-[9.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
              <th class="px-1.5 py-1.5 text-left">{{ t(locale, 'desktop.sessions.columnTime') }}</th>
              <th class="px-1.5 py-1.5 text-left">{{ t(locale, 'desktop.sessions.columnTool') }}</th>
              <th class="px-1.5 py-1.5 text-left">{{ t(locale, 'desktop.sessions.columnSource') }}</th>
              <th class="px-1.5 py-1.5 text-left">{{ t(locale, 'desktop.sessions.columnModel') }}</th>
              <th class="px-1.5 py-1.5 text-left">{{ t(locale, 'desktop.sessions.columnStatus') }}</th>
              <th class="px-1.5 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnInput') }}</th>
              <th class="px-1.5 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnOutput') }}</th>
              <th class="px-1.5 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnCache') }}</th>
              <th class="px-1.5 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnTotalTokens') }}</th>
              <th class="px-1.5 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnCost') }}</th>
              <th class="px-1.5 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnTtft') }}</th>
              <th class="px-1.5 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnDuration') }}</th>
              <th class="px-1.5 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnRate') }}</th>
              <th class="px-1.5 py-1.5 text-left">{{ t(locale, 'desktop.sessions.columnSession') }}</th>
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
              tabindex="0"
              class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
              :class="selectedRequest?.requestKey === request.requestKey ? 'bg-[var(--theme-accent-soft)]' : ''"
              @click="openRequestDrawer(request)"
              @keydown="handleRowKeydown($event, request)"
            >
              <td class="whitespace-nowrap px-1.5 py-1.5 text-[var(--theme-text-secondary)]">{{ formatTime(request.timestampSec) }}</td>
              <td class="px-1.5 py-1.5">
                <span class="flex items-center gap-1">
                  <LobeIcon v-if="getToolIcon(request.tool)" :slug="getToolIcon(request.tool) ?? 'claudecode'" :size="12" @error="() => {}" />
                  <span v-else class="h-1.5 w-1.5 rounded-full bg-[var(--theme-border-strong)]"></span>
                  <span class="max-w-16 truncate text-[var(--theme-text-secondary)]">{{ requestToolLabel(request.tool) }}</span>
                </span>
              </td>
              <td class="max-w-24 truncate px-1.5 py-1.5 text-[var(--theme-text-secondary)]" :title="requestSourceLabel(request)">{{ requestSourceLabel(request) }}</td>
              <td class="max-w-28 truncate px-1.5 py-1.5 font-mono text-[10px] text-[var(--theme-text-secondary)]" :title="request.model">{{ requestModelLabel(request) }}</td>
              <td class="px-1.5 py-1.5">
                <!-- 本地-only 记录不展示不存在的状态值 -->
                <span v-if="request.coverageOrigin === 'local_only'" class="text-[10px] text-[var(--theme-text-quaternary)]">—</span>
                <span v-else class="inline-flex items-center rounded-full border px-1.5 py-px text-[9px] font-bold leading-none" :class="requestStatusClasses(request)">{{ requestStatusLabel(request) }}</span>
              </td>
              <td class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.inputTokens) }}</td>
              <td class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(request.outputTokens) }}</td>
              <td class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens(requestCacheTokens(request)) }}</td>
              <td class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(request.totalTokens) }}</td>
              <td class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(request.estimatedCost) }}</td>
              <td class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) ? formatDuration(request.ttftMs) : '—' }}</td>
              <td class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) ? formatDuration(request.durationMs) : '—' }}</td>
              <td class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ requestHasProxyPerformance(request) && request.outputTokensPerSecond ? `${request.outputTokensPerSecond.toFixed(1)}t/s` : '—' }}</td>
              <td class="max-w-24 px-1.5 py-1.5 font-mono text-[10px]">
                <!-- 会话 ID：点击/回车跳转会话工作区（stop 阻止行点击打开抽屉）；title 保留完整 ID -->
                <button
                  type="button"
                  tabindex="0"
                  class="block w-full cursor-pointer truncate text-left text-[var(--theme-text-tertiary)] transition-colors hover:text-[var(--theme-accent-primary)] focus-visible:text-[var(--theme-accent-primary)] focus-visible:outline-none"
                  :title="request.sessionId"
                  @keydown.stop
                  @click.stop="nav.openSession(request.sessionId)"
                >{{ shortId(request.sessionId) }}</button>
              </td>
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
            <div class="flex shrink-0 items-start gap-1">
              <button
                type="button"
                class="inline-flex items-center gap-1 rounded-lg border border-[var(--theme-border-default)] px-2 py-1 text-[10px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:border-[var(--theme-accent-primary)] hover:text-[var(--theme-accent-primary)]"
                :title="t(locale, 'desktop.requests.openInSession')"
                @click="openInSession"
              >
                <ExternalLink class="h-3 w-3" aria-hidden="true" />
                {{ t(locale, 'desktop.requests.openInSession') }}
              </button>
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
