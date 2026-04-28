<script setup lang="ts">
import { computed, ref } from 'vue'
import { ChevronLeft, ChevronRight } from 'lucide-vue-next'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenPair, formatTokenValue } from '../../utils/format'
import type { AppLocale, DayActivity, MonthActivity, StatisticsMetric, YearActivity } from '../../types'
import { useMonitorStore } from '../../stores/monitor'

const store = useMonitorStore()

type ActivityViewMode = 'month' | 'year'

const props = defineProps<{
  activity: MonthActivity | null
  yearActivity: YearActivity | null
  locale: AppLocale
  metric: StatisticsMetric
  viewMode: ActivityViewMode
  year: number
  month: number
  loading: boolean
  yearLoading: boolean
  selectedDate: string
}>()

const emit = defineEmits<{
  previous: []
  next: []
  selectDay: [day: DayActivity]
  setMetric: [value: StatisticsMetric]
  setView: [value: ActivityViewMode]
}>()

const hovered = ref<DayActivity | null>(null)
const gridWrap = ref<HTMLElement | null>(null)
const tooltipStyle = ref<Record<string, string>>({})

interface CalendarCell {
  key: string
  day: DayActivity | null
  dayNumber: string
}

interface AnnualCell {
  key: string
  day: DayActivity | null
}

const periodLabel = computed(() => {
  if (props.viewMode === 'year') return String(props.year)
  return `${props.year}-${String(props.month).padStart(2, '0')}`
})

const activeDays = computed(() => {
  if (props.viewMode === 'year') {
    if (props.yearActivity?.year !== props.year) {
      return []
    }
    return props.yearActivity.days
  }

  if (props.activity?.year !== props.year || props.activity?.month !== props.month) {
    return []
  }
  return props.activity.days
})

