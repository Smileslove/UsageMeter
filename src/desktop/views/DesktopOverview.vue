<script setup lang="ts">
/**
 * 桌面主窗口 - 概览页（设计文档第 6 章）。
 * 在一个屏幕内回答：用了多少 / 额度是否有风险 / 消耗来自哪里 / 是否有异常。
 * 数据口径与快速面板一致：monitor store 的 snapshot / limitSurvival / overviewBreakdown / rateSummary。
 *
 * 结构：本文件保留页面级编排（时间范围 + 回退窗口口径、趋势图、数据刷新链路），
 * KPI 带 / 额度与生存 / 三维贡献 / 最近会话 / 健康检查分别下沉到
 * views/overview/* 子组件与 composables/useOverviewKpis.ts、useQuotaLimits.ts。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import VChart from 'vue-echarts'
import {
  Activity,
  CheckCircle2,
  CircleAlert,
  Clock,
  Database,
  Info,
  RefreshCw,
  TriangleAlert
} from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { rangeQueryForWindow, useDesktopAnalyticsStore } from '../stores/desktopAnalytics'
import SegmentedControl from '../../components/SegmentedControl.vue'
import { t, windowNameLabel } from '../../i18n'
import {
  buildTrendChartOption,
  formatMetric,
  metricColor,
  metricLabel,
  metricValue,
  registerChartComponents,
  trendLineSeries,
  useChartTheme
} from '../composables/useTrendChart'
import { useOverviewKpis } from '../composables/useOverviewKpis'
import { useQuotaLimits } from '../composables/useQuotaLimits'
import { formatRate } from '../../utils/format'
import { metricValueOfBreakdownItem } from '../../utils/metric'
import { pickEffectiveWindow } from '../../utils/windowFallback'
import {
  GEMINI_OAUTH_PLAN_LABEL_PREFIX,
  OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID,
  OFFICIAL_OPENAI_OAUTH_SOURCE_ID,
  OPENAI_OAUTH_PLAN_LABEL_PREFIX,
  WINDOW_ORDER
} from '../../types'
import type { OverviewBreakdownItem, StatisticsMetric, WindowName, WindowUsage } from '../../types'
import type { ContributionSection } from './overview/ContributionRanks.vue'
import type { HealthItem } from './overview/HealthCard.vue'
import KpiBand from './overview/KpiBand.vue'
import QuotaLimitsCard from './overview/QuotaLimitsCard.vue'
import ContributionRanks from './overview/ContributionRanks.vue'
import RecentSessionsCard from './overview/RecentSessionsCard.vue'
import HealthCard from './overview/HealthCard.vue'

registerChartComponents()

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

// ============ 页面内时间范围 ============

/** 页面级时间范围选项（复用 settings.window* 文案；设计：窗口选择位于页面内部而非全局顶栏）。 */
const RANGES: Array<{ value: WindowName; key: string }> = [
  { value: '5h', key: 'settings.window5h' },
  { value: '24h', key: 'settings.window24h' },
  { value: 'today', key: 'settings.windowToday' },
  { value: '7d', key: 'settings.window7d' },
  { value: '30d', key: 'settings.window30d' },
  { value: 'current_month', key: 'settings.windowCurrentMonth' }
]

// ============ 数据口径 ============

/** 当前概览窗口对应的窗口数据（缺失时回退到第一个可用窗口；回退逻辑见 utils/windowFallback）。 */
const windowData = computed<WindowUsage | null>(() => {
  const windows = store.snapshot?.windows ?? []
  const pick = pickEffectiveWindow(windows, analytics.overviewWindow)
  return windows.find(w => w.window === pick.window) ?? null
})

/** 是否正在显示回退窗口（当前所选时间范围暂无数据）。 */
const isWindowFallback = computed(() => {
  const pick = pickEffectiveWindow(store.snapshot?.windows ?? [], analytics.overviewWindow)
  return pick.isFallback
})

/** 实际生效的窗口（含回退）：KPI/趋势/排行/速率统一跟随。 */
const effectiveWindow = computed<WindowName>(
  () => (windowData.value?.window ?? analytics.overviewWindow) as WindowName
)

