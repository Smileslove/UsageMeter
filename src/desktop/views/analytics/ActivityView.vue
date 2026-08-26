<script setup lang="ts">
/**
 * 分析页 - 活跃度视图（设计 7.6）：月历 + 年度贡献图 + 小时热力图占位。
 * 月份状态（currentMonth / selectedDate）与对应的数据拉取编排由父级 DesktopAnalytics
 * 持有（父级 watch 月份/指标变化并触发 fetchMonthActivity/fetchYearActivity），
 * 本组件通过 props 接收月份状态、emits 通知月份切换与日期选中，渲染派生全部在内部。
 */
import { computed } from 'vue'
import { BarChart3, CalendarDays, ChevronLeft, ChevronRight } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { useDesktopAnalyticsStore } from '../../stores/desktopAnalytics'
import { formatMetric, metricLabel } from '../../composables/useTrendChart'
import { t } from '../../../i18n'
import { formatCost, formatRequestCount, formatTokenValue } from '../../../utils/format'
import {
  formatLocalDate,
  getMonthDayCount,
  intensityClass,
  makeEmptyDay,
  METRICS,
  valueOf
} from '../../../components/statistics/activityUtils'
import type { DayActivity } from '../../../types'

const props = defineProps<{
  /** 当前浏览月份（父级持有，翻月由父级更新并触发数据拉取）。 */
  currentMonth: Date
  /** 选中的日期（YYYY-MM-DD），用于月历高亮。 */
  selectedDate: string
}>()

const emit = defineEmits<{
  /** 翻月请求（-1 上一月 / 1 下一月），父级更新 currentMonth 并清空选中日期。 */
  (e: 'move-month', delta: number): void
  /** 选中月历中的某一天。 */
  (e: 'select-day', date: string): void
}>()

const store = useMonitorStore()
const analytics = useDesktopAnalyticsStore()
const locale = computed(() => store.settings.locale)

const monthYear = computed(() => props.currentMonth.getFullYear())
const monthNumber = computed(() => props.currentMonth.getMonth() + 1)

function moveMonth(delta: number) {
  emit('move-month', delta)
}

function selectDay(day: DayActivity) {
  emit('select-day', day.date)
}

/** 月历内最大值（用于单元格强度归一化）。 */
function monthMaxValue(): number {
  const days = calendarCells.value
    .map(c => c.day)
    .filter((d): d is DayActivity => !!d)
  return Math.max(...days.map(d => valueOf(d, analytics.analyticsMetric)), 0)
}

/** 月历单元格强度（0-1）。 */
function monthCellRatio(day: DayActivity | null): number {
  if (!day) return 0
  const max = monthMaxValue()
  if (max <= 0) return 0
  return valueOf(day, analytics.analyticsMetric) / max
}

/** 月历单元格（复用 activityUtils 语义）。 */
const calendarCells = computed(() => {
  const activity = store.monthActivity
  const activeDays = activity && activity.year === monthYear.value && activity.month === monthNumber.value ? activity.days : []
  const dayMap = new Map(activeDays.map(d => [d.date, d]))
  const dayCount = getMonthDayCount(monthYear.value, monthNumber.value)
  const firstDay = new Date(monthYear.value, monthNumber.value - 1, 1).getDay()
  const leadingCount = firstDay === 0 ? 6 : firstDay - 1
  const cells: Array<{ key: string; day: DayActivity | null; dayNumber: string }> = Array.from(
    { length: leadingCount },
    (_, index) => ({ key: `blank-leading-${index}`, day: null, dayNumber: '' })
  )
  for (let day = 1; day <= dayCount; day += 1) {
    const date = `${monthYear.value}-${String(monthNumber.value).padStart(2, '0')}-${String(day).padStart(2, '0')}`
    cells.push({ key: date, day: dayMap.get(date) ?? makeEmptyDay(date), dayNumber: String(day) })
  }
  const trailingCount = Math.max(0, Math.ceil(cells.length / 7) * 7 - cells.length)
  return [
    ...cells,
    ...Array.from({ length: trailingCount }, (_, index) => ({
      key: `blank-trailing-${index}`,
      day: null,
      dayNumber: ''
    }))
  ]
})

