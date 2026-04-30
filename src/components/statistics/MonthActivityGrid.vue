<script setup lang="ts">
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenPair, formatTokenValue } from '../../utils/format'
import { getTodayKey, makeEmptyDay, getMonthDayCount, intensityClass, valueOf, type CalendarCell } from './activityUtils'
import type { AppLocale, DayActivity, MonthActivity, StatisticsMetric } from '../../types'
import { useMonitorStore } from '../../stores/monitor'

const store = useMonitorStore()

const props = defineProps<{
  activity: MonthActivity | null
  locale: AppLocale
  metric: StatisticsMetric
  year: number
  month: number
  loading: boolean
  selectedDate: string
}>()

const emit = defineEmits<{
  selectDay: [day: DayActivity]
}>()

const hovered = ref<DayActivity | null>(null)
const gridWrap = ref<HTMLElement | null>(null)
const tooltipStyle = ref<Record<string, string>>({})

const activeDays = computed(() => {
  if (props.activity?.year !== props.year || props.activity?.month !== props.month) {
    return []
  }
  return props.activity.days
})

const maxValue = computed(() => {
  const values = activeDays.value.map(day => valueOf(day, props.metric))
  return Math.max(...values, 0)
})

const calendarCells = computed(() => {
  const dayMap = new Map(activeDays.value.map(day => [day.date, day]))
  const dayCount = getMonthDayCount(props.year, props.month)
  const firstDay = new Date(props.year, props.month - 1, 1).getDay()
  const leadingCount = firstDay === 0 ? 6 : firstDay - 1
  const cells: CalendarCell[] = Array.from({ length: leadingCount }, (_, index) => ({
    key: `blank-leading-${index}`,
    day: null,
    dayNumber: ''
  }))

  for (let day = 1; day <= dayCount; day += 1) {
    const date = `${props.year}-${String(props.month).padStart(2, '0')}-${String(day).padStart(2, '0')}`
    cells.push({
      key: date,
      day: dayMap.get(date) ?? makeEmptyDay(date),
      dayNumber: String(day)
    })
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

const weekDays = computed(() => [
  t(props.locale, 'statistics.weekMon'),
  t(props.locale, 'statistics.weekTue'),
  t(props.locale, 'statistics.weekWed'),
  t(props.locale, 'statistics.weekThu'),
  t(props.locale, 'statistics.weekFri'),
  t(props.locale, 'statistics.weekSat'),
  t(props.locale, 'statistics.weekSun')
])

const hoveredTokenPair = computed(() => {
  if (!hovered.value) {
    return { input: '0.00', output: '0.00' }
  }
  return formatTokenPair(hovered.value.inputTokens, hovered.value.outputTokens)
})

function cellClass(day: DayActivity): string {
  const active = props.selectedDate === day.date
  const today = getTodayKey() === day.date
  const base = 'relative flex h-7 min-w-0 flex-col items-center justify-center rounded-lg text-[10px] font-semibold transition'
  if (active) return `${base} bg-emerald-50 text-emerald-700 ring-1 ring-emerald-400 dark:bg-emerald-500/10 dark:text-emerald-200 dark:ring-emerald-500/70`
  if (today) return `${base} bg-gray-50 text-gray-600 ring-1 ring-gray-200 dark:bg-neutral-800/70 dark:text-gray-300 dark:ring-neutral-700`
  return `${base} text-gray-600 hover:bg-gray-50 dark:text-gray-300 dark:hover:bg-neutral-800/70`
}

function dayDotClass(day: DayActivity): string {
  const val = valueOf(day, props.metric)
  const ratio = maxValue.value > 0 ? val / maxValue.value : 0
  const base = `h-2.5 w-2.5 rounded-full transition-all${props.loading ? ' animate-pulse' : ''}`
  return `${base} ${intensityClass(ratio)}`
}

function handleDayEnter(day: DayActivity, event: MouseEvent) {
  hovered.value = day

  const wrapper = gridWrap.value
  const target = event.currentTarget as HTMLElement | null
  if (!wrapper || !target) {
    tooltipStyle.value = {}
    return
  }

  const wrapRect = wrapper.getBoundingClientRect()
  const targetRect = target.getBoundingClientRect()
  const tooltipWidth = 176
  const edgeGap = 4
  const center = targetRect.left - wrapRect.left + targetRect.width / 2
  const minX = tooltipWidth / 2 + edgeGap
  const maxX = Math.max(minX, wrapRect.width - tooltipWidth / 2 - edgeGap)
  const left = Math.min(Math.max(center, minX), maxX)
  const showAbove = targetRect.top - wrapRect.top > 84
  const top = showAbove
    ? targetRect.top - wrapRect.top - 8
    : targetRect.bottom - wrapRect.top + 8

  tooltipStyle.value = {
    left: `${left}px`,
    top: `${top}px`,
    transform: showAbove ? 'translate(-50%, -100%)' : 'translate(-50%, 0)'
  }
}

function handleDayLeave() {
  hovered.value = null
  tooltipStyle.value = {}
}
</script>

<template>
  <div ref="gridWrap" class="relative">
    <div class="grid grid-cols-7 gap-0.5 text-center text-[10px] font-semibold text-gray-600 dark:text-gray-300">
      <span v-for="day in weekDays" :key="day">{{ day }}</span>
    </div>
    <div class="mt-1 grid grid-cols-7 gap-0.5">
      <template v-for="cell in calendarCells" :key="cell.key">
        <button
          v-if="cell.day"
          :class="cellClass(cell.day)"
          :disabled="loading"
          @mouseenter="handleDayEnter(cell.day, $event)"
          @mouseleave="handleDayLeave"
          @click="emit('selectDay', cell.day)"
        >
          <span class="font-mono leading-tight">{{ cell.dayNumber }}</span>
          <span class="mt-0.5" :class="dayDotClass(cell.day)" />
        </button>
        <span v-else class="h-7 rounded-lg" />
      </template>
    </div>

    <div v-if="hovered" :style="tooltipStyle" class="pointer-events-none absolute z-10 w-44 rounded-xl border border-gray-100 bg-white/95 p-2.5 text-[11px] leading-5 shadow-lg backdrop-blur dark:border-neutral-700 dark:bg-[#252527]/95">
      <div class="mb-1.5 font-mono text-[12px] font-semibold text-gray-800 dark:text-gray-100">{{ hovered.date }}</div>
      <div class="grid grid-cols-[minmax(0,1fr)_auto] gap-x-2 text-gray-500 dark:text-gray-400"><span class="truncate">{{ t(locale, 'statistics.requests') }}</span><span class="font-mono font-semibold text-gray-800 dark:text-gray-100">{{ formatRequestCount(hovered.requestCount) }}</span></div>
      <div class="grid grid-cols-[minmax(0,1fr)_auto] gap-x-2 text-gray-500 dark:text-gray-400"><span class="truncate">{{ t(locale, 'statistics.inputTokens') }}</span><span class="font-mono font-semibold text-gray-800 dark:text-gray-100">{{ hoveredTokenPair.input }}</span></div>
      <div class="grid grid-cols-[minmax(0,1fr)_auto] gap-x-2 text-gray-500 dark:text-gray-400"><span class="truncate">{{ t(locale, 'statistics.outputTokens') }}</span><span class="font-mono font-semibold text-gray-800 dark:text-gray-100">{{ hoveredTokenPair.output }}</span></div>
      <div class="grid grid-cols-[minmax(0,1fr)_auto] gap-x-2 text-gray-500 dark:text-gray-400"><span class="truncate">{{ t(locale, 'statistics.totalTokens') }}</span><span class="font-mono font-semibold text-gray-800 dark:text-gray-100">{{ formatTokenValue(hovered.totalTokens) }}</span></div>
      <div class="grid grid-cols-[minmax(0,1fr)_auto] gap-x-2 text-gray-500 dark:text-gray-400"><span class="truncate">{{ t(locale, 'statistics.cost') }}</span><span class="font-mono font-semibold text-gray-800 dark:text-gray-100">{{ formatCost(hovered.cost, store.settings.currency) }}</span></div>
    </div>
  </div>
</template>