/** 回退窗口名（用于提示条文案）。 */
const fallbackWindowName = computed(() => {
  const shown = windowData.value
  if (!shown || !isWindowFallback.value) return ''
  return windowNameLabel(locale.value, shown.window)
})

const rateSummary = computed(() => store.rateSummary)

// ============ KPI 条带（设计 6.4）：计算下沉到 useOverviewKpis ============

const { kpis, hasRateData } = useOverviewKpis(windowData, rateSummary)

// ============ 用量趋势（设计 6.5） ============

/** 当前概览窗口 → 统计查询（5h/24h/today 小时粒度，7d/30d/本月 天粒度）。 */
const trendQuery = computed(() =>
  rangeQueryForWindow(
    effectiveWindow.value,
    store.settings.timezone,
    store.settings.dayBoundaryMode === 'night_owl' ? 4 : 0
  )
)

/** 趋势数据点（statisticsSummary 为共享状态，概览页挂载/切换窗口时主动拉取自己的范围）。 */
const trendPoints = computed(() => store.statisticsSummary?.trend ?? [])

/** 除主指标外额外叠加显示的序列（点击 legend chip 叠加，最多 3 条全部）。 */
const extraMetrics = ref<StatisticsMetric[]>([])
const visibleMetrics = computed(() => new Set<StatisticsMetric>([analytics.analyticsMetric, ...extraMetrics.value]))

function selectTrendMetric(metric: StatisticsMetric) {
  if (!extraMetrics.value.includes(metric)) {
    extraMetrics.value = [...extraMetrics.value, metric]
  }
  // 再次点击不取消，只维持一个当前主指标（设计 6.4）
  analytics.analyticsMetric = metric
}

/** 主题颜色：touch theme 设置触发重算（同 StatisticsTrendChart 的做法，实现在 useTrendChart）。 */
const chartTheme = useChartTheme()

const trendMetrics: StatisticsMetric[] = ['requests', 'tokens', 'cost']

const chartOptions = computed(() => {
  const colors = chartTheme.value
  // 极简 tooltip：时间 + 三指标精确值 + 平均速度（页面特有，保留在页面内）
  const tooltipFormatter = (params: any) => {
    const point = trendPoints.value[params[0].dataIndex]
    if (!point) return ''
    const rows = trendMetrics
      .map(m => {
        const value = formatMetric(m, metricValue(point, m))
        const color = metricColor(m)
        return `<div style="display:flex;align-items:center;gap:6px;margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${color};"></span><span>${metricLabel(m)}: <b>${value}</b></span></div>`
      })
      .join('')
    const rate =
      point.avgTokensPerSecond == null ? '--' : `${formatRate(point.avgTokensPerSecond)} t/s`
    return `<div style="font-weight:600;margin-bottom:4px;">${point.label}</div>${rows}<div style="display:flex;align-items:center;gap:6px;margin-top:3px;"><span style="display:inline-block;width:7px;height:7px;border-radius:999px;background:${colors.series3};"></span><span>${t(locale.value, 'common.avgRate')}: <b>${rate}</b></span></div>`
  }
  // 多指标叠加序列：主指标 2.5 宽渐变面积，其余 1.6 宽实线（点击 legend chip 叠加）
  const series = trendMetrics
    .filter(m => visibleMetrics.value.has(m))
    .map(m => {
      const color = metricColor(m)
      return trendLineSeries({
        name: m,
        data: trendPoints.value.map(p => metricValue(p, m)),
        color,
        primary: m === analytics.analyticsMetric
      })
    })
  return buildTrendChartOption({
    points: trendPoints.value,
    metric: analytics.analyticsMetric,
    colors,
    tooltipFormatter,
    series
  })
})

// ============ 额度与生存（设计 6.6）：计算下沉到 useQuotaLimits ============

const { limitRows, limitLoading } = useQuotaLimits()

// ============ 三维贡献（设计 6.7） ============

const breakdown = computed(() => store.overviewBreakdown)

