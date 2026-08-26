<script setup lang="ts">
/**
 * 分析页 - 工具栏（设计 7.3）：时间范围 / 粒度 / 主指标 / 比较 / 导出占位。
 * 与页面现有模式一致：直接读写 desktopAnalytics store（analyticsWindow /
 * analyticsGranularity / analyticsMetric / analyticsCompare），无 props / emits。
 */
import { computed, ref } from 'vue'
import { FileDown, ListFilter, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import {
  useDesktopAnalyticsStore,
  type AnalyticsCompare,
  type AnalyticsGranularity
} from '../../stores/desktopAnalytics'
import { metricColor } from '../../composables/useTrendChart'
import { t, windowNameLabel } from '../../../i18n'
import { METRICS } from '../../../components/statistics/activityUtils'
import { WINDOW_ORDER } from '../../../types'
import { formatToolDisplayName } from '../../../utils/toolDisplay'
import { useFocusTrap } from '../../composables/useFocusTrap'

const store = useMonitorStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

const granularityOptions: Array<{ value: AnalyticsGranularity; key: string }> = [
  { value: 'auto', key: 'desktop.analytics.granularityAuto' },
  { value: 'hour', key: 'desktop.analytics.granularityHour' },
  { value: 'day', key: 'desktop.analytics.granularityDay' }
]

const compareOptions: Array<{ value: AnalyticsCompare; key: string }> = [
  { value: 'none', key: 'desktop.analytics.compareNone' },
  { value: 'previous', key: 'desktop.analytics.comparePrevious' }
]

/** 不合法粒度组合：30d/本月 不提供小时粒度，5h/24h/当天 不提供天粒度（设计 7.3）。 */
function granularityDisabled(g: AnalyticsGranularity): boolean {
  if (g === 'hour') {
    return analytics.analyticsWindow === '30d' || analytics.analyticsWindow === 'current_month'
  }
  if (g === 'day') {
    return (
      analytics.analyticsWindow === '5h' ||
      analytics.analyticsWindow === '24h' ||
      analytics.analyticsWindow === 'today'
    )
  }
  return false
}

// ---- 筛选摘要（设计 7.3）：模型/项目组合多选，全局来源/工具只读展示 ----

const filterPanelOpen = ref(false)
const panelRef = ref<HTMLElement | null>(null)
useFocusTrap({
  open: filterPanelOpen,
  container: panelRef,
  onClose: () => {
    filterPanelOpen.value = false
  }
})

const hasFilter = computed(
  () => analytics.analyticsModelFilter.length > 0 || analytics.analyticsProjectFilter.length > 0
)

/** 按钮摘要：『模型 2 · 项目 1』，未筛选时灰态。 */
const filterSummaryText = computed(() => {
  const modelCount = analytics.analyticsModelFilter.length
  const projectCount = analytics.analyticsProjectFilter.length
  if (modelCount === 0 && projectCount === 0) {
    return t(locale.value, 'desktop.analytics.filterNone')
  }
  const parts: string[] = []
  if (modelCount > 0) parts.push(t(locale.value, 'desktop.analytics.filterModelCount', { count: modelCount }))
  if (projectCount > 0) parts.push(t(locale.value, 'desktop.analytics.filterProjectCount', { count: projectCount }))
  return parts.join(' · ')
})

/** 模型选项：优先 statisticsSummary.models（与趋势/总量过滤同口径），再补 overviewBreakdown.modelRanking。 */
const modelOptions = computed(() => {
  const seen = new Map<string, string>()
  for (const m of store.statisticsSummary?.models ?? []) {
    if (!seen.has(m.modelName)) seen.set(m.modelName, m.modelName)
  }
  for (const item of store.overviewBreakdown?.modelRanking ?? []) {
    if (item.kind !== 'model' || seen.has(item.id)) continue
    seen.set(item.id, item.label === '__unknown__' ? t(locale.value, 'sources.unknown') : item.label)
  }
  return [...seen.entries()].map(([id, label]) => ({ id, label }))
})

const projectOptions = computed(() =>
  store.projectStats.map(p => ({
    id: p.projectKey ?? p.name,
    label: p.name === '__unknown__' ? t(locale.value, 'common.unknownProject') : p.name
  }))
)

function isModelSelected(id: string): boolean {
  return analytics.analyticsModelFilter.includes(id)
}

function toggleModel(id: string) {
  const list = analytics.analyticsModelFilter
  analytics.analyticsModelFilter = list.includes(id) ? list.filter(x => x !== id) : [...list, id]
}

function toggleProject(id: string) {
  const list = analytics.analyticsProjectFilter
  analytics.analyticsProjectFilter = list.includes(id) ? list.filter(x => x !== id) : [...list, id]
}

function clearAllFilters() {
  analytics.analyticsModelFilter = []
  analytics.analyticsProjectFilter = []
}

/** 全局来源/工具筛选只读展示（改动走顶栏 SourceSelector/ToolSelector）。 */
const activeSourceLabel = computed(() => {
  const id = store.settings.sourceAware.activeSourceFilter
  if (!id || id === '__unknown__') return t(locale.value, 'desktop.allSources')
  const source = store.settings.sourceAware.sources.find(s => s.id === id)
  return source?.displayName || source?.baseUrl || t(locale.value, 'desktop.allSources')
})

const activeToolLabel = computed(() => {
  const tool = store.settings.clientTools.activeToolFilter
  if (!tool) return t(locale.value, 'desktop.allTools')
  return formatToolDisplayName(tool, locale.value, store.settings.clientTools.profiles)
})
</script>

<template>
  <div class="flex flex-wrap items-center gap-2">
    <!-- 时间范围：页面内部工具栏读写 analyticsWindow -->
    <div
      class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="group"
      :aria-label="t(locale, 'desktop.analytics.toolbarRange')"
    >
      <button
        v-for="w in WINDOW_ORDER"
        :key="w"
        type="button"
        class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
        :class="
          analytics.analyticsWindow === w
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        :aria-pressed="analytics.analyticsWindow === w"
        @click="analytics.analyticsWindow = w"
      >
        {{ windowNameLabel(locale, w) }}
      </button>
    </div>

    <!-- 粒度：自动 / 小时 / 天；不合法组合禁用并说明 -->
    <div
      class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="group"
      :aria-label="t(locale, 'desktop.analytics.toolbarGranularity')"
    >
      <button
        v-for="g in granularityOptions"
        :key="g.value"
        type="button"
        class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150 disabled:cursor-not-allowed disabled:opacity-40"
        :class="
          analytics.analyticsGranularity === g.value
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        :disabled="granularityDisabled(g.value)"
        :title="granularityDisabled(g.value) ? t(locale, 'desktop.analytics.granularityInvalid') : undefined"
        :aria-pressed="analytics.analyticsGranularity === g.value"
        @click="analytics.analyticsGranularity = g.value"
      >
        {{ t(locale, g.key) }}
      </button>
    </div>

    <!-- 主指标：费用 / 请求 / Token -->
    <div
      class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="group"
      :aria-label="t(locale, 'desktop.analytics.toolbarMetric')"
    >
      <button
        v-for="m in METRICS"
        :key="m.value"
        type="button"
        class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
        :class="
          analytics.analyticsMetric === m.value
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        :aria-pressed="analytics.analyticsMetric === m.value"
        @click="analytics.analyticsMetric = m.value"
      >
        <span
          class="h-1.5 w-1.5 rounded-full"
          :style="{ backgroundColor: metricColor(m.value) }"
        ></span>
        {{ t(locale, m.key) }}
      </button>
    </div>

    <!-- 比较：无 / 上一等长周期 -->
    <div
      class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="group"
      :aria-label="t(locale, 'desktop.analytics.toolbarCompare')"
    >
      <button
        v-for="c in compareOptions"
        :key="c.value"
        type="button"
        class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
        :class="
          analytics.analyticsCompare === c.value
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        :aria-pressed="analytics.analyticsCompare === c.value"
        @click="analytics.analyticsCompare = c.value"
      >
        {{ t(locale, c.key) }}
      </button>
    </div>

    <!-- 筛选摘要（设计 7.3）：模型/项目组合多选，全局来源/工具只读展示 -->
    <div class="relative">
      <button
        type="button"
        class="flex h-7 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors duration-150"
        :class="
          hasFilter
            ? 'border-[var(--theme-accent-soft)] bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'border-[var(--theme-border-default)] text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]'
        "
        :aria-expanded="filterPanelOpen"
        :title="t(locale, 'desktop.analytics.filterSummary')"
        @click="filterPanelOpen = !filterPanelOpen"
      >
        <ListFilter :size="14" aria-hidden="true" />
        <span>{{ filterSummaryText }}</span>
      </button>

      <!-- 点击外部关闭：透明遮罩位于浮层之下 -->
      <div
        v-if="filterPanelOpen"
        class="fixed inset-0 z-40"
        @mousedown="filterPanelOpen = false"
        @touchstart="filterPanelOpen = false"
      ></div>

      <div
        v-if="filterPanelOpen"
        ref="panelRef"
        role="dialog"
        :aria-label="t(locale, 'desktop.analytics.filterPanelTitle')"
        class="absolute right-0 top-full z-50 mt-2 w-[min(380px,calc(100vw-2rem))] rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-bg-elevated)] p-3 shadow-lg"
      >
        <div class="flex items-center justify-between gap-2">
          <h3 class="text-xs font-semibold text-[var(--theme-text-secondary)]">
            {{ t(locale, 'desktop.analytics.filterPanelTitle') }}
          </h3>
          <button
            v-if="hasFilter"
            type="button"
            class="flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:bg-[var(--theme-accent-soft)]"
            @click="clearAllFilters"
          >
            <X :size="11" aria-hidden="true" />
            {{ t(locale, 'desktop.analytics.filterClearAll') }}
          </button>
        </div>

        <!-- 模型多选 -->
        <div class="mt-3">
          <p class="text-xs font-medium uppercase tracking-wide text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.analytics.filterModelLabel') }}
          </p>
          <div
            v-if="modelOptions.length"
            class="mt-1.5 max-h-40 overflow-y-auto rounded-md border border-[var(--theme-border-subtle)]"
          >
            <label
              v-for="opt in modelOptions"
              :key="opt.id"
              class="flex cursor-pointer items-center gap-2 px-2 py-1.5 text-xs text-[var(--theme-text-secondary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)]"
            >
              <input
                type="checkbox"
                class="h-3.5 w-3.5 shrink-0 accent-[var(--theme-accent-primary)]"
                :checked="isModelSelected(opt.id)"
                @change="toggleModel(opt.id)"
              />
              <span class="truncate">{{ opt.label }}</span>
            </label>
          </div>
          <p
            v-else
            class="mt-1.5 rounded-md border border-dashed border-[var(--theme-border-subtle)] px-2 py-2 text-center text-xs text-[var(--theme-text-quaternary)]"
          >
            {{ t(locale, 'desktop.analytics.filterEmpty') }}
          </p>
        </div>

        <!-- 项目多选 -->
        <div class="mt-3">
          <p class="text-xs font-medium uppercase tracking-wide text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.analytics.filterProjectLabel') }}
          </p>
          <div
            v-if="projectOptions.length"
            class="mt-1.5 max-h-40 overflow-y-auto rounded-md border border-[var(--theme-border-subtle)]"
          >
            <label
              v-for="opt in projectOptions"
              :key="opt.id"
              class="flex cursor-pointer items-center gap-2 px-2 py-1.5 text-xs text-[var(--theme-text-secondary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)]"
            >
              <input
                type="checkbox"
                class="h-3.5 w-3.5 shrink-0 accent-[var(--theme-accent-primary)]"
                :checked="analytics.analyticsProjectFilter.includes(opt.id)"
                @change="toggleProject(opt.id)"
              />
              <span class="truncate">{{ opt.label }}</span>
            </label>
          </div>
          <p
            v-else
            class="mt-1.5 rounded-md border border-dashed border-[var(--theme-border-subtle)] px-2 py-2 text-center text-xs text-[var(--theme-text-quaternary)]"
          >
            {{ t(locale, 'desktop.analytics.filterEmpty') }}
          </p>
        </div>

        <!-- 全局来源/工具筛选（只读：改动走顶栏） -->
        <div class="mt-3 rounded-md border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-hover)] px-2 py-2 text-xs">
          <p class="flex items-center justify-between gap-2">
            <span class="shrink-0 text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.analytics.filterGlobalSource') }}</span>
            <span class="truncate font-medium text-[var(--theme-text-secondary)]">{{ activeSourceLabel }}</span>
          </p>
          <p class="mt-1 flex items-center justify-between gap-2">
            <span class="shrink-0 text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.analytics.filterGlobalTool') }}</span>
            <span class="truncate font-medium text-[var(--theme-text-secondary)]">{{ activeToolLabel }}</span>
          </p>
          <p class="mt-1.5 text-xs text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.analytics.filterGlobalNote') }}</p>
        </div>
      </div>
    </div>

    <!-- 导出（占位：CSV/PNG 将在后续版本提供） -->
    <button
      type="button"
      class="flex h-7 w-7 items-center justify-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
      :title="t(locale, 'desktop.analytics.exportPlaceholder')"
      :aria-label="t(locale, 'desktop.analytics.exportPlaceholder')"
      aria-disabled="true"
    >
      <FileDown :size="15" aria-hidden="true" />
    </button>
  </div>
</template>
