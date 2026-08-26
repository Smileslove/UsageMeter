<script setup lang="ts">
/**
 * 中栏活动脉络时间线（设计 9.8 会话内全文搜索 / 9.5 工具调用卡）。
 * - 头部：会话内搜索框（防抖 400ms 自动搜索 / 回车立即）+ 事件计数 + 导出入口；
 * - 搜索模式：命中列表替代时间线，上一项/下一项循环导航（选中并滚动到可见区）；
 * - 常规时间线：事件流（sequence 升序）+ sentinel 触底续载（useInfiniteScroll）；
 * - 通过 emits 与父组件通信：select / load-more / export，自身不持有数据加载状态。
 *   会话内搜索状态随本组件挂载生命周期自然重置（父组件切换会话时 viewState 离开
 *   'ready' 使本组件卸载，等价原 loadSession 内 clearSearch()）。
 */
import { computed, onUnmounted, ref, watch } from 'vue'
import {
  Bot, ChevronDown, ChevronLeft, ChevronRight, Clock, Download,
  FileOutput, Loader2, RefreshCw, Search, TriangleAlert, Wrench, X
} from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t, backendErrorLabel } from '../../../i18n'
import { searchSessionActivity } from '../../../api/activityApi'
import { useInfiniteScroll } from '../../composables/useInfiniteScroll'
import {
  ALL_KINDS, KIND_META, statusMeta,
  formatAbsTime, formatRelTime, formatDuration, shortAgentId
} from './eventMeta'
import type { SessionEventKind, SessionEventListItem } from '../../../types'

const props = defineProps<{
  sessionKey: string | null
  events: SessionEventListItem[]
  total: number
  hasMore: boolean
  loading: boolean
  loadingMore: boolean
  selectedKinds: Set<SessionEventKind>
  activeAgentKey: string | null
  /** 选中事件 key（高亮；由父组件维护，选中事件本身属检查器）。 */
  selectedEventKey: string | null
  fulltextEnabled: boolean
}>()

const emit = defineEmits<{
  select: [event: SessionEventListItem]
  'load-more': []
  export: []
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

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
  const key = props.sessionKey
  const query = searchInput.value.trim()
  if (!key || !props.fulltextEnabled || !query) return
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
  if (!props.fulltextEnabled) return
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
  emit('select', searchResults.value[next])
  requestAnimationFrame(() => {
    document.getElementById(`search-hit-${next}`)?.scrollIntoView({ block: 'nearest' })
  })
}

onUnmounted(() => {
  if (searchDebounceTimer) clearTimeout(searchDebounceTimer)
  searchDebounceTimer = null
})

// ============ 滚动加载（IntersectionObserver，sentinel 触发） ============
const sentinel = ref<HTMLElement | null>(null)
useInfiniteScroll({
  trigger: sentinel,
  onLoadMore: () => emit('load-more'),
  hasMore: () => props.hasMore,
  loading: () => props.loadingMore || props.loading,
  enabled: () => true, // 本组件仅在 viewState === 'ready' 时挂载
  rootMargin: '240px',
  reobserve: sentinel
})
</script>

