<script setup lang="ts">
/**
 * 桌面主窗口 - 分析页（设计文档第 7 章）。
 * 拆分为 tab 子组件后的骨架：顶部 tabs 切换、工具栏挂载、四视图挂载点、
 * 数据拉取编排（趋势 / 活跃度）与同页深链消费。各视图渲染逻辑见 views/analytics/ 子组件：
 * - AnalyticsToolbar.vue：时间范围/粒度/主指标/比较/导出占位（直接读写 desktopAnalytics store）
 * - TrendView.vue：趋势图 + 总量摘要 + 峰值表（props 收趋势数据，emit retry）
 * - BreakdownView.vue：构成 ranking（自读共享 store）
 * - ActivityView.vue：活跃度月历/年度贡献（props 收月份状态，emit 翻月/选日）
 * - PerformanceView.vue：性能视图（自读共享 store）
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import {
  previousRangeQuery,
  rangeQueryForWindow,
  useDesktopAnalyticsStore,
  type AnalyticsView
} from '../stores/desktopAnalytics'
import { t } from '../../i18n'
import { WINDOW_ORDER } from '../../types'
import type { StatisticsMetric, StatisticsSummary, StatisticsTrendPoint, WindowName } from '../../types'
import AnalyticsToolbar from './analytics/AnalyticsToolbar.vue'
import TrendView from './analytics/TrendView.vue'
import BreakdownView from './analytics/BreakdownView.vue'
import ActivityView from './analytics/ActivityView.vue'
import PerformanceView from './analytics/PerformanceView.vue'

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

const dayBoundaryHour = computed(() => (store.settings.dayBoundaryMode === 'night_owl' ? 4 : 0))

// ============ 顶部 tabs（设计 7.2：二级视图，不加入侧栏） ============

const viewTabs: Array<{ value: AnalyticsView; key: string }> = [
  { value: 'trend', key: 'desktop.analytics.tabsTrend' },
  { value: 'composition', key: 'desktop.analytics.tabsComposition' },
  { value: 'activity', key: 'desktop.analytics.tabsActivity' },
  { value: 'performance', key: 'desktop.analytics.tabsPerformance' }
]

// ============ 趋势视图数据编排（渲染在 TrendView，设计 7.4） ============

/** 分析范围查询：auto 粒度跟随窗口默认，hour/day 由工具栏覆盖。 */
const trendQuery = computed(() => {
  const q = rangeQueryForWindow(analytics.analyticsWindow, store.settings.timezone, dayBoundaryHour.value)
  if (analytics.analyticsGranularity === 'hour') return { ...q, bucket: 'hour' as const }
  if (analytics.analyticsGranularity === 'day') return { ...q, bucket: 'day' as const }
  return q
})

/** 上一等长周期摘要（比较模式开启时）。statisticsSummary 是共享状态，串行拉取避免互相覆盖。 */
const prevSummary = ref<StatisticsSummary | null>(null)

async function fetchTrend() {
  await store.fetchStatisticsSummary(trendQuery.value)
  if (analytics.analyticsCompare === 'previous') {
    await store.fetchStatisticsSummary(previousRangeQuery(trendQuery.value))
    prevSummary.value = store.statisticsSummary
    // 恢复主显示为当前周期（prev 数据只进 prevSummary）
    await store.fetchStatisticsSummary(trendQuery.value)
  } else {
    prevSummary.value = null
  }
}

/**
 * 趋势点按模型筛选（设计 7.3 筛选摘要）：后端按模型返回趋势序列（statisticsSummary.models[].trend），
 * 前端按选中模型合并时间桶求和。模型级趋势缺失时退回全量（口径受限，见筛选摘要说明）。
 */
function filterTrendByModels(summary: StatisticsSummary | null, models: string[]): StatisticsTrendPoint[] {
  if (!summary) return []
  if (models.length === 0) return summary.trend ?? []
  const selected = new Set(models)
  const byEpoch = new Map<number, StatisticsTrendPoint>()
  for (const model of summary.models) {
    if (!selected.has(model.modelName)) continue
    for (const p of model.trend) {
      const acc = byEpoch.get(p.startEpoch)
      if (acc) {
        acc.requestCount += p.requestCount
        acc.totalTokens += p.totalTokens
        acc.inputTokens += p.inputTokens
        acc.outputTokens += p.outputTokens
        acc.cacheCreateTokens += p.cacheCreateTokens
        acc.cacheReadTokens += p.cacheReadTokens
        acc.cost += p.cost
        acc.avgTokensPerSecond = null
      } else {
        byEpoch.set(p.startEpoch, { ...p })
      }
    }
  }
  if (byEpoch.size === 0) return summary.trend ?? []
  return [...byEpoch.values()].sort((a, b) => a.startEpoch - b.startEpoch)
}

