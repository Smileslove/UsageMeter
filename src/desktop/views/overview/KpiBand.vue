<script setup lang="ts">
/**
 * 概览页六项指标：按内容区宽度排列紧凑卡片，加载骨架保持相同尺寸。
 * 数据由父页面经 useOverviewKpis 组装后传入；指标切换统一由趋势图控件负责。
 */
import type { PropType } from 'vue'
import { t } from '../../../i18n'
import type { KpiItem } from '../../composables/useOverviewKpis'

defineProps({
  kpis: { type: Array as PropType<KpiItem[]>, required: true },
  locale: { type: String, required: true }
})

function iconToneClass(key: string): string {
  if (key === 'requests' || key === 'successRate') return 'kpi-tone--mint'
  if (key === 'cost') return 'kpi-tone--amber'
  if (key === 'cache' || key === 'avgSpeed') return 'kpi-tone--violet'
  return 'kpi-tone--blue'
}
</script>

<template>
  <div
    class="overview-kpi-grid"
    role="group"
    :aria-label="t(locale, 'desktop.overview.kpiBand')"
  >
    <div v-if="kpis.length" class="overview-kpi-items">
      <div
        v-for="kpi in kpis"
        :key="kpi.key"
        class="overview-kpi-card"
        :class="iconToneClass(kpi.key)"
      >
        <div class="kpi-heading">
          <component :is="kpi.icon" :size="17" :stroke-width="1.8" class="shrink-0" aria-hidden="true" />
          <span class="truncate" :title="t(locale, kpi.labelKey)">{{ t(locale, kpi.labelKey) }}</span>
        </div>
        <div class="kpi-value" :title="kpi.primary">{{ kpi.primary }}</div>
        <div class="kpi-detail" :title="kpi.secondaryTitleKey ? t(locale, kpi.secondaryTitleKey) : undefined">
          <div v-for="(detail, index) in kpi.details" :key="index" class="kpi-detail-row">
            <span class="kpi-detail-dot" aria-hidden="true"></span>
            <span class="kpi-detail-label" :title="detail.labelKey ? t(locale, detail.labelKey) : detail.label">
              {{ detail.labelKey ? t(locale, detail.labelKey) : detail.label }}
            </span>
            <span class="kpi-detail-value" :title="detail.value">{{ detail.value }}</span>
          </div>
          <span v-if="kpi.coverageTag" class="kpi-coverage">
            {{ t(locale, 'desktop.overview.kpiCoverageOnly') }}
          </span>
        </div>
      </div>
    </div>
    <div v-else class="overview-kpi-items" aria-busy="true">
      <div v-for="i in 6" :key="i" class="overview-kpi-card kpi-skeleton animate-pulse">
        <div class="h-3 w-16 rounded bg-[var(--theme-text-primary)]/10"></div>
        <div class="h-6 w-24 rounded bg-[var(--theme-text-primary)]/10"></div>
        <div class="kpi-detail space-y-1.5">
          <div v-for="row in 2" :key="row" class="flex justify-between gap-2">
            <div class="h-3 w-10 rounded bg-[var(--theme-text-primary)]/10"></div>
            <div class="h-3 w-12 rounded bg-[var(--theme-text-primary)]/10"></div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.overview-kpi-grid {
  container: overview-kpis / inline-size;
}

.overview-kpi-items {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.overview-kpi-card {
  --kpi-tone: var(--theme-accent-primary);
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
  min-height: 130px;
  padding: 10px 12px;
  border: 1px solid color-mix(in srgb, var(--kpi-tone) 22%, var(--theme-border-default));
  border-radius: 16px;
  background:
    linear-gradient(135deg, transparent 25%, color-mix(in srgb, var(--kpi-tone) 7%, transparent)),
    var(--theme-bg-elevated);
  box-shadow: 0 3px 10px color-mix(in srgb, var(--theme-text-primary) 3%, transparent);
}

.kpi-heading {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  color: var(--kpi-tone);
  font-size: 12px;
  font-weight: 600;
  line-height: 18px;
}

.kpi-value {
  overflow: hidden;
  color: var(--theme-text-primary);
  font-family: var(--font-mono);
  font-size: clamp(22px, 2cqi, 26px);
  font-weight: 650;
  line-height: 1.15;
  letter-spacing: -0.045em;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.kpi-detail {
  margin-top: auto;
  min-height: 47px;
  padding-top: 8px;
  border-top: 1px solid color-mix(in srgb, var(--kpi-tone) 18%, transparent);
  color: var(--theme-text-secondary);
  font-size: 11px;
  line-height: 16px;
}

.kpi-detail-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  padding: 2px 0;
}

.kpi-detail-dot {
  width: 5px;
  height: 5px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--kpi-tone);
  opacity: 0.55;
}

.kpi-detail-label {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.kpi-detail-value {
  flex-shrink: 0;
  max-width: 65%;
  overflow: hidden;
  color: var(--kpi-tone);
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.kpi-coverage {
  display: inline-block;
  padding: 0 5px;
  border-radius: 4px;
  color: var(--theme-status-warning-fg);
  background: var(--theme-status-warning-bg);
}

.kpi-tone--mint {
  --kpi-tone: var(--theme-chart-requests);
}

.kpi-tone--amber {
  --kpi-tone: var(--theme-chart-cost);
}

.kpi-tone--violet {
  --kpi-tone: var(--theme-chart-series-3);
}

@container overview-kpis (min-width: 600px) {
  .overview-kpi-items {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}

@container overview-kpis (min-width: 1000px) {
  .overview-kpi-items {
    grid-template-columns: repeat(6, minmax(0, 1fr));
  }
}

@media (prefers-reduced-motion: reduce) {
  .kpi-skeleton {
    animation: none;
  }
}
</style>