const weekDayLabels = computed(() => [
  t(locale.value, 'statistics.weekMon'),
  t(locale.value, 'statistics.weekTue'),
  t(locale.value, 'statistics.weekWed'),
  t(locale.value, 'statistics.weekThu'),
  t(locale.value, 'statistics.weekFri'),
  t(locale.value, 'statistics.weekSat'),
  t(locale.value, 'statistics.weekSun')
])

/** 年度贡献单元格：53 周 × 7 天（可水平压缩，不出现页面级横向滚动，设计 7.6）。 */
const annualCells = computed(() => {
  const activity = store.yearActivity
  const days = activity && activity.year === monthYear.value ? activity.days : []
  const dayMap = new Map(days.map(d => [d.date, d]))
  const start = new Date(monthYear.value, 0, 1)
  const end = new Date(monthYear.value + 1, 0, 1)
  const leadingCount = start.getDay() === 0 ? 6 : start.getDay() - 1
  const cells: Array<{ key: string; day: DayActivity | null }> = Array.from({ length: leadingCount }, (_, i) => ({
    key: `year-leading-${i}`,
    day: null
  }))
  for (let time = start.getTime(); time < end.getTime(); time += 86400000) {
    const date = new Date(time)
    const key = formatLocalDate(date)
    cells.push({ key, day: dayMap.get(key) ?? makeEmptyDay(key) })
  }
  const trailingCount = Math.max(0, Math.ceil(cells.length / 7) * 7 - cells.length)
  return [
    ...cells,
    ...Array.from({ length: trailingCount }, (_, i) => ({ key: `year-trailing-${i}`, day: null }))
  ]
})

const annualMaxValue = computed(() => {
  const days = annualCells.value.map(c => c.day).filter((d): d is DayActivity => !!d)
  return Math.max(...days.map(d => valueOf(d, analytics.analyticsMetric)), 0)
})

function cellRatio(day: DayActivity): number {
  if (annualMaxValue.value <= 0) return 0
  return valueOf(day, analytics.analyticsMetric) / annualMaxValue.value
}

function dayTooltip(day: DayActivity): string {
  const v = valueOf(day, analytics.analyticsMetric)
  return `${day.date} · ${formatMetric(analytics.analyticsMetric, v)} · ${t(locale.value, 'desktop.analytics.activityRequests', { count: formatRequestCount(day.requestCount) })}`
}

function monthCalendarTooltip(day: DayActivity): string {
  const v = valueOf(day, analytics.analyticsMetric)
  const rows = [
    `${t(locale.value, 'desktop.analytics.totalsRequests')} ${formatRequestCount(day.requestCount)}`,
    `${t(locale.value, 'desktop.analytics.totalsTokens')} ${formatTokenValue(day.totalTokens)}`,
    `${t(locale.value, 'desktop.analytics.totalsCost')} ${formatCost(day.cost, store.settings.currency)}`
  ].join(' · ')
  return `${day.date} · ${metricLabel(analytics.analyticsMetric)} ${formatMetric(analytics.analyticsMetric, v)} · ${rows}`
}
</script>