const trendPoints = computed(() => filterTrendByModels(store.statisticsSummary, analytics.analyticsModelFilter))
const prevPoints = computed(() => filterTrendByModels(prevSummary.value, analytics.analyticsModelFilter))

watch(
  () => [analytics.analyticsWindow, analytics.analyticsGranularity, analytics.analyticsCompare] as const,
  () => void fetchTrend()
)

// ============ 活跃度视图状态与数据编排（渲染在 ActivityView，设计 7.6） ============

const currentMonth = ref(new Date())
const selectedDate = ref('')

const monthYear = computed(() => currentMonth.value.getFullYear())
const monthNumber = computed(() => currentMonth.value.getMonth() + 1)

function moveMonth(delta: number) {
  currentMonth.value = new Date(monthYear.value, currentMonth.value.getMonth() + delta, 1)
  selectedDate.value = ''
}

function selectDay(date: string) {
  selectedDate.value = date
}

watch(
  () => [monthYear.value, monthNumber.value, analytics.analyticsMetric] as const,
  () => {
    void store.fetchMonthActivity(monthYear.value, monthNumber.value, analytics.analyticsMetric)
    void store.fetchYearActivity(monthYear.value, analytics.analyticsMetric)
  },
  { immediate: true }
)

// ============ 数据加载 ============

function applyPendingFilters() {
  const filters = nav.consumePendingFilters()
  if (!filters) return
  if (filters.sourceId && filters.sourceId !== store.settings.sourceAware.activeSourceFilter) {
    void store.setActiveSourceFilter(filters.sourceId)
  }
  if (filters.tool && filters.tool !== store.settings.clientTools.activeToolFilter) {
    void store.setActiveToolFilter(filters.tool)
  }
  // window/metric/view 属于分析页局部状态（设计 3.5）：仅接受合法值，非法忽略。
  if (filters.window && (WINDOW_ORDER as readonly string[]).includes(filters.window)) {
    analytics.analyticsWindow = filters.window as WindowName
  }
  if (filters.metric && ['cost', 'requests', 'tokens'].includes(filters.metric)) {
    analytics.analyticsMetric = filters.metric as StatisticsMetric
  }
  if (filters.view && ['trend', 'composition', 'activity', 'performance'].includes(filters.view)) {
    analytics.analyticsView = filters.view as AnalyticsView
  }
}

// 同页深链：hash 相同页面不重挂载，onMounted 消费路径不执行；
// pendingConsumeTick 变化时若本页激活则补消费（跨页场景由 onMounted 覆盖，这里幂等）。
watch(
  () => nav.pendingConsumeTick,
  () => {
    if (nav.currentPage === 'analytics') applyPendingFilters()
  }
)

onMounted(() => {
  applyPendingFilters()
  void store.fetchOverviewBreakdown(analytics.analyticsWindow)
  void store.fetchProjectStats()
  void fetchTrend()
})
</script>

<template>
  <section class="flex flex-col gap-4">
    <!-- 顶部 tabs：趋势 / 构成 / 活跃度 / 性能（设计 7.2） -->
    <div
      class="flex w-fit items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
      role="tablist"
      :aria-label="t(locale, 'desktop.analytics.tabsLabel')"
    >
      <button
        v-for="tab in viewTabs"
        :key="tab.value"
        type="button"
        role="tab"
        :aria-selected="analytics.analyticsView === tab.value"
        class="rounded-md px-3 py-1.5 text-xs font-medium transition-colors duration-150"
        :class="
          analytics.analyticsView === tab.value
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
        "
        @click="analytics.analyticsView = tab.value"
      >
        {{ t(locale, tab.key) }}
      </button>
    </div>

    <!-- 工具栏：时间范围 / 粒度 / 主指标 / 比较 / 导出占位（设计 7.3） -->
    <AnalyticsToolbar />

    <!-- 趋势视图（设计 7.4）：数据编排在父级，渲染在 TrendView -->
    <TrendView
      v-if="analytics.analyticsView === 'trend'"
      :points="trendPoints"
      :prev-points="prevPoints"
      :compare="analytics.analyticsCompare"
      :statistics-loading="store.statisticsLoading"
      @retry="() => void fetchTrend()"
    />

    <!-- 构成视图（设计 7.5） -->
    <BreakdownView v-else-if="analytics.analyticsView === 'composition'" />

    <!-- 活跃度视图（设计 7.6）：月份状态与拉取在父级 -->
    <ActivityView
      v-else-if="analytics.analyticsView === 'activity'"
      :current-month="currentMonth"
      :selected-date="selectedDate"
      @move-month="moveMonth"
      @select-day="selectDay"
    />

    <!-- 性能视图（设计 7.7） -->
    <PerformanceView v-else />
  </section>
</template>
