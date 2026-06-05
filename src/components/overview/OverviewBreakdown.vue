<script setup lang="ts">
import { computed, ref } from 'vue'
import { Activity, Boxes, CircleDollarSign, HelpCircle, Layers3, LayoutGrid } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenValue } from '../../utils/format'
import { resolveToolLobeIcon } from '../../iconConfig'
import LobeIcon from '../LobeIcon.vue'
import type { OverviewBreakdownItem } from '../../types'

const store = useMonitorStore()

type BreakdownSortMetric = 'cost' | 'requests' | 'tokens'
type RankMetaTone = 'requests' | 'tokens' | 'cost' | 'rate' | 'error' | 'muted'
type RankMetaItem = {
  label: string
  value: string
  tone: RankMetaTone
}

const breakdown = computed(() => store.overviewBreakdown)
const locale = computed(() => store.settings.locale)
const hasCost = computed(() => breakdown.value?.capability.hasCost ?? false)
const selectedSortMetric = ref<BreakdownSortMetric>('cost')

const sortOptions = computed<Array<{ value: BreakdownSortMetric; label: string }>>(() => {
  const options: Array<{ value: BreakdownSortMetric; label: string }> = []
  if (hasCost.value) {
    options.push({ value: 'cost', label: t(locale.value, 'overview.sortCost') })
  }
  options.push(
    { value: 'requests', label: t(locale.value, 'overview.sortRequests') },
    { value: 'tokens', label: t(locale.value, 'overview.sortTokens') }
  )
  return options
})

const effectiveSortMetric = computed<BreakdownSortMetric>(() => {
  if (selectedSortMetric.value === 'cost' && !hasCost.value) return 'tokens'
  return selectedSortMetric.value
})

const sourceItems = computed(() => sortItems(breakdown.value?.sourceRanking ?? []))
const toolItems = computed(() => sortItems(breakdown.value?.toolRanking ?? []))
const modelItems = computed(() => sortItems(breakdown.value?.modelRanking ?? []))

const showSourceSection = computed(() => sourceItems.value.length > 0)
const showSourceHint = computed(() => !showSourceSection.value && hasAnyItems.value)
const hasAnyItems = computed(() => sourceItems.value.length + toolItems.value.length + modelItems.value.length > 0)

function primaryValue(item: OverviewBreakdownItem): string {
  if (effectiveSortMetric.value === 'cost') return formatCost(item.cost, store.settings.currency)
  if (effectiveSortMetric.value === 'requests') return formatRequestCount(item.requestCount)
  return formatTokenValue(item.totalTokens)
}

function metricValue(item: OverviewBreakdownItem, metric: BreakdownSortMetric): number {
  if (metric === 'cost') return item.cost
  if (metric === 'requests') return item.requestCount
  return item.totalTokens
}

function sortItems(items: OverviewBreakdownItem[]): OverviewBreakdownItem[] {
  const metric = effectiveSortMetric.value
  return [...items].sort((a, b) => {
    const primary = metricValue(b, metric) - metricValue(a, metric)
    if (primary !== 0) return primary

    return b.cost - a.cost
      || b.totalTokens - a.totalTokens
      || b.requestCount - a.requestCount
      || displayLabel(a).localeCompare(displayLabel(b))
  })
}

function displayLabel(item: OverviewBreakdownItem): string {
  if (item.label === '__unknown__') return t(locale.value, 'sources.unknown')
  if (item.label === '__official_api__') return t(locale.value, 'sources.officialAnthropic')
  return item.label
}

function toolIcon(item: OverviewBreakdownItem): string | null {
  return resolveToolLobeIcon(item.id, item.icon)
}

function requestMeta(item: OverviewBreakdownItem): RankMetaItem {
  return {
    label: t(locale.value, 'overview.requestsShort'),
    value: formatRequestCount(item.requestCount),
    tone: 'requests'
  }
}