<template>
  <!-- 中栏：活动脉络时间线（min 480px） -->
  <main class="theme-surface flex min-w-0 flex-1 flex-col overflow-hidden rounded-xl border" :aria-label="t(locale, 'desktop.activity.timelineLabel')">
    <div class="flex shrink-0 items-center gap-2 border-b border-[var(--theme-border-subtle)] px-4 py-2">
      <span class="shrink-0 text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.timelineLabel') }}</span>

      <!-- 会话内搜索（M3 FTS；非 fulltext 档禁用执行，9.8） -->
      <div class="ml-1 flex min-w-0 max-w-xs flex-1 items-center gap-1.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-1">
        <Search class="h-3 w-3 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <input
          v-model="searchInput"
          type="text"
          class="min-w-0 flex-1 bg-transparent text-xs text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)] disabled:opacity-50"
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

      <span class="ml-auto shrink-0 text-xs text-[var(--theme-text-quaternary)]">
        {{ t(locale, 'desktop.activity.eventsCount', { count: total }) }}
        <span v-if="activeAgentKey" class="ml-1.5 text-[var(--theme-accent-primary)]">{{ t(locale, 'desktop.activity.filterActive', { label: activeAgentKey }) }}</span>
      </span>

      <!-- 导出（15.3：范围预览对话框，默认不勾选 payload） -->
      <button
        type="button"
        class="theme-button-secondary inline-flex h-7 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-xs font-semibold"
        :aria-label="t(locale, 'desktop.activity.exportButton')"
        :title="t(locale, 'desktop.activity.exportButton')"
        @click="emit('export')"
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
      <span class="text-xs font-semibold text-[var(--theme-accent-primary)]">{{ t(locale, 'desktop.activity.searchHitCount', { count: searchTotal }) }}</span>
      <span v-if="searchResults.length > 0 && searchActiveIndex >= 0" class="font-mono text-xs text-[var(--theme-text-quaternary)]">
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
          class="ml-1 rounded px-1.5 py-0.5 text-xs font-semibold text-[var(--theme-accent-primary)] hover:underline"
          @click="clearSearch"
        >
          {{ t(locale, 'desktop.activity.searchClear') }}
        </button>
      </div>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto p-3" style="max-height: 62vh">
      <!-- ===== 搜索模式：命中列表（替代时间线，设计 9.8） ===== -->
      <template v-if="searchMode">
        <div v-if="searchLoading && searchResults.length === 0" class="flex items-center justify-center gap-1.5 py-8 text-xs text-[var(--theme-text-tertiary)]">
          <Loader2 class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
          {{ t(locale, 'desktop.activity.timelineLoading') }}
        </div>
        <div v-else-if="searchError" class="flex flex-col items-center justify-center px-6 py-16 text-center">
          <TriangleAlert class="h-6 w-6 text-rose-500" aria-hidden="true" />
          <p class="mt-2 max-w-md break-all text-xs leading-relaxed text-rose-500">{{ backendErrorLabel(locale, searchError) }}</p>
          <button
            type="button"
            class="theme-button-secondary mt-4 inline-flex items-center gap-1.5 rounded-lg px-3.5 py-1.5 text-xs font-semibold"
            @click="runSearch(true)"
          >
            <RefreshCw class="h-3 w-3" aria-hidden="true" />
            {{ t(locale, 'desktop.activity.retry') }}
          </button>
        </div>
        <div v-else-if="searchResults.length === 0" class="flex flex-col items-center justify-center px-6 py-16 text-center">
          <Search class="h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
          <p class="mt-2 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.timelineEmpty') }}</p>
        </div>
        <div v-else class="space-y-1.5">
          <button
            v-for="(event, index) in searchResults"
            :id="`search-hit-${index}`"
            :key="event.eventKey"
            type="button"
            class="block w-full rounded-xl border px-3 py-2.5 text-left transition-colors"
            :class="selectedEventKey === event.eventKey
              ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-soft)]'
              : 'border-[var(--theme-border-subtle)] hover:bg-[var(--theme-bg-hover)]'"
            @click="emit('select', event)"
          >
            <div class="flex items-start gap-2.5">
              <span class="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg" :class="KIND_META[event.kind].cls">
                <component :is="KIND_META[event.kind].icon" class="h-3.5 w-3.5" aria-hidden="true" />
              </span>
              <span class="min-w-0 flex-1">
                <span class="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                  <span class="text-xs font-semibold text-[var(--theme-text-primary)]">{{ t(locale, KIND_META[event.kind].labelKey) }}</span>
                  <span class="text-xs text-[var(--theme-text-tertiary)]" :title="event.timestampMs != null ? formatAbsTime(event.timestampMs, locale) : undefined">
                    {{ event.timestampMs != null ? formatRelTime(event.timestampMs, locale) : t(locale, 'desktop.activity.timeUnknown') }}
                  </span>
                  <span
                    v-if="statusMeta(event.status)"
                    class="inline-flex items-center rounded-full px-1.5 py-px text-xs font-bold leading-none"
                    :class="statusMeta(event.status)!.cls"
                  >
                    {{ t(locale, statusMeta(event.status)!.labelKey) }}
                  </span>
                  <span v-if="event.tool" class="inline-flex items-center gap-0.5 rounded bg-cyan-500/10 px-1.5 py-px font-mono text-xs text-cyan-600 dark:text-cyan-300">
                    <Wrench class="h-2.5 w-2.5" aria-hidden="true" />
                    {{ event.tool.normalizedName || event.tool.rawName }}
                  </span>
                </span>
                <span v-if="event.summary" class="mt-1 line-clamp-2 block break-words text-xs leading-relaxed text-[var(--theme-text-secondary)]">
                  {{ event.summary }}
                </span>
              </span>
              <ChevronRight class="mt-1 h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            </div>
          </button>
          <div v-if="searchHasMore" class="flex items-center justify-center py-2">
            <button
              type="button"
              class="theme-button-secondary inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-semibold disabled:opacity-60"
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
          <p class="mt-2 text-xs text-[var(--theme-text-tertiary)]">
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
            :class="selectedEventKey === event.eventKey
              ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-soft)]'
              : 'border-[var(--theme-border-subtle)] hover:bg-[var(--theme-bg-hover)]'"
            @click="emit('select', event)"
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
                  <span class="text-xs font-semibold text-[var(--theme-text-primary)]">{{ t(locale, KIND_META[event.kind].labelKey) }}</span>
                  <span class="text-xs text-[var(--theme-text-tertiary)]" :title="event.timestampMs != null ? formatAbsTime(event.timestampMs, locale) : undefined">
                    {{ event.timestampMs != null ? formatRelTime(event.timestampMs, locale) : t(locale, 'desktop.activity.timeUnknown') }}
                  </span>
                  <span
                    v-if="statusMeta(event.status)"
                    class="inline-flex items-center rounded-full px-1.5 py-px text-xs font-bold leading-none"
                    :class="statusMeta(event.status)!.cls"
                  >
                    {{ t(locale, statusMeta(event.status)!.labelKey) }}
                  </span>
                  <span v-if="event.actorAgentKey" class="inline-flex items-center gap-0.5 rounded bg-slate-500/10 px-1.5 py-px font-mono text-xs text-slate-500 dark:text-slate-300">
                    <Bot class="h-2.5 w-2.5" aria-hidden="true" />
                    {{ shortAgentId(event.actorAgentKey) }}
                  </span>
                </span>
                <!-- 摘要（最多 2 行截断，默认折叠长文本） -->
                <span v-if="event.summary" class="mt-1 line-clamp-2 block break-words text-xs leading-relaxed text-[var(--theme-text-secondary)]">
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
              <span class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">{{ event.tool.normalizedName || event.tool.rawName }}</span>
              <span v-if="event.tool.family" class="rounded bg-cyan-500/10 px-1.5 py-px text-xs font-bold text-cyan-600 dark:text-cyan-300">{{ event.tool.family }}</span>
              <span v-if="event.tool.durationMs != null" class="inline-flex items-center gap-0.5 text-xs text-[var(--theme-text-tertiary)]">
                <Clock class="h-3 w-3" aria-hidden="true" />
                {{ formatDuration(event.tool.durationMs) }}
              </span>
              <span
                v-for="key in event.tool.inputKeys.slice(0, 6)"
                :key="key"
                class="max-w-28 truncate rounded bg-[var(--theme-border-subtle)] px-1.5 py-px font-mono text-xs text-[var(--theme-text-tertiary)]"
                :title="key"
              >
                {{ key }}
              </span>
              <span v-if="event.tool.inputKeys.length > 6" class="text-xs text-[var(--theme-text-quaternary)]">+{{ event.tool.inputKeys.length - 6 }}</span>
              <span class="ml-auto inline-flex items-center gap-0.5 text-xs font-semibold text-[var(--theme-accent-primary)]">
                {{ t(locale, 'desktop.activity.expand') }}
                <ChevronDown class="h-3 w-3" aria-hidden="true" />
              </span>
            </div>
          </button>

          <!-- 分页 sentinel + 底部状态 -->
          <div ref="sentinel" class="flex items-center justify-center py-2">
            <span v-if="loadingMore" class="inline-flex items-center gap-1.5 text-xs text-[var(--theme-text-tertiary)]">
              <Loader2 class="h-3 w-3 animate-spin" aria-hidden="true" />
              {{ t(locale, 'desktop.activity.timelineLoading') }}
            </span>
            <span v-else-if="!hasMore" class="text-xs text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.noMore') }}</span>
          </div>
        </div>
      </template>
    </div>
  </main>
</template>
