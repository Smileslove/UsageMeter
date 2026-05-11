<script setup lang="ts">
import { computed, onMounted, watch } from 'vue'
import { Activity, Boxes, CircleDollarSign, HelpCircle, Layers3, LayoutGrid } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenValue } from '../../utils/format'
import { TOOL_LOBE_ICONS } from '../../iconConfig'
import LobeIcon from '../LobeIcon.vue'
import type { OverviewBreakdownItem } from '../../types'

const store = useMonitorStore()

const breakdown = computed(() => store.overviewBreakdown)
const locale = computed(() => store.settings.locale)
const hasCost = computed(() => breakdown.value?.capability.hasCost ?? false)

const sourceItems = computed(() => breakdown.value?.sourceRanking ?? [])
const toolItems = computed(() => breakdown.value?.toolRanking ?? [])
const modelItems = computed(() => breakdown.value?.modelRanking ?? [])

const showSourceSection = computed(() => store.isProxyMode && sourceItems.value.length > 0)
const showSourceHint = computed(() => !store.isProxyMode)
const hasAnyItems = computed(() => sourceItems.value.length + toolItems.value.length + modelItems.value.length > 0)

watch(
  () => ({
    window: store.settings.summaryWindow,
    source: store.settings.sourceAware.activeSourceFilter,
    tool: store.settings.clientTools.activeToolFilter,
    dataSource: store.settings.dataSource,
    snapshotEpoch: store.lastUpdatedEpoch
  }),
  ({ window }) => {
    if (window) {
      store.fetchOverviewBreakdown(window)
    }
  },
  { immediate: false }
)

onMounted(() => {
  if (store.settings.summaryWindow) {
    store.fetchOverviewBreakdown(store.settings.summaryWindow)
  }
})

function primaryValue(item: OverviewBreakdownItem): string {
  if (hasCost.value) return formatCost(item.cost, store.settings.currency)
  if (item.totalTokens > 0) return formatTokenValue(item.totalTokens)
  return formatRequestCount(item.requestCount)
}

function displayLabel(item: OverviewBreakdownItem): string {
  if (item.label === '__unknown__') return t(locale.value, 'sources.unknown')
  if (item.label === '__official_api__') return t(locale.value, 'sources.officialAnthropic')
  return item.label
}

function toolIcon(item: OverviewBreakdownItem): string | null {
  return item.icon || TOOL_LOBE_ICONS[item.id] || null
}

function secondaryValue(item: OverviewBreakdownItem): string {
  return `${formatRequestCount(item.requestCount)} ${t(locale.value, 'overview.requestsShort')}`
}

function tokenValue(item: OverviewBreakdownItem): string {
  return `${formatTokenValue(item.totalTokens)} ${t(locale.value, 'overview.tokensShort')}`
}

function statusValue(item: OverviewBreakdownItem): string {
  const errors = item.errorRequests ?? 0
  if (errors > 0) return `${formatRequestCount(errors)} ${t(locale.value, 'overview.errorsShort')}`
  if (item.avgTokensPerSecond && item.avgTokensPerSecond > 0) {
    return `${item.avgTokensPerSecond.toFixed(1)} t/s`
  }
  return ''
}