function tokenMeta(item: OverviewBreakdownItem): RankMetaItem {
  return {
    label: t(locale.value, 'overview.tokensShort'),
    value: formatTokenValue(item.totalTokens),
    tone: 'tokens'
  }
}

function costMeta(item: OverviewBreakdownItem): RankMetaItem {
  return {
    label: t(locale.value, 'overview.sortCost'),
    value: hasCost.value ? formatCost(item.cost, store.settings.currency) : '-',
    tone: hasCost.value ? 'cost' : 'muted'
  }
}

function statusMeta(item: OverviewBreakdownItem): RankMetaItem {
  const errors = item.errorRequests
  if (errors == null) {
    return {
      label: t(locale.value, 'overview.errorsShort'),
      value: '—',
      tone: 'muted'
    }
  }
  if (errors > 0) {
    return {
      label: t(locale.value, 'overview.errorsShort'),
      value: formatRequestCount(errors),
      tone: 'error'
    }
  }
  if (item.avgTokensPerSecond && item.avgTokensPerSecond > 0) {
    return {
      label: t(locale.value, 'overview.avgRateShort'),
      value: `${item.avgTokensPerSecond.toFixed(1)} t/s`,
      tone: 'rate'
    }
  }
  return {
    label: t(locale.value, 'overview.avgRateShort'),
    value: '-',
    tone: 'muted'
  }
}

function metaItems(item: OverviewBreakdownItem): RankMetaItem[] {
  const items: RankMetaItem[] = [
    requestMeta(item),
    tokenMeta(item),
    costMeta(item),
    statusMeta(item)
  ]
  return items.filter((meta) => meta.tone !== effectiveSortMetric.value)
}

function sectionLimit(items: OverviewBreakdownItem[], max = 4): OverviewBreakdownItem[] {
  return items.slice(0, max)
}
</script>

