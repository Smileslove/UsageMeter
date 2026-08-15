<script setup lang="ts">
/**
 * 分析页 - 构成视图（设计 7.5）：按来源/工具/模型/项目四个维度展示构成 ranking。
 * 数据来自共享 store（overviewBreakdown / projectStats，父级 onMounted 拉取），
 * 维度切换与列排序是本组件局部状态，直接读写 desktopAnalytics store 的主指标。
 */
import { computed, ref } from 'vue'
import { useMonitorStore } from '../../../stores/monitor'
import { useDesktopAnalyticsStore } from '../../stores/desktopAnalytics'
import { t } from '../../../i18n'
import {
  formatCost,
  formatRate,
  formatRequestCount,
  formatTokenValue
} from '../../../utils/format'
import { formatToolDisplayName } from '../../../utils/toolDisplay'
import type { OverviewBreakdownItem, ProjectStats } from '../../../types'

const store = useMonitorStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

// ---- 维度与排序状态 ----

type CompositionDimension = 'source' | 'tool' | 'model' | 'project'

const dimensionOptions: Array<{ value: CompositionDimension; key: string }> = [
  { value: 'source', key: 'desktop.analytics.dimensionSource' },
  { value: 'tool', key: 'desktop.analytics.dimensionTool' },
  { value: 'model', key: 'desktop.analytics.dimensionModel' },
  { value: 'project', key: 'desktop.analytics.dimensionProject' }
]

const dimension = ref<CompositionDimension>('source')

type CompositionSortKey = 'percent' | 'requests' | 'tokens' | 'cost' | 'speed' | 'errorRate' | 'lastSeen'

interface CompositionRow {
  id: string
  name: string
  percent: number
  requests: number
  tokens: number
  inputTokens: number
  outputTokens: number
  cost: number
  speed: number | null
  errorRate: number | null
  lastSeen: number | null
}

const sortKey = ref<CompositionSortKey>('percent')
const sortDir = ref<'asc' | 'desc'>('desc')

const sortableColumns: Array<{ key: CompositionSortKey; labelKey: string }> = [
  { key: 'percent', labelKey: 'desktop.analytics.compPercent' },
  { key: 'requests', labelKey: 'desktop.analytics.compRequests' },
  { key: 'tokens', labelKey: 'desktop.analytics.compTokens' },
  { key: 'cost', labelKey: 'desktop.analytics.compCost' },
  { key: 'speed', labelKey: 'desktop.analytics.compSpeed' },
  { key: 'errorRate', labelKey: 'desktop.analytics.compErrorRate' },
  { key: 'lastSeen', labelKey: 'desktop.analytics.compLastActive' }
]

// ---- 行构建 ----

function primaryValueOf(item: OverviewBreakdownItem): number {
  if (analytics.analyticsMetric === 'cost') return item.cost
  if (analytics.analyticsMetric === 'tokens') return item.totalTokens
  return item.requestCount
}

function projectPrimaryValue(p: ProjectStats): number {
  if (analytics.analyticsMetric === 'cost') return p.totalCost
  if (analytics.analyticsMetric === 'tokens') {
    return p.totalInputTokens + p.totalOutputTokens + p.totalCacheCreateTokens + p.totalCacheReadTokens
  }
  return p.requestCount
}

function displayLabel(item: OverviewBreakdownItem): string {
  if (item.kind === 'tool') {
    return formatToolDisplayName(item.id, locale.value, store.settings.clientTools.profiles)
  }
  if (item.label === '__unknown__') return t(locale.value, 'sources.unknown')
  if (item.label === '__official_api__') return t(locale.value, 'sources.officialAnthropic')
  return item.label
}

function rowsFromItems(items: OverviewBreakdownItem[]): CompositionRow[] {
  const total = Math.max(items.reduce((sum, i) => sum + primaryValueOf(i), 0), 1)
  return items.map(item => ({
    id: item.id,
    name: displayLabel(item),
    percent: (primaryValueOf(item) / total) * 100,
    requests: item.requestCount,
    tokens: item.totalTokens,
    inputTokens: item.inputTokens,
    outputTokens: item.outputTokens,
    cost: item.cost,
    speed: item.avgTokensPerSecond ?? null,
    errorRate: item.errorRequests != null && item.requestCount > 0 ? (item.errorRequests / item.requestCount) * 100 : null,
    lastSeen: item.lastSeenMs ?? null
  }))
}