function primaryValue(item: OverviewBreakdownItem): number {
  return metricValueOfBreakdownItem(item, analytics.analyticsMetric)
}

/** 前 5 + "其他"（其余汇总为一行），占比按当前主指标计算。 */
function topRows(items: OverviewBreakdownItem[]): ContributionSection['rows'] {
  const sorted = [...items].sort((a, b) => primaryValue(b) - primaryValue(a))
  const top = sorted.slice(0, 5)
  const rest = sorted.slice(5)
  const total = Math.max(sorted.reduce((sum, i) => sum + primaryValue(i), 0), 1)
  const rows: ContributionSection['rows'] = top.map(item => ({ item, percent: (primaryValue(item) / total) * 100 }))
  if (rest.length > 0) {
    const restValue = rest.reduce((sum, i) => sum + primaryValue(i), 0)
    const other: OverviewBreakdownItem = {
      id: '__other__',
      label: t(locale.value, 'desktop.overview.contributionOther'),
      kind: 'model',
      color: null,
      icon: null,
      requestCount: rest.reduce((sum, i) => sum + i.requestCount, 0),
      totalTokens: rest.reduce((sum, i) => sum + i.totalTokens, 0),
      inputTokens: 0,
      outputTokens: 0,
      cacheCreateTokens: 0,
      cacheReadTokens: 0,
      cost: restValue,
      percent: 0
    }
    rows.push({ item: other, percent: (restValue / total) * 100 })
  }
  return rows
}

const contributionSections = computed<ContributionSection[]>(() => {
  const b = breakdown.value
  if (!b) return []
  return [
    { key: 'source', titleKey: 'desktop.overview.contributionSources', rows: topRows(b.sourceRanking) },
    { key: 'tool', titleKey: 'desktop.overview.contributionTools', rows: topRows(b.toolRanking) },
    { key: 'model', titleKey: 'desktop.overview.contributionModels', rows: topRows(b.modelRanking) }
  ]
})

/** 点击行：设置过滤并进入分析页（设计 6.7），经 desktopNavigation 深链通道传递筛选上下文。 */
function onContributionClick(sectionKey: 'source' | 'tool' | 'model', item: OverviewBreakdownItem) {
  if (item.id === '__other__') {
    nav.navigate('analytics')
    return
  }
  if (sectionKey === 'source') {
    const sourceId = item.id === OFFICIAL_OPENAI_OAUTH_SOURCE_ID
      || item.id.startsWith(OPENAI_OAUTH_PLAN_LABEL_PREFIX)
      ? OFFICIAL_OPENAI_OAUTH_SOURCE_ID
      : item.id === OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID
        || item.id.startsWith(GEMINI_OAUTH_PLAN_LABEL_PREFIX)
        ? OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID
        : item.id
    nav.applyNavigationTarget({ page: 'analytics', sourceId })
  } else if (sectionKey === 'tool') {
    nav.applyNavigationTarget({ page: 'analytics', tool: item.id })
  } else {
    nav.navigate('analytics')
  }
}

// ============ 最近活跃会话（设计 6.8）：渲染下沉到 RecentSessionsCard ============

const recentSessions = computed(() => store.sessions.slice(0, 5))

// ============ 数据覆盖与异常（设计 6.8）：渲染下沉到 HealthCard ============

function retry() {
  void store.refreshUsage()
  refreshWindowData()
}