<template>
  <div class="overview-breakdown">
    <div class="flex items-center justify-between px-1">
      <div class="flex items-center gap-1.5">
        <Activity class="h-3.5 w-3.5 text-gray-400 dark:text-gray-500" />
        <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider">
          {{ t(locale, 'overview.attribution') }}
        </h3>
      </div>
      <div class="sort-control" :aria-label="t(locale, 'overview.sortBy')">
        <button
          v-for="option in sortOptions"
          :key="option.value"
          type="button"
          :class="effectiveSortMetric === option.value ? 'active' : ''"
          @click="selectedSortMetric = option.value"
        >
          {{ option.label }}
        </button>
      </div>
    </div>

    <div v-if="store.overviewBreakdownLoading && !breakdown" class="overview-empty dark:!border-white/10 dark:!bg-[#15161A] dark:!text-gray-500">
      {{ t(locale, 'common.syncing') }}
    </div>

    <div v-else-if="!hasAnyItems" class="overview-empty dark:!border-white/10 dark:!bg-[#15161A] dark:!text-gray-500">
      {{ t(locale, 'overview.noBreakdown') }}
    </div>

    <template v-else>
      <section v-if="showSourceSection" class="rank-card dark:!border-white/10 dark:!bg-[#15161A]">
        <div class="rank-card-header dark:!text-gray-200">
          <div class="flex items-center gap-1.5">
            <Layers3 class="h-3.5 w-3.5 text-sky-500 dark:text-sky-300" />
            <span>{{ t(locale, 'sources.ranking') }}</span>
          </div>
        </div>

        <div
          v-for="item in sectionLimit(sourceItems)"
          :key="item.id"
          class="rank-row dark:!border-white/10"
        >
          <div class="rank-icon dark:!bg-white/[0.08] dark:!text-gray-400">
            <LobeIcon v-if="item.icon" :slug="item.icon" :size="16" @error="() => {}" />
            <span v-else class="h-2.5 w-2.5 rounded-full" :style="{ backgroundColor: item.color || '#9CA3AF' }"></span>
          </div>
          <div class="rank-main">
            <div class="rank-line">
              <span class="rank-label dark:!text-gray-200">{{ displayLabel(item) }}</span>
              <span class="rank-value dark:!text-gray-100">{{ primaryValue(item) }}</span>
            </div>
            <div class="rank-meta">
              <span
                v-for="meta in metaItems(item)"
                :key="`${item.id}-${meta.tone}`"
                class="rank-meta-pill dark:!border-white/35"
                :class="`rank-meta-pill-${meta.tone}`"
              >
                <span class="rank-meta-label dark:!text-gray-400">{{ meta.label }}</span>
                <span class="rank-meta-value dark:!text-gray-300">{{ meta.value }}</span>
              </span>
            </div>
          </div>
        </div>
      </section>

      <section v-else-if="showSourceHint" class="rank-card rank-hint dark:!border-white/10 dark:!bg-[#15161A]">
        <HelpCircle class="h-3.5 w-3.5 text-gray-400" />
        <span>{{ t(locale, 'overview.proxySourceHint') }}</span>
      </section>

      <section v-if="toolItems.length > 1 || store.settings.clientTools.activeToolFilter" class="rank-card dark:!border-white/10 dark:!bg-[#15161A]">
        <div class="rank-card-header dark:!text-gray-200">
          <div class="flex items-center gap-1.5">
            <LayoutGrid class="h-3.5 w-3.5 text-emerald-500 dark:text-emerald-300" />
            <span>{{ t(locale, 'tools.ranking') }}</span>
          </div>
        </div>

        <div
          v-for="item in sectionLimit(toolItems)"
          :key="item.id"
          class="rank-row dark:!border-white/10"
        >
          <div class="rank-icon dark:!bg-white/[0.08] dark:!text-gray-400">
            <LobeIcon v-if="toolIcon(item)" :slug="toolIcon(item)!" :size="16" @error="() => {}" />
            <Boxes v-else class="h-3.5 w-3.5 text-gray-400" />
          </div>
          <div class="rank-main">
            <div class="rank-line">
              <span class="rank-label dark:!text-gray-200">{{ displayLabel(item) }}</span>
              <span class="rank-value dark:!text-gray-100">{{ primaryValue(item) }}</span>
            </div>
            <div class="rank-meta">
              <span
                v-for="meta in metaItems(item)"
                :key="`${item.id}-${meta.tone}`"
                class="rank-meta-pill dark:!border-white/35"
                :class="`rank-meta-pill-${meta.tone}`"
              >
                <span class="rank-meta-label dark:!text-gray-400">{{ meta.label }}</span>
                <span class="rank-meta-value dark:!text-gray-300">{{ meta.value }}</span>
              </span>
            </div>
          </div>
        </div>
      </section>

      <section v-if="modelItems.length > 0" class="rank-card dark:!border-white/10 dark:!bg-[#15161A]">
        <div class="rank-card-header dark:!text-gray-200">
          <div class="flex items-center gap-1.5">
            <CircleDollarSign class="h-3.5 w-3.5 text-amber-500 dark:text-amber-300" />
            <span>{{ t(locale, 'overview.modelRanking') }}</span>
          </div>
        </div>

        <div
          v-for="item in sectionLimit(modelItems, 5)"
          :key="item.id"
          class="rank-row static dark:!border-white/10"
        >
          <div class="rank-index dark:!bg-white/[0.08] dark:!text-gray-400">{{ modelItems.indexOf(item) + 1 }}</div>
          <div class="rank-main">
            <div class="rank-line">
              <span class="rank-label dark:!text-gray-200">{{ displayLabel(item) }}</span>
              <span class="rank-value dark:!text-gray-100">{{ primaryValue(item) }}</span>
            </div>
            <div class="rank-meta">
              <span
                v-for="meta in metaItems(item)"
                :key="`${item.id}-${meta.tone}`"
                class="rank-meta-pill dark:!border-white/35"
                :class="`rank-meta-pill-${meta.tone}`"
              >
                <span class="rank-meta-label dark:!text-gray-400">{{ meta.label }}</span>
                <span class="rank-meta-value dark:!text-gray-300">{{ meta.value }}</span>
              </span>
            </div>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.overview-breakdown {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.overview-empty {
  border: 1px solid var(--theme-border-default);
  border-radius: 1rem;
  background: var(--theme-surface-gradient);
  padding: 1rem;
  text-align: center;
  font-size: 12px;
  color: var(--theme-text-tertiary);
}

.sort-control {
  display: inline-flex;
  flex-shrink: 0;
  align-items: center;
  gap: 0.15rem;
  border: 1px solid color-mix(in srgb, var(--theme-text-primary) 7%, transparent);
  border-radius: 999px;
  background: color-mix(in srgb, var(--theme-text-primary) 9%, transparent);
  padding: 0.12rem;
}

:root[data-appearance='dark'] .sort-control {
  border-color: var(--theme-border-default);
  background: var(--theme-dark-item-bg);
}

.sort-control button {
  min-width: 2.1rem;
  border: 0;
  border-radius: 999px;
  background: transparent;
  padding: 0.16rem 0.42rem;
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
  color: var(--theme-text-tertiary);
  transition: background-color 0.15s ease, color 0.15s ease;
}

.sort-control button:hover {
  color: var(--theme-text-primary);
}

.sort-control button.active {
  background: var(--theme-accent-primary);
  color: var(--theme-accent-contrast);
}

.rank-card {
  overflow: hidden;
  border: 1px solid var(--theme-border-default);
  border-radius: 1rem;
  background: var(--theme-surface-gradient);
  box-shadow: var(--theme-shadow-inline);
}

.rank-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  padding: 0.55rem 0.7rem 0.35rem;
  font-size: 12px;
  font-weight: 700;
  color: var(--theme-text-secondary);
}