function rowsFromProjects(projects: ProjectStats[]): CompositionRow[] {
  const total = Math.max(projects.reduce((sum, p) => sum + projectPrimaryValue(p), 0), 1)
  return projects.map(p => ({
    id: p.projectKey ?? p.name,
    name: p.name === '__unknown__' ? t(locale.value, 'common.unknownProject') : p.name,
    percent: (projectPrimaryValue(p) / total) * 100,
    requests: p.requestCount,
    tokens: p.totalInputTokens + p.totalOutputTokens + p.totalCacheCreateTokens + p.totalCacheReadTokens,
    inputTokens: p.totalInputTokens + p.totalCacheReadTokens,
    outputTokens: p.totalOutputTokens,
    cost: p.totalCost,
    speed: null,
    errorRate: null,
    lastSeen: p.lastActive
  }))
}

const compositionData = computed<CompositionRow[]>(() => {
  const b = store.overviewBreakdown
  if (dimension.value === 'source') return rowsFromItems(b?.sourceRanking ?? [])
  if (dimension.value === 'tool') return rowsFromItems(b?.toolRanking ?? [])
  if (dimension.value === 'model') return rowsFromItems(b?.modelRanking ?? [])
  return rowsFromProjects(store.projectStats)
})

// ---- 排序 ----

function columnValue(row: CompositionRow, key: CompositionSortKey): number {
  if (key === 'percent') return row.percent
  if (key === 'requests') return row.requests
  if (key === 'tokens') return row.tokens
  if (key === 'cost') return row.cost
  if (key === 'speed') return row.speed ?? -1
  if (key === 'errorRate') return row.errorRate ?? -1
  return row.lastSeen ?? -1
}

/** 排序使用原始数值，不使用格式化字符串（设计 7.8）。 */
const sortedComposition = computed(() => {
  const rows = [...compositionData.value]
  const dir = sortDir.value === 'desc' ? -1 : 1
  rows.sort((a, b) => {
    const primary = (columnValue(b, sortKey.value) - columnValue(a, sortKey.value)) * dir
    if (primary !== 0) return primary
    return b.requests - a.requests || a.name.localeCompare(b.name)
  })
  return rows
})

function toggleSort(key: CompositionSortKey) {
  if (sortKey.value === key) {
    sortDir.value = sortDir.value === 'desc' ? 'asc' : 'desc'
  } else {
    sortKey.value = key
    sortDir.value = 'desc'
  }
}

function switchDimension(d: CompositionDimension) {
  dimension.value = d
  sortKey.value = 'percent'
  sortDir.value = 'desc'
}

// ---- 展示工具 ----