<template>
  <div class="flex flex-col gap-5">
    <!-- 月历 + 年度贡献图 -->
    <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
      <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h3 class="flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
          <CalendarDays :size="14" class="shrink-0" aria-hidden="true" />
          {{ t(locale, 'desktop.analytics.monthCalendar') }}
          <span class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">
            {{ monthYear }}-{{ String(monthNumber).padStart(2, '0') }}
          </span>
        </h3>
        <div class="flex items-center gap-1.5">
          <div
            class="flex items-center gap-0.5 rounded-lg border border-[var(--theme-border-default)] p-0.5"
            role="group"
            :aria-label="t(locale, 'desktop.analytics.toolbarMetric')"
          >
            <button
              v-for="m in METRICS"
              :key="m.value"
              type="button"
              class="rounded-md px-2 py-1 text-xs font-medium transition-colors duration-150"
              :class="
                analytics.analyticsMetric === m.value
                  ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
                  : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
              "
              :aria-pressed="analytics.analyticsMetric === m.value"
              @click="analytics.analyticsMetric = m.value"
            >
              {{ t(locale, m.key) }}
            </button>
          </div>
          <button
            type="button"
            class="grid h-7 w-7 place-items-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
            :title="t(locale, 'statistics.previousMonth')"
            :aria-label="t(locale, 'statistics.previousMonth')"
            @click="moveMonth(-1)"
          >
            <ChevronLeft :size="15" aria-hidden="true" />
          </button>
          <button
            type="button"
            class="grid h-7 w-7 place-items-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
            :title="t(locale, 'statistics.nextMonth')"
            :aria-label="t(locale, 'statistics.nextMonth')"
            @click="moveMonth(1)"
          >
            <ChevronRight :size="15" aria-hidden="true" />
          </button>
        </div>
      </div>

      <!-- 月历：完整周标题 + 日期 + 精确 tooltip（设计 7.6）；格子固定紧凑高度，保证整月一屏可见 -->
      <div class="grid grid-cols-7 gap-1.5">
        <div
          v-for="wd in weekDayLabels"
          :key="wd"
          class="pb-1 text-center text-xs font-medium text-[var(--theme-text-quaternary)]"
        >
          {{ wd }}
        </div>
        <div
          v-for="cell in calendarCells"
          :key="cell.key"
          class="grid h-9 place-items-center"
        >
          <button
            v-if="cell.day"
            type="button"
            class="grid h-full w-full place-items-center rounded-md border text-xs font-mono transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
            :class="[
              intensityClass(monthCellRatio(cell.day)),
              selectedDate === cell.key
                ? 'border-[var(--theme-accent-primary)] ring-2 ring-[var(--theme-ring-focus)]'
                : 'border-transparent hover:border-[var(--theme-border-strong)]'
            ]"
            :title="monthCalendarTooltip(cell.day)"
            @click="selectDay(cell.day)"
          >
            {{ cell.dayNumber }}
          </button>
          <span v-else></span>
        </div>
      </div>
    </div>

    <!-- 年度贡献图：横向 53 周、纵向 7 天 -->
    <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
      <h3 class="mb-3 flex items-center gap-1.5 text-[15px] font-semibold text-[var(--theme-text-secondary)]">
        <BarChart3 :size="14" class="shrink-0" aria-hidden="true" />
        {{ t(locale, 'desktop.analytics.yearContribution') }}
        <span class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">{{ monthYear }}</span>
      </h3>
      <div
        v-if="store.yearActivityLoading && !store.yearActivity"
        class="h-[120px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"
        :aria-label="t(locale, 'common.syncing')"
      ></div>
      <div
        v-else-if="annualCells.some(c => c.day && valueOf(c.day, analytics.analyticsMetric) > 0)"
        class="grid grid-flow-col grid-rows-7 gap-[2px] overflow-hidden"
        :style="{ gridAutoColumns: 'minmax(0, 1fr)' }"
      >
        <span
          v-for="cell in annualCells"
          :key="cell.key"
          :class="[
            'aspect-square w-full rounded-[3px]',
            cell.day ? intensityClass(cellRatio(cell.day)) : 'bg-transparent'
          ]"
          :title="cell.day ? dayTooltip(cell.day) : undefined"
        ></span>
      </div>
      <div
        v-else
        class="grid place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center text-xs text-[var(--theme-text-tertiary)]"
      >
        {{ t(locale, 'desktop.analytics.yearEmpty') }}
      </div>
    </div>

    <!-- 小时 × 星期热力图：本期无数据源，占位说明 -->
    <div class="rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center">
      <p class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.analytics.hourlyHeatmapComingSoon') }}</p>
    </div>
  </div>
</template>