const healthItems = computed<HealthItem[]>(() => {
  const items: HealthItem[] = []
  if (store.error) {
    items.push({
      key: 'scanFailed',
      icon: CircleAlert,
      tone: 'danger',
      text: t(locale.value, 'desktop.overview.healthScanFailed'),
      actionKey: 'desktop.overview.healthRetry',
      action: retry
    })
  }
  if (store.snapshot && !hasRateData.value) {
    items.push({
      key: 'proxyCoverage',
      icon: TriangleAlert,
      tone: 'warning',
      text: t(locale.value, 'desktop.overview.healthProxyCoverage'),
      actionKey: 'desktop.manageDataSources',
      action: () => nav.navigate('settings')
    })
  }
  if (store.proxyStatus && !store.proxyStatus.running) {
    items.push({
      key: 'gatewayDown',
      icon: CircleAlert,
      tone: 'danger',
      text: t(locale.value, 'desktop.overview.healthGatewayDown')
    })
  }
  if (store.configuredSourceLastFailureAt) {
    items.push({
      key: 'quotaQuery',
      icon: TriangleAlert,
      tone: 'warning',
      text: t(locale.value, 'desktop.overview.healthQuotaQueryFailed')
    })
  }
  const stale =
    !!store.lastUpdatedEpoch && Date.now() - store.lastUpdatedEpoch * 1000 > 10 * 60_000
  if (stale) {
    items.push({
      key: 'stale',
      icon: Clock,
      tone: 'warning',
      text: t(locale.value, 'desktop.overview.healthStaleData')
    })
  }
  if (items.length === 0) {
    items.push({ key: 'ok', icon: CheckCircle2, tone: 'ok', text: t(locale.value, 'desktop.overview.healthOk') })
  }
  return items
})

// ============ 状态处理（设计 6.9） ============

const isEmpty = computed(() => {
  if (!store.snapshot) return false
  const total = store.snapshot.summary.totalRequests + store.snapshot.summary.totalTokens
  return total <= 0 && store.sessions.length === 0
})

const fatalError = computed(() => !!store.error && !store.snapshot)

// ============ 数据加载 ============

/** 防抖窗口数据请求：overviewWindow 与 effectiveWindow（回退切回）等多个来源
 *  可能在同一 tick 内触发刷新，合并为一次请求；store 侧已有请求序号守卫保证最新落地。
 *  拉取目标 = 用户所选窗口（快照初始仅含 summaryWindow 一个窗口，目标窗口须显式拉取）；
 *  趋势/排行展示 = effectiveWindow（含回退，与 KPI 口径一致）。 */
let refreshTimer: ReturnType<typeof setTimeout> | null = null
/** 窗口数据 + 趋势数据（范围变化或挂载时调用）。 */
function refreshWindowData() {
  if (refreshTimer) clearTimeout(refreshTimer)
  refreshTimer = setTimeout(() => {
    refreshTimer = null
    void store.fetchOverviewDeferredBundle(analytics.overviewWindow)
    void store.fetchStatisticsSummary(trendQuery.value)
  }, 60)
}

/** 深链：消费 pendingFilters 并应用——sourceId/tool 进全局筛选，window/metric 进概览页局部状态（仅接受合法值）。 */
function applyPendingFilters() {
  const filters = nav.consumePendingFilters()
  if (!filters) return
  if (filters.sourceId && filters.sourceId !== store.settings.sourceAware.activeSourceFilter) {
    void store.setActiveSourceFilter(filters.sourceId)
  }
  if (filters.tool && filters.tool !== store.settings.clientTools.activeToolFilter) {
    void store.setActiveToolFilter(filters.tool)
  }
  if (filters.window && (WINDOW_ORDER as readonly string[]).includes(filters.window)) {
    analytics.overviewWindow = filters.window as WindowName
  }
  if (filters.metric && ['cost', 'requests', 'tokens'].includes(filters.metric)) {
    analytics.analyticsMetric = filters.metric as StatisticsMetric
  }
}

// 用户显式切换窗口：目标窗口不在快照列表（refresh_usage_bundle 仅返回
// summaryWindow 一个窗口）时 effectiveWindow 不变、旧 watch 不触发，
// 必须按所选窗口显式拉取（与 SummaryPanel.selectWindow 语义一致）。
watch(
  () => analytics.overviewWindow,
  () => {
    if (store.snapshot) refreshWindowData()
  }
)

// 实际生效窗口（含回退）变化 → 刷新趋势/排行展示态。
// 覆盖两条路径：① 目标窗口数据到达后 effectiveWindow 跟随切换；
// ② 回退态数据到达后 effectiveWindow 切回用户所选窗口。
// 窗口数据已由 overviewWindow 拉取链路上达快照，这里不再重复请求，
// 仅让趋势/排行跟随实际生效窗口（与 KPI 口径一致）。
watch(
  () => effectiveWindow.value,
  () => {
    if (store.snapshot) {
      void store.fetchStatisticsSummary(trendQuery.value)
    }
  }
)

