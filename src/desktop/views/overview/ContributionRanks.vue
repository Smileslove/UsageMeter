<script lang="ts">
/**
 * 三维贡献排行的数据契约：由父页面从 overviewBreakdown 计算后经 props 传入。
 * 普通 script 块用于导出类型（script setup 无法导出）。
 */
import type { OverviewBreakdownItem } from '../../../types'

export interface ContributionRow {
  item: OverviewBreakdownItem
  percent: number
}

export interface ContributionSection {
  key: 'source' | 'tool' | 'model'
  titleKey: string
  rows: ContributionRow[]
}
</script>

<script setup lang="ts">
/**
 * 概览页三维贡献排行（设计 6.7）：来源 / 工具 / 模型各前 5 + "其他"。
 * 数据由父页面计算后传入；点击行通知父页面设置过滤并进入分析页。
 */
import type { PropType, Component } from 'vue'
import { Layers3, LayoutGrid, Zap } from 'lucide-vue-next'
import { t } from '../../../i18n'
import { metricLabel } from '../../composables/useTrendChart'
import { formatCost, formatRate, formatRequestCount, formatTokenValue } from '../../../utils/format'
import { formatToolDisplayName } from '../../../utils/toolDisplay'
import type { AppLocale, ClientToolProfile, CurrencySettings, StatisticsMetric } from '../../../types'

const props = defineProps({
  sections: { type: Array as PropType<ContributionSection[]>, required: true },
  metric: { type: String as PropType<StatisticsMetric>, required: true },
  locale: { type: String as PropType<AppLocale>, required: true },
  currency: { type: Object as PropType<CurrencySettings>, required: true },
  profiles: { type: Array as PropType<ClientToolProfile[]>, required: true },
  /** 排行数据加载中（store.overviewBreakdownLoading）。 */
  loading: { type: Boolean, required: true },
  /** 排行数据是否已到达（store.overviewBreakdown 非空）。 */
  hasBreakdown: { type: Boolean, required: true }
})

const emit = defineEmits<{
  open: [sectionKey: 'source' | 'tool' | 'model', item: OverviewBreakdownItem]
}>()

function sectionIcon(key: string): Component {
  if (key === 'source') return Layers3
  if (key === 'tool') return LayoutGrid
  return Zap
}

function displayLabel(item: OverviewBreakdownItem): string {
  if (item.kind === 'tool') {
    return formatToolDisplayName(item.id, props.locale, props.profiles)
  }
  if (item.label === '__unknown__') return t(props.locale, 'sources.unknown')
  if (item.label === '__official_api__') return t(props.locale, 'sources.officialAnthropic')
  return item.label
}

function formatContributionPrimary(item: OverviewBreakdownItem): string {
  if (props.metric === 'cost') return formatCost(item.cost, props.currency)
  if (props.metric === 'tokens') return formatTokenValue(item.totalTokens)
  return formatRequestCount(item.requestCount)
}
</script>

<template>
  <div class="grid grid-cols-1 gap-5 md:grid-cols-3">
    <section
      v-for="section in sections"
      :key="section.key"
      class="rounded-lg border border-[var(--theme-border-default)] p-4"
      style="background: var(--theme-surface-gradient)"
    >
      <div class="mb-2 flex items-center justify-between gap-2">
        <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
          <component :is="sectionIcon(section.key)" :size="14" class="shrink-0" aria-hidden="true" />
          {{ t(locale, section.titleKey) }}
        </h3>
        <span class="shrink-0 text-[10px] text-[var(--theme-text-quaternary)]">{{ metricLabel(metric) }}</span>
      </div>

      <div v-if="loading && !hasBreakdown" class="flex flex-col gap-2">
        <div v-for="i in 3" :key="i" class="h-[56px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"></div>
      </div>
      <div v-else-if="section.rows.length === 0" class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-3 py-6 text-center text-xs text-[var(--theme-text-tertiary)]">
        {{ t(locale, 'desktop.overview.contributionNoData') }}
      </div>
      <div v-else class="flex flex-col">
        <button
          v-for="row in section.rows"
          :key="row.item.id"
          type="button"
          class="flex flex-col gap-1 rounded-md px-1.5 py-2 text-left transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
          :title="t(locale, 'desktop.overview.contributionOpenHint')"
          @click="emit('open', section.key, row.item)"
        >
          <span class="flex items-center justify-between gap-2">
            <span class="truncate text-xs font-medium text-[var(--theme-text-primary)]">{{ displayLabel(row.item) }}</span>
            <span class="shrink-0 font-mono text-[11px] font-semibold text-[var(--theme-text-secondary)]">
              {{ formatContributionPrimary(row.item) }}
            </span>
          </span>
          <span class="flex items-center gap-2">
            <span class="h-1 flex-1 overflow-hidden rounded-full bg-[var(--theme-text-primary)]/10">
              <span
                class="block h-full rounded-full transition-[width] duration-300"
                :style="{ width: `${row.percent}%`, backgroundColor: row.item.color || 'var(--theme-accent-primary)' }"
              ></span>
            </span>
            <span class="w-9 shrink-0 text-right text-[10px] text-[var(--theme-text-quaternary)]">{{ formatRate(row.percent) }}%</span>
          </span>
          <span class="text-[10px] text-[var(--theme-text-quaternary)]">
            {{ t(locale, 'desktop.overview.contributionRequests', { count: formatRequestCount(row.item.requestCount) }) }}
          </span>
        </button>
      </div>
    </section>
  </div>
</template>