.rank-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 0.55rem;
  border-top: 1px solid color-mix(in srgb, var(--theme-border-default) 72%, transparent);
  padding: 0.48rem 0.7rem;
}

.rank-icon,
.rank-index {
  display: flex;
  height: 1.45rem;
  width: 1.45rem;
  flex-shrink: 0;
  align-items: center;
  justify-content: center;
  border-radius: 0.55rem;
  background: var(--theme-surface-muted-gradient);
  font-size: 10px;
  font-weight: 700;
  color: var(--theme-text-tertiary);
}

.rank-main {
  min-width: 0;
  flex: 1 1 0%;
}

.rank-line {
  display: flex;
  min-width: 0;
  align-items: baseline;
  justify-content: space-between;
  gap: 0.5rem;
}

.rank-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  font-weight: 600;
  color: var(--theme-text-primary);
}

.rank-value {
  flex-shrink: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
  font-size: 11px;
  font-weight: 700;
  color: var(--theme-text-primary);
}

.rank-meta {
  margin-top: 0.18rem;
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  min-width: 0;
  align-items: center;
  gap: 0;
  overflow: hidden;
}

.rank-meta-pill {
  display: flex;
  min-width: 0;
  align-items: baseline;
  gap: 0.18rem;
  overflow: hidden;
  border-left: 1px solid color-mix(in srgb, var(--theme-border-default) 78%, transparent);
  padding: 0 0.38rem;
  text-align: left;
}

.rank-meta-pill:first-child {
  border-left: 0;
  padding-left: 0;
}

.rank-meta-pill:last-child {
  padding-right: 0;
}

.rank-meta-label,
.rank-meta-value {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.rank-meta-label {
  font-size: 9px;
  font-weight: 650;
  line-height: 1;
  color: var(--theme-text-tertiary);
}

.rank-meta-value {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
  color: var(--theme-text-secondary);
}

.rank-meta-pill-error .rank-meta-label,
.rank-meta-pill-error .rank-meta-value {
  color: var(--theme-status-danger-fg);
}

.rank-meta-pill-muted .rank-meta-value {
  color: var(--theme-text-quaternary);
}

.rank-hint {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.75rem;
  font-size: 11px;
  color: var(--theme-text-tertiary);
}

</style>