function formatRelativeTime(epoch: number | null): string {
  if (!epoch) return '—'
  const diffMs = Date.now() - epoch * 1000
  if (diffMs < 60_000) return t(locale.value, 'common.justNow')
  const minutes = Math.floor(diffMs / 60_000)
  if (minutes < 60) return `${minutes}${t(locale.value, 'common.minutesAgo')}`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}${t(locale.value, 'common.hoursAgo')}`
  return `${Math.floor(hours / 24)}${t(locale.value, 'common.daysAgo')}`
}

/** 仅当 store 有代理覆盖数据时渲染（与性能视图同口径，用于 speed 列）。 */
const performance = computed(() => store.statisticsSummary?.performance ?? null)
const hasPerformance = computed(
  () => !!store.statisticsSummary?.capability.hasPerformance && !!performance.value && performance.value.requestCount > 0
)
</script>

<template>
  <div class="flex flex-col gap-3">
    <div class="flex flex-wrap items-center gap-2">
      <div
        class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
        role="group"
        :aria-label="t(locale, 'desktop.analytics.dimensionLabel')"
      >
        <button
          v-for="d in dimensionOptions"
          :key="d.value"
          type="button"
          class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
          :class="
            dimension === d.value
              ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
              : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
          "
          :aria-pressed="dimension === d.value"
          @click="switchDimension(d.value)"
        >
          {{ t(locale, d.key) }}
        </button>
      </div>
    </div>

    <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
      <div v-if="sortedComposition.length" class="overflow-x-auto">
        <table class="w-full min-w-[720px] text-xs">
          <thead>
            <tr class="border-b border-[var(--theme-border-subtle)] text-left text-[10px] font-medium uppercase tracking-wide text-[var(--theme-text-quaternary)]">
              <th class="py-2 pr-3 font-medium">#</th>
              <th class="py-2 pr-3 font-medium">{{ t(locale, 'desktop.analytics.compName') }}</th>
              <th
                v-for="col in sortableColumns"
                :key="col.key"
                class="cursor-pointer select-none py-2 pr-3 font-medium transition-colors duration-150 hover:text-[var(--theme-text-secondary)]"
                :class="sortKey === col.key ? 'text-[var(--theme-accent-primary)]' : ''"
                :aria-sort="sortKey === col.key ? (sortDir === 'desc' ? 'descending' : 'ascending') : 'none'"
                @click="toggleSort(col.key)"
              >
                <span class="inline-flex items-center gap-0.5">
                  {{ t(locale, col.labelKey) }}
                  <span v-if="sortKey === col.key" class="text-[9px]">{{ sortDir === 'desc' ? '↓' : '↑' }}</span>
                </span>
              </th>
            </tr>
          </thead>
          <tbody class="divide-y divide-[var(--theme-border-subtle)]">
            <tr
              v-for="(row, index) in sortedComposition"
              :key="row.id"
              class="transition-colors duration-150 hover:bg-[var(--theme-bg-hover)]"
            >
              <td class="py-2 pr-3 text-[var(--theme-text-quaternary)]">{{ index + 1 }}</td>
              <td class="max-w-[180px] truncate py-2 pr-3 font-medium text-[var(--theme-text-primary)]" :title="row.name">
                {{ row.name }}
              </td>
              <td class="py-2 pr-3">
                <span class="flex items-center gap-2">
                  <span class="h-1 w-20 overflow-hidden rounded-full bg-[var(--theme-text-primary)]/10">
                    <span class="block h-full rounded-full" :style="{ width: `${Math.min(row.percent, 100)}%`, backgroundColor: 'var(--theme-accent-primary)' }"></span>
                  </span>
                  <span class="font-mono text-[10px] text-[var(--theme-text-tertiary)]">{{ formatRate(row.percent) }}%</span>
                </span>
              </td>
              <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatRequestCount(row.requests) }}</td>
              <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]" :title="`${t(locale, 'desktop.analytics.compInput')} ${formatTokenValue(row.inputTokens)} · ${t(locale, 'desktop.analytics.compOutput')} ${formatTokenValue(row.outputTokens)}`">
                {{ formatTokenValue(row.tokens) }}
              </td>
              <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatCost(row.cost, store.settings.currency) }}</td>
              <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">
                {{ row.speed == null || !hasPerformance ? '—' : `${formatRate(row.speed)} t/s` }}
              </td>
              <td class="py-2 pr-3 text-right font-mono text-[var(--theme-text-secondary)]">
                {{ row.errorRate == null ? '—' : `${formatRate(row.errorRate)}%` }}
              </td>
              <td class="py-2 text-right text-[var(--theme-text-tertiary)]" :title="row.lastSeen ? new Date(row.lastSeen).toLocaleString(locale.replace('_', '-')) : undefined">
                {{ formatRelativeTime(row.lastSeen) }}
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <div
        v-else
        class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-10 text-center text-xs leading-5 text-[var(--theme-text-tertiary)]"
      >
        {{ t(locale, 'desktop.analytics.compEmpty') }}
      </div>
    </div>
  </div>
</template>