// 同页深链：hash 相同页面不重挂载，onMounted 消费路径不执行；
// pendingConsumeTick 变化时若本页激活则补消费（跨页场景由 onMounted 覆盖，这里幂等）。
watch(
  () => nav.pendingConsumeTick,
  () => {
    if (nav.currentPage === 'overview') applyPendingFilters()
  }
)

// snapshot 到达（说明 settings 已加载完成）后，继承一次快速面板 summaryWindow 并补齐窗口数据
watch(
  () => store.snapshot,
  snapshot => {
    if (snapshot) {
      analytics.inheritSummaryWindowOnce()
      if (!store.overviewBreakdown) refreshWindowData()
    }
  },
  { immediate: true }
)

onMounted(() => {
  applyPendingFilters()
  void store.fetchSessions(30, 0, false)
  if (store.snapshot) refreshWindowData()
})

onUnmounted(() => {
  if (refreshTimer) clearTimeout(refreshTimer)
  refreshTimer = null
})
</script>

<template>
  <section class="flex flex-col gap-5">
    <!-- 页面顶部行：回退提示（如有）+ 页面内时间范围选择（窗口选择归属页面，不放全局顶栏） -->
    <div class="flex items-center justify-between gap-3">
      <div
        v-if="isWindowFallback"
        class="flex min-w-0 flex-1 items-center gap-2 rounded-lg border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-3 py-2 text-xs text-[var(--theme-text-secondary)]"
        role="note"
      >
        <Info :size="14" class="shrink-0 text-[var(--theme-text-tertiary)]" aria-hidden="true" />
        <span class="truncate">
          {{ t(locale, 'desktop.overview.windowFallbackHint', { fallback: fallbackWindowName }) }}
        </span>
      </div>
      <span v-else class="flex-1" aria-hidden="true" />

      <div
        class="flex shrink-0 items-center rounded-lg border border-[var(--theme-border-default)] p-0.5"
        role="group"
        :aria-label="t(locale, 'settings.summaryWindow')"
      >
        <button
          v-for="item in RANGES"
          :key="item.value"
          type="button"
          class="rounded-md px-2.5 py-1 text-xs font-medium transition-colors duration-150"
          :class="
            analytics.overviewWindow === item.value
              ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
              : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
          "
          @click="analytics.overviewWindow = item.value"
        >
          {{ windowNameLabel(locale, item.value) }}
        </button>
      </div>
    </div>
    <!-- 加载失败：页面内错误 + 重试（设计 6.9） -->
    <div
      v-if="fatalError"
      class="flex flex-col items-center gap-3 rounded-lg border border-[var(--theme-status-danger-border)] bg-[var(--theme-status-danger-bg)] px-6 py-10 text-center"
      role="alert"
    >
      <CircleAlert :size="22" class="text-[var(--theme-status-danger-fg)]" aria-hidden="true" />
      <div>
        <p class="text-sm font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.overview.errorTitle') }}</p>
        <p class="mt-1 max-w-md text-xs leading-5 text-[var(--theme-text-tertiary)]">{{ store.error }}</p>
      </div>
      <button
        type="button"
        class="flex items-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3 py-1.5 text-xs font-semibold text-[var(--theme-accent-contrast)] transition-opacity duration-150 hover:opacity-90"
        @click="retry"
      >
        <RefreshCw :size="13" aria-hidden="true" />
        {{ t(locale, 'desktop.overview.errorRetry') }}
      </button>
    </div>

    <!-- 完全空数据：中央说明 + 主操作"管理数据源"（设计 6.9） -->
    <div
      v-else-if="isEmpty"
      class="flex flex-col items-center gap-3 rounded-lg border border-dashed border-[var(--theme-border-strong)] px-6 py-16 text-center"
    >
      <Database :size="22" class="text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <div>
        <p class="text-sm font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.overview.emptyTitle') }}</p>
        <p class="mt-1 max-w-md text-xs leading-5 text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.overview.emptyDesc') }}</p>
      </div>
      <button
        type="button"
        class="rounded-lg bg-[var(--theme-accent-primary)] px-3 py-1.5 text-xs font-semibold text-[var(--theme-accent-contrast)] transition-opacity duration-150 hover:opacity-90"
        @click="nav.navigate('settings')"
      >
        {{ t(locale, 'desktop.manageDataSources') }}
      </button>
    </div>

    <template v-else>
      <!-- KPI 条带：一条连续表面 6 个指标（设计 6.4），固定高度骨架保证布局不跳动 -->
      <KpiBand
        :kpis="kpis"
        :active-metric="analytics.analyticsMetric"
        :locale="locale"
        @select="analytics.analyticsMetric = $event"
      />

      <!-- 用量趋势 + 额度与生存 -->
      <div class="grid grid-cols-1 gap-5 xl:grid-cols-3">
        <!-- 用量趋势（约 2/3 宽） -->
        <div class="rounded-lg border border-[var(--theme-border-default)] p-4 xl:col-span-2" style="background: var(--theme-surface-gradient)">
          <div class="mb-3 flex items-center justify-between gap-2">
            <h3 class="flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
              <Activity :size="14" class="shrink-0" aria-hidden="true" />
              {{ t(locale, 'desktop.overview.trendTitle') }}
            </h3>
            <SegmentedControl
              :active-index="Math.max(0, trendMetrics.findIndex(metric => metric === analytics.analyticsMetric))"
              :aria-label="t(locale, 'desktop.overview.trendMetric')"
              tone="soft"
              class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
            >
              <button
                v-for="m in trendMetrics"
                :key="m"
                type="button"
                class="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
                :class="
                  analytics.analyticsMetric === m
                    ? 'text-[var(--theme-accent-primary)]'
                    : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
                "
                :aria-pressed="analytics.analyticsMetric === m"
                @click="selectTrendMetric(m)"
              >
                <span class="h-1.5 w-1.5 rounded-full" :style="{ backgroundColor: metricColor(m) }"></span>
                {{ metricLabel(m) }}
              </button>
            </SegmentedControl>
          </div>

          <div v-if="trendPoints.length" class="h-[240px]">
            <v-chart class="h-full w-full" :option="chartOptions" autoresize />
          </div>
          <div
            v-else-if="store.statisticsLoading"
            class="h-[240px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"
            :aria-label="t(locale, 'common.syncing')"
          ></div>
          <div
            v-else
            class="grid h-[240px] place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] text-xs text-[var(--theme-text-tertiary)]"
          >
            {{ t(locale, 'desktop.overview.trendNoData') }}
          </div>
        </div>

        <!-- 额度与生存（约 1/3 宽）：按来源纵向列表（设计 6.6） -->
        <QuotaLimitsCard
          :rows="limitRows"
          :loading="limitLoading"
          :locale="locale"
          @open-analytics="nav.navigate('analytics')"
        />
      </div>

      <!-- 三维贡献：来源 / 工具 / 模型排行（设计 6.7） -->
      <ContributionRanks
        :sections="contributionSections"
        :metric="analytics.analyticsMetric"
        :locale="locale"
        :currency="store.settings.currency"
        :profiles="store.settings.clientTools.profiles"
        :loading="store.overviewBreakdownLoading"
        :has-breakdown="!!store.overviewBreakdown"
        @open="onContributionClick"
      />

      <!-- 最近活跃会话 + 数据覆盖与异常 -->
      <div class="grid grid-cols-1 gap-5 xl:grid-cols-3">
        <RecentSessionsCard
          :sessions="recentSessions"
          :total-sessions="store.sessions.length"
          :loading="store.sessionsLoading"
          :locale="locale"
          :currency="store.settings.currency"
          :profiles="store.settings.clientTools.profiles"
          @open-session="nav.openSession"
          @view-all="nav.navigate('sessions')"
        />
        <HealthCard :items="healthItems" :locale="locale" />
      </div>
    </template>
  </section>
</template>
