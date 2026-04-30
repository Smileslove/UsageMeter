<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenPair, formatTokenValue } from '../../utils/format'
import { getTodayKey, makeEmptyDay, intensityClass, valueOf, formatLocalDate, type AnnualCell } from './activityUtils'
import type { AppLocale, DayActivity, StatisticsMetric, YearActivity } from '../../types'
import { useMonitorStore } from '../../stores/monitor'

const store = useMonitorStore()

const props = defineProps<{
  yearActivity: YearActivity | null
  locale: AppLocale
  metric: StatisticsMetric
  year: number
  loading: boolean
  selectedDate: string
}>()

const emit = defineEmits<{
  selectDay: [day: DayActivity]
}>()

const hovered = ref<DayActivity | null>(null)
const gridWrap = ref<HTMLElement | null>(null)
const yearGridScroll = ref<HTMLElement | null>(null)
const tooltipStyle = ref<Record<string, string>>({})

// 拖拽滚动状态
const isDragging = ref(false)
const dragStartX = ref(0)
const dragScrollLeft = ref(0)

const activeDays = computed(() => {
  if (props.yearActivity?.year !== props.year) {
    return []
  }
  return props.yearActivity.days
})

const maxValue = computed(() => {
  const values = activeDays.value.map(day => valueOf(day, props.metric))
  return Math.max(...values, 0)
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

  // 使用时间戳迭代避免修改原对象
  const startTime = start.getTime()
  const endTime = end.getTime()
  const oneDayMs = 86400000

  for (let time = startTime; time < endTime; time += oneDayMs) {
    const date = new Date(time)
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

const weekDays = computed(() => [
  t(props.locale, 'statistics.weekMon'),
  t(props.locale, 'statistics.weekTue'),
  t(props.locale, 'statistics.weekWed'),
  t(props.locale, 'statistics.weekThu'),
  t(props.locale, 'statistics.weekFri'),
  t(props.locale, 'statistics.weekSat'),
  t(props.locale, 'statistics.weekSun')
])

// 月份标签（使用本地化的月份名称）
// GitHub 风格：月份标签显示在该月第一天所在的列，但同月只显示一次
const monthLabelPositions = computed(() => {
  const positions: Array<string | null> = []
  const firstDayOfYear = new Date(props.year, 0, 1).getDay()
  const leadingCount = firstDayOfYear === 0 ? 6 : firstDayOfYear - 1

  // 判断是否闰年
  const isLeapYear = (props.year % 4 === 0 && props.year % 100 !== 0) || (props.year % 400 === 0)
  const daysInYear = isLeapYear ? 366 : 365

  // 记录每个列开始的月份（该列第一个有效日期的月份）
  let lastMonth: number | null = null

  for (let col = 0; col < annualWeeks.value; col += 1) {
    // 这一列的第一个格子对应的日期偏移
    const startCellIndex = leadingCount + col * 7
    // 找到这一列中第一个有效日期
    let firstValidMonth: number | null = null

    for (let row = 0; row < 7; row += 1) {
      const dayOfYear = startCellIndex + row - leadingCount
      if (dayOfYear >= 0 && dayOfYear < daysInYear) {
        const date = new Date(props.year, 0, 1)
        date.setDate(date.getDate() + dayOfYear)
        firstValidMonth = date.getMonth()
        break
      }
    }

    // 只有当月份变化时才显示标签
    if (firstValidMonth !== null && firstValidMonth !== lastMonth) {
      positions.push(t(props.locale, `statistics.month${firstValidMonth + 1}`))
      lastMonth = firstValidMonth
    } else {
      positions.push(null)
    }
  }
  return positions
})

const hoveredTokenPair = computed(() => {
  if (!hovered.value) {
    return { input: '0.00', output: '0.00' }
  }
  return formatTokenPair(hovered.value.inputTokens, hovered.value.outputTokens)
})

function annualCellClass(day: DayActivity): string {
  const val = valueOf(day, props.metric)
  const ratio = maxValue.value > 0 ? val / maxValue.value : 0
  const active = props.selectedDate === day.date
  const today = getTodayKey() === day.date
  const base = `h-3 w-3 rounded-[3px] transition ${props.loading ? ' animate-pulse' : ''}`
  const ring = active
    ? 'ring-1 ring-emerald-500 ring-offset-1 ring-offset-white dark:ring-emerald-300 dark:ring-offset-[#1C1C1E]'
    : today
      ? 'ring-1 ring-gray-300 dark:ring-neutral-600'
      : ''

  return `${base} ${intensityClass(ratio)} ${ring}`
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

// 拖拽滚动功能
function handleDragStart(event: MouseEvent) {
  if (!yearGridScroll.value) return
  isDragging.value = true
  dragStartX.value = event.pageX - yearGridScroll.value.offsetLeft
  dragScrollLeft.value = yearGridScroll.value.scrollLeft
  yearGridScroll.value.style.cursor = 'grabbing'
  yearGridScroll.value.style.userSelect = 'none'
}

function handleDragMove(event: MouseEvent) {
  if (!isDragging.value || !yearGridScroll.value) return
  event.preventDefault()
  const x = event.pageX - yearGridScroll.value.offsetLeft
  const walk = (x - dragStartX.value) * 1.5
  yearGridScroll.value.scrollLeft = dragScrollLeft.value - walk
}

function handleDragEnd() {
  if (!yearGridScroll.value) return
  isDragging.value = false
  yearGridScroll.value.style.cursor = 'grab'
  yearGridScroll.value.style.userSelect = ''
}

// 自动滚动到当前月份
function scrollToCurrentMonth() {
  if (!yearGridScroll.value) return
  const today = new Date()
  if (today.getFullYear() !== props.year) return

  const firstDay = new Date(props.year, 0, 1).getDay()
  const leadingCount = firstDay === 0 ? 6 : firstDay - 1
  const dayOffset = Math.floor((Date.UTC(props.year, today.getMonth(), 1) - Date.UTC(props.year, 0, 1)) / 86400000)
  const targetColumn = Math.floor((leadingCount + dayOffset) / 7)

  const cellWidth = 14
  const scrollPosition = targetColumn * cellWidth - yearGridScroll.value.clientWidth / 3

  yearGridScroll.value.scrollLeft = Math.max(0, scrollPosition)
}

onMounted(() => {
  nextTick(() => {
    scrollToCurrentMonth()
  })
})
</script>

<template>
  <div ref="gridWrap" class="relative">
    <!-- 主体区域：左侧固定 + 右侧可滚动 -->
    <div class="flex">
      <!-- 左侧固定的星期标签 -->
      <div class="shrink-0 grid grid-rows-7 gap-0.5 text-[8px] font-semibold leading-3 text-gray-600 dark:text-gray-300 pt-4">
        <span>{{ weekDays[0] }}</span>
        <span />
        <span>{{ weekDays[2] }}</span>
        <span />
        <span>{{ weekDays[4] }}</span>
        <span />
        <span />
      </div>

      <!-- 右侧可滚动区域 -->
      <div
        ref="yearGridScroll"
        class="ml-1 flex-1 overflow-x-auto cursor-grab scrollbar-hide"
        @mousedown="handleDragStart"
        @mousemove="handleDragMove"
        @mouseup="handleDragEnd"
        @mouseleave="handleDragEnd"
      >
        <div class="min-w-max">
          <!-- 月份标签 -->
          <div class="grid gap-0.5 text-[10px] font-semibold text-gray-600 dark:text-gray-300 mb-1" :style="{ gridTemplateColumns: `repeat(${annualWeeks}, 12px)` }">
            <span
              v-for="(label, index) in monthLabelPositions"
              :key="index"
              class="whitespace-nowrap leading-3"
            >
              {{ label }}
            </span>
          </div>
          <!-- 格子区域 -->
          <div class="grid grid-flow-col grid-rows-7 gap-0.5" :style="{ gridTemplateColumns: `repeat(${annualWeeks}, 12px)` }">
            <template v-for="cell in annualCells" :key="cell.key">
              <button
                v-if="cell.day"
                class="grid aspect-square place-items-center rounded-[3px]"
                style="width: 12px; height: 12px;"
                :disabled="loading"
                @mouseenter="handleDayEnter(cell.day, $event)"
                @mouseleave="handleDayLeave"
                @click="emit('selectDay', cell.day)"
              >
                <span :class="annualCellClass(cell.day)" />
              </button>
              <span v-else class="aspect-square" style="width: 12px; height: 12px;" />
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
</template>