function barWidth(item: OverviewBreakdownItem): string {
  return `${Math.max(3, Math.min(100, item.percent || 0))}%`
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
      <span class="text-[10px] text-gray-400 dark:text-gray-500">
        {{ hasCost ? t(locale, 'overview.costPrimary') : t(locale, 'overview.tokenPrimary') }}
      </span>
    </div>

    <div v-if="store.overviewBreakdownLoading && !breakdown" class="overview-empty">
      {{ t(locale, 'common.syncing') }}
    </div>

    <div v-else-if="!hasAnyItems" class="overview-empty">
      {{ t(locale, 'overview.noBreakdown') }}
    </div>

    <template v-else>
      <section v-if="showSourceSection" class="rank-card">
        <div class="rank-card-header">
          <div class="flex items-center gap-1.5">
            <Layers3 class="h-3.5 w-3.5 text-sky-500 dark:text-sky-300" />
            <span>{{ t(locale, 'sources.ranking') }}</span>
          </div>
        </div>

        <div
          v-for="item in sectionLimit(sourceItems)"
          :key="item.id"
          class="rank-row"
        >
          <div class="rank-icon">
            <LobeIcon v-if="item.icon" :slug="item.icon" :size="16" @error="() => {}" />
            <span v-else class="h-2.5 w-2.5 rounded-full" :style="{ backgroundColor: item.color || '#9CA3AF' }"></span>
          </div>
          <div class="rank-main">
            <div class="rank-line">
              <span class="rank-label">{{ displayLabel(item) }}</span>
              <span class="rank-value">{{ primaryValue(item) }}</span>
            </div>
            <div class="rank-meta">
              <div class="rank-bar"><span :style="{ width: barWidth(item), backgroundColor: item.color || '#38BDF8' }"></span></div>
              <span>{{ secondaryValue(item) }}</span>
              <span>{{ tokenValue(item) }}</span>
              <span v-if="statusValue(item)">{{ statusValue(item) }}</span>
            </div>
          </div>
        </div>
      </section>

      <section v-else-if="showSourceHint" class="rank-card rank-hint">
        <HelpCircle class="h-3.5 w-3.5 text-gray-400" />
        <span>{{ t(locale, 'overview.proxySourceHint') }}</span>
      </section>

      <section v-if="toolItems.length > 1 || store.settings.clientTools.activeToolFilter" class="rank-card">
        <div class="rank-card-header">
          <div class="flex items-center gap-1.5">
            <LayoutGrid class="h-3.5 w-3.5 text-emerald-500 dark:text-emerald-300" />
            <span>{{ t(locale, 'tools.ranking') }}</span>
          </div>
        </div>

        <div
          v-for="item in sectionLimit(toolItems)"
          :key="item.id"
          class="rank-row"
        >
          <div class="rank-icon">
            <LobeIcon v-if="toolIcon(item)" :slug="toolIcon(item)!" :size="16" @error="() => {}" />
            <Boxes v-else class="h-3.5 w-3.5 text-gray-400" />
          </div>
          <div class="rank-main">
            <div class="rank-line">
              <span class="rank-label">{{ displayLabel(item) }}</span>
              <span class="rank-value">{{ primaryValue(item) }}</span>
            </div>
            <div class="rank-meta">
              <div class="rank-bar"><span :style="{ width: barWidth(item) }"></span></div>
              <span>{{ secondaryValue(item) }}</span>
              <span>{{ tokenValue(item) }}</span>
              <span v-if="statusValue(item)">{{ statusValue(item) }}</span>
            </div>
          </div>
        </div>
      </section>

      <section v-if="modelItems.length > 0" class="rank-card">
        <div class="rank-card-header">
          <div class="flex items-center gap-1.5">
            <CircleDollarSign class="h-3.5 w-3.5 text-amber-500 dark:text-amber-300" />
            <span>{{ t(locale, 'overview.modelRanking') }}</span>
          </div>
        </div>

        <div
          v-for="item in sectionLimit(modelItems, 5)"
          :key="item.id"
          class="rank-row static"
        >
          <div class="rank-index">{{ modelItems.indexOf(item) + 1 }}</div>
          <div class="rank-main">
            <div class="rank-line">
              <span class="rank-label">{{ displayLabel(item) }}</span>
              <span class="rank-value">{{ primaryValue(item) }}</span>
            </div>
            <div class="rank-meta">
              <div class="rank-bar"><span :style="{ width: barWidth(item) }"></span></div>
              <span>{{ secondaryValue(item) }}</span>
              <span>{{ tokenValue(item) }}</span>
              <span v-if="statusValue(item)">{{ statusValue(item) }}</span>
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
  border: 1px solid rgb(243 244 246);
  border-radius: 1rem;
  background: rgb(255 255 255);
  padding: 1rem;
  text-align: center;
  font-size: 12px;
  color: rgb(156 163 175);
}

.rank-card {
  overflow: hidden;
  border: 1px solid rgb(243 244 246);
  border-radius: 1rem;
  background: rgb(255 255 255);
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.025);
}

.rank-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  padding: 0.55rem 0.7rem 0.35rem;
  font-size: 12px;
  font-weight: 700;
  color: rgb(55 65 81);
}

.rank-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 0.55rem;
  border-top: 1px solid rgb(249 250 251);
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
  background: rgb(249 250 251);
  font-size: 10px;
  font-weight: 700;
  color: rgb(156 163 175);
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
  color: rgb(31 41 55);
}

.rank-value {
  flex-shrink: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
  font-size: 11px;
  font-weight: 700;
  color: rgb(17 24 39);
}

.rank-meta {
  margin-top: 0.18rem;
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 0.4rem;
  overflow: hidden;
  font-size: 10px;
  color: rgb(156 163 175);
  white-space: nowrap;
}

.rank-bar {
  height: 0.25rem;
  width: 2.6rem;
  flex-shrink: 0;
  overflow: hidden;
  border-radius: 999px;
  background: rgb(243 244 246);
}

.rank-bar span {
  display: block;
  height: 100%;
  border-radius: inherit;
  background: rgb(16 185 129);
}

.rank-hint {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.75rem;
  font-size: 11px;
  color: rgb(156 163 175);
}

:global(html.dark) .overview-empty,
:global(html.dark) .rank-card {
  border-color: rgb(38 38 38);
  background: rgb(28 28 30);
}

:global(html.dark) .rank-card-header,
:global(html.dark) .rank-label,
:global(html.dark) .rank-value {
  color: rgb(243 244 246);
}

:global(html.dark) .rank-row {
  border-top-color: rgb(38 38 38 / 0.8);
}

:global(html.dark) .rank-icon,
:global(html.dark) .rank-index {
  background: rgb(38 38 38);
}

:global(html.dark) .rank-bar {
  background: rgb(64 64 64);
}

</style>