const maxValue = computed(() => {
  const values = activeDays.value.map(valueOf)
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

const annualCells = computed(() => {
  const cells: AnnualCell[] = []
  const dayMap = new Map(activeDays.value.map(day => [day.date, day]))
  const start = new Date(props.year, 0, 1)
  const end = new Date(props.year + 1, 0, 1)
  const leadingCount = start.getDay() === 0 ? 6 : start.getDay() - 1

  for (let index = 0; index < leadingCount; index += 1) {
    cells.push({ key: `year-leading-${index}`, day: null })
  }

  for (let date = new Date(start); date < end; date.setDate(date.getDate() + 1)) {
    const key = formatLocalDate(date)
    cells.push({ key, day: dayMap.get(key) ?? makeEmptyDay(key) })
  }

  const trailingCount = Math.max(0, Math.ceil(cells.length / 7) * 7 - cells.length)
  for (let index = 0; index < trailingCount; index += 1) {
    cells.push({ key: `year-trailing-${index}`, day: null })
  }

  return cells
})

const annualWeeks = computed(() => Math.max(1, Math.ceil(annualCells.value.length / 7)))

const annualMonthLabels = computed(() => {
  const labels: Array<{ key: string; label: string; column: number }> = []
  for (let month = 0; month < 12; month += 1) {
    const firstDay = new Date(props.year, 0, 1).getDay()
    const leadingCount = firstDay === 0 ? 6 : firstDay - 1
    const dayOffset = Math.floor((Date.UTC(props.year, month, 1) - Date.UTC(props.year, 0, 1)) / 86400000)
    labels.push({
      key: `${props.year}-${month + 1}`,
      label: String(month + 1),
      column: Math.floor((leadingCount + dayOffset) / 7) + 1
    })
  }
  return labels
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

const metrics: Array<{ value: StatisticsMetric; key: string }> = [
  { value: 'cost', key: 'statistics.metricCost' },
  { value: 'requests', key: 'statistics.metricRequests' },
  { value: 'tokens', key: 'statistics.metricTokens' }
]

const todayKey = formatLocalDate(new Date())

function formatLocalDate(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function getMonthDayCount(year: number, month: number): number {
  return new Date(year, month, 0).getDate()
}

function makeEmptyDay(date: string): DayActivity {
  return {
    date,
    requestCount: 0,
    totalTokens: 0,
    inputTokens: 0,
    outputTokens: 0,
    cacheCreateTokens: 0,
    cacheReadTokens: 0,
    cost: 0,
    modelCount: 0,
    successRequests: null,
    errorRequests: null
  }
}

function valueOf(day: DayActivity): number {
  if (props.metric === 'cost') return day.cost
  if (props.metric === 'tokens') return day.totalTokens
  return day.requestCount
}

function cellClass(day: DayActivity): string {
  const active = props.selectedDate === day.date
  const today = todayKey === day.date
  const base = 'relative flex h-7 min-w-0 flex-col items-center justify-center rounded-lg text-[9px] transition'
  if (active) return `${base} bg-emerald-50 text-emerald-700 ring-1 ring-emerald-400 dark:bg-emerald-500/10 dark:text-emerald-200 dark:ring-emerald-500/70`
  if (today) return `${base} bg-gray-50 text-gray-600 ring-1 ring-gray-200 dark:bg-neutral-800/70 dark:text-gray-300 dark:ring-neutral-700`
  return `${base} text-gray-600 hover:bg-gray-50 dark:text-gray-300 dark:hover:bg-neutral-800/70`
}

function dayDotClass(day: DayActivity): string {
  const value = valueOf(day)
  const ratio = maxValue.value > 0 ? value / maxValue.value : 0
  const base = `h-2 w-2 rounded-full transition-all${props.loading ? ' animate-pulse' : ''}`

  return `${base} ${intensityClass(ratio)}`
}

function annualCellClass(day: DayActivity): string {
  const value = valueOf(day)
  const ratio = maxValue.value > 0 ? value / maxValue.value : 0
  const active = props.selectedDate === day.date
  const today = todayKey === day.date
  const loading = props.viewMode === 'year' ? props.yearLoading : props.loading
  const base = `h-1.5 w-1.5 rounded-[2px] transition ${loading ? ' animate-pulse' : ''}`
  const ring = active
    ? 'ring-1 ring-emerald-500 ring-offset-1 ring-offset-white dark:ring-emerald-300 dark:ring-offset-[#1C1C1E]'
    : today
      ? 'ring-1 ring-gray-300 dark:ring-neutral-600'
      : ''

  return `${base} ${intensityClass(ratio)} ${ring}`
}

function intensityClass(ratio: number): string {
  if (ratio <= 0) return 'bg-gray-100 dark:bg-neutral-800'
  if (ratio < 0.25) return 'bg-emerald-200 dark:bg-emerald-900/70'
  if (ratio < 0.5) return 'bg-emerald-300 dark:bg-emerald-700'
  if (ratio < 0.75) return 'bg-emerald-400 dark:bg-emerald-500'
  return 'bg-emerald-500 dark:bg-emerald-300'
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
  <section class="rounded-2xl border border-gray-50 bg-white p-3 shadow-[0_2px_10px_rgba(0,0,0,0.02)] dark:border-neutral-800 dark:bg-[#1C1C1E]">
    <div class="mb-2 flex items-center justify-between gap-2">
      <div>
        <p class="font-mono text-sm font-bold text-gray-900 dark:text-gray-100">{{ periodLabel }}</p>
      </div>
      <div class="flex min-w-0 flex-wrap items-center justify-end gap-1.5">
        <div class="grid grid-cols-2 gap-0.5 rounded-full bg-gray-50 p-0.5 dark:bg-neutral-800/70">
          <button
            class="h-6 rounded-full px-2 text-[10px] font-semibold transition"
            :class="viewMode === 'month' ? 'bg-white text-emerald-600 shadow-[0_1px_4px_rgba(0,0,0,0.04)] dark:bg-neutral-700 dark:text-emerald-300' : 'text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-100'"
            @click="emit('setView', 'month')"
          >
            {{ t(locale, 'statistics.monthView') }}
          </button>
          <button
            class="h-6 rounded-full px-2 text-[10px] font-semibold transition"
            :class="viewMode === 'year' ? 'bg-white text-emerald-600 shadow-[0_1px_4px_rgba(0,0,0,0.04)] dark:bg-neutral-700 dark:text-emerald-300' : 'text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-100'"
            @click="emit('setView', 'year')"
          >
            {{ t(locale, 'statistics.yearView') }}
          </button>
        </div>
        <div class="grid grid-cols-3 gap-0.5 rounded-full bg-gray-50 p-0.5 dark:bg-neutral-800/70">
          <button
            v-for="item in metrics"
            :key="item.value"
            class="h-6 rounded-full px-2 text-[10px] font-semibold transition"
            :class="metric === item.value ? 'bg-white text-emerald-600 shadow-[0_1px_4px_rgba(0,0,0,0.04)] dark:bg-neutral-700 dark:text-emerald-300' : 'text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-100'"
            @click="emit('setMetric', item.value)"
          >
            {{ t(locale, item.key) }}
          </button>
        </div>
        <button class="grid h-7 w-7 place-items-center rounded-full text-gray-500 transition hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-neutral-800" :title="t(locale, viewMode === 'year' ? 'statistics.previousYear' : 'statistics.previousMonth')" @click="emit('previous')">
          <ChevronLeft class="h-4 w-4" />
        </button>
        <button class="grid h-7 w-7 place-items-center rounded-full text-gray-500 transition hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-neutral-800" :title="t(locale, viewMode === 'year' ? 'statistics.nextYear' : 'statistics.nextMonth')" @click="emit('next')">
          <ChevronRight class="h-4 w-4" />
        </button>
      </div>
    </div>

    <div ref="gridWrap" class="relative">
      <template v-if="viewMode === 'month'">
        <div class="grid grid-cols-7 gap-1 text-center text-[9px] font-semibold text-gray-600 dark:text-gray-300">
          <span v-for="day in weekDays" :key="day">{{ day }}</span>
        </div>
        <div class="mt-1 grid grid-cols-7 gap-1">
          <template v-for="cell in calendarCells" :key="cell.key">
            <button
              v-if="cell.day"
              :class="cellClass(cell.day)"
              :disabled="loading"
              @mouseenter="handleDayEnter(cell.day, $event)"
              @mouseleave="handleDayLeave"
              @click="emit('selectDay', cell.day)"
            >
              <span class="mb-0.5 font-mono leading-none">{{ cell.dayNumber }}</span>
              <span :class="dayDotClass(cell.day)" />
            </button>
            <span v-else class="h-7 rounded-lg" />
          </template>
        </div>
      </template>

      <div v-else class="overflow-visible pb-1">
        <div>
          <div class="ml-5 grid gap-px text-[8px] font-semibold text-gray-600 dark:text-gray-300" :style="{ gridTemplateColumns: `repeat(${annualWeeks}, minmax(0, 1fr))` }">
            <span
              v-for="label in annualMonthLabels"
              :key="label.key"
              class="truncate leading-3"
              :style="{ gridColumn: label.column }"
            >
              {{ label.label }}
            </span>
          </div>
          <div class="mt-1 grid grid-cols-[16px_minmax(0,1fr)] gap-1">
            <div class="grid grid-rows-7 gap-px text-[8px] font-semibold leading-[6px] text-gray-600 dark:text-gray-300">
              <span>{{ weekDays[0] }}</span>
              <span />
              <span>{{ weekDays[2] }}</span>
              <span />
              <span>{{ weekDays[4] }}</span>
              <span />
              <span />
            </div>
            <div class="grid grid-flow-col grid-rows-7 gap-px" :style="{ gridTemplateColumns: `repeat(${annualWeeks}, minmax(0, 1fr))` }">
              <template v-for="cell in annualCells" :key="cell.key">
                <button
                  v-if="cell.day"
                  class="grid aspect-square min-h-0 w-full place-items-center rounded-[2px]"
                  :disabled="yearLoading"
                  @mouseenter="handleDayEnter(cell.day, $event)"
                  @mouseleave="handleDayLeave"
                  @click="emit('selectDay', cell.day)"
                >
                  <span :class="annualCellClass(cell.day)" />
                </button>
                <span v-else class="aspect-square w-full" />
              </template>
            </div>
          </div>
        </div>
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
  </section>
</template>
