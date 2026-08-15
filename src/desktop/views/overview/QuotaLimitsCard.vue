<script setup lang="ts">
/**
 * 概览页"额度与生存"卡片（设计 6.6）：按来源纵向列表。
 * 行数据由父页面经 useQuotaLimits 组装后传入；可点击行通知父页面进入分析页。
 */
import type { PropType, Component } from 'vue'
import { CheckCircle2, CircleAlert, CircleHelp, ShieldCheck, TriangleAlert } from 'lucide-vue-next'
import { t } from '../../../i18n'
import { formatRate } from '../../../utils/format'
import type { LimitRow, LimitState } from '../../composables/useQuotaLimits'

defineProps({
  rows: { type: Array as PropType<LimitRow[]>, required: true },
  loading: { type: Boolean, required: true },
  locale: { type: String, required: true }
})

const emit = defineEmits<{
  openAnalytics: []
}>()

function stateToneClass(state: LimitState): string {
  if (state === 'safe') return 'text-emerald-600 dark:text-emerald-400'
  if (state === 'attention') return 'text-amber-600 dark:text-amber-400'
  if (state === 'danger') return 'text-red-600 dark:text-red-400'
  return 'text-[var(--theme-text-tertiary)]'
}

function barClass(state: LimitState): string {
  if (state === 'safe') return 'bg-emerald-500 dark:bg-emerald-400'
  if (state === 'attention') return 'bg-amber-500 dark:bg-amber-400'
  if (state === 'danger') return 'bg-red-500 dark:bg-red-400'
  return 'bg-[var(--theme-text-quaternary)]'
}

function stateIcon(state: LimitState): Component {
  if (state === 'safe') return CheckCircle2
  if (state === 'attention') return TriangleAlert
  if (state === 'danger') return CircleAlert
  return CircleHelp
}
</script>

<template>
  <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
    <h3 class="mb-3 flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
      <ShieldCheck :size="14" class="shrink-0" aria-hidden="true" />
      {{ t(locale, 'desktop.overview.limitTitle') }}
    </h3>

    <div v-if="loading" class="flex flex-col gap-2">
      <div v-for="i in 3" :key="i" class="h-[84px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"></div>
    </div>
    <div
      v-else-if="rows.length === 0"
      class="grid place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center text-xs leading-5 text-[var(--theme-text-tertiary)]"
    >
      {{ t(locale, 'desktop.overview.limitEmpty') }}
    </div>
    <ul v-else class="flex flex-col gap-2">
      <li
        v-for="row in rows"
        :key="row.key"
        :class="[
          'rounded-lg border border-[var(--theme-border-default)] p-3',
          row.clickable ? 'cursor-pointer transition-colors duration-150 hover:bg-[var(--theme-bg-hover)]' : ''
        ]"
        :title="row.clickable ? t(locale, 'desktop.overview.limitOpenHint') : undefined"
        @click="row.clickable && emit('openAnalytics')"
      >
        <div class="flex items-center justify-between gap-2">
          <div class="flex min-w-0 items-center gap-1.5">
            <component :is="stateIcon(row.state)" :size="14" class="shrink-0" :class="stateToneClass(row.state)" aria-hidden="true" />
            <span class="truncate text-xs font-semibold text-[var(--theme-text-primary)]">{{ row.label }}</span>
            <span
              class="shrink-0 rounded-full border border-[var(--theme-border-default)] px-1.5 py-px text-[10px] text-[var(--theme-text-tertiary)]"
            >
              {{ row.windowLabel }}
            </span>
          </div>
          <span class="shrink-0 text-[11px] font-medium" :class="stateToneClass(row.state)">
            {{ t(locale, row.conclusionKey) }}
          </span>
        </div>
        <div class="mt-2 flex items-center gap-2">
          <div class="h-1.5 flex-1 overflow-hidden rounded-full bg-[var(--theme-text-primary)]/10">
            <div
              class="h-full rounded-full transition-[width] duration-300"
              :class="barClass(row.state)"
              :style="{ width: `${row.barPct}%` }"
            ></div>
          </div>
          <span class="w-10 shrink-0 text-right font-mono text-[11px] font-semibold text-[var(--theme-text-secondary)]">
            {{ row.usedPct == null ? '--' : `${formatRate(row.usedPct)}%` }}
          </span>
        </div>
        <div class="mt-1.5 flex items-center justify-between gap-2 text-[10px] text-[var(--theme-text-quaternary)]">
          <span>{{ t(locale, 'desktop.overview.limitResetIn', { time: row.resetText }) }}</span>
          <span v-if="row.confidenceKey">
            {{ t(locale, 'desktop.overview.limitConfidence') }} {{ t(locale, row.confidenceKey) }}
          </span>
        </div>
      </li>
    </ul>
  </div>
</template>
