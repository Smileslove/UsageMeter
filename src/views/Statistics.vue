<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import type { DayActivity, StatisticsBucket, StatisticsMetric, StatisticsRangePreset } from '../types'
import MonthActivityGrid from '../components/statistics/MonthActivityGrid.vue'
import StatisticsRangePicker from '../components/statistics/StatisticsRangePicker.vue'
import StatisticsMetricCards from '../components/statistics/StatisticsMetricCards.vue'
import StatisticsTrendChart from '../components/statistics/StatisticsTrendChart.vue'
import StatisticsModelList from '../components/statistics/StatisticsModelList.vue'
import { backendErrorLabel } from '../i18n'

const store = useMonitorStore()
const preset = ref<StatisticsRangePreset>('7d')
const monthMetric = ref<StatisticsMetric>('cost')
const analysisMetric = ref<StatisticsMetric>('cost')
const currentMonth = ref(new Date())
const activityView = ref<'month' | 'year'>('month')
const selectedDate = ref('')
const customStart = ref(toDateTimeInput(addDays(startOfLocalDay(new Date()), -6)))
const customEnd = ref(toDateTimeInput(new Date()))

const locale = computed(() => store.settings.locale)

function toDateInput(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function toDateTimeInput(date: Date): string {
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  return `${toDateInput(date)}T${hours}:${minutes}`
}

function startOfLocalDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate(), 0, 0, 0, 0)
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date)
  next.setDate(next.getDate() + days)
  return next
}

function presetRangeDates(value: StatisticsRangePreset): { start: Date; end: Date } {
  const now = new Date()
  if (value === '5h') {
    return { start: new Date(now.getTime() - 5 * 60 * 60 * 1000), end: now }
  }
  if (value === 'today') {
    return { start: startOfLocalDay(now), end: now }
  }
  if (value === '1d') {
    return { start: new Date(now.getTime() - 24 * 60 * 60 * 1000), end: now }
  }
  if (value === '7d') {
    return { start: addDays(startOfLocalDay(now), -6), end: now }
  }
  if (value === '30d') {
    return { start: addDays(startOfLocalDay(now), -29), end: now }
  }
  if (value === 'current_month') {
    return { start: new Date(now.getFullYear(), now.getMonth(), 1), end: now }
  }
  const start = customStart.value ? new Date(customStart.value) : addDays(startOfLocalDay(now), -6)
  const end = customEnd.value ? new Date(customEnd.value) : now
  return { start, end }
}

function setPreset(value: StatisticsRangePreset) {
  preset.value = value
  if (value !== 'custom') {
    const next = presetRangeDates(value)
    customStart.value = toDateTimeInput(next.start)
    customEnd.value = toDateTimeInput(next.end)
  }
}

function setCustomStart(value: string) {
  customStart.value = value
  preset.value = 'custom'
}

function setCustomEnd(value: string) {
  customEnd.value = value
  preset.value = 'custom'
}

const range = computed(() => {
  const { start, end } = presetRangeDates(preset.value)
  return { start: Math.floor(start.getTime() / 1000), end: Math.floor(end.getTime() / 1000) }
})

const bucket = computed<StatisticsBucket>(() => {
  const hours = (range.value.end - range.value.start) / 3600
  return hours <= 48 ? 'hour' : 'day'
})

const monthYear = computed(() => currentMonth.value.getFullYear())
const monthNumber = computed(() => currentMonth.value.getMonth() + 1)

function fetchSummary() {
  store.fetchStatisticsSummary({
    startEpoch: range.value.start,
    endEpoch: range.value.end,
    timezone: store.settings.timezone,
    bucket: bucket.value,
    metric: analysisMetric.value
  })
}

function fetchMonth() {
  if (activityView.value === 'year') {
    store.fetchYearActivity(monthYear.value, monthMetric.value)
    return
  }
  store.fetchMonthActivity(monthYear.value, monthNumber.value, monthMetric.value)
}

function moveMonth(delta: number) {
  if (activityView.value === 'year') {
    currentMonth.value = new Date(currentMonth.value.getFullYear() + delta, currentMonth.value.getMonth(), 1)
    return
  }
  currentMonth.value = new Date(currentMonth.value.getFullYear(), currentMonth.value.getMonth() + delta, 1)
}

function selectDay(day: DayActivity) {
  selectedDate.value = day.date
  customStart.value = `${day.date}T00:00`
  customEnd.value = `${day.date}T23:59`
  preset.value = 'custom'
}

watch([range, bucket, analysisMetric], fetchSummary, { deep: true })
watch([activityView, monthYear, monthNumber, monthMetric], fetchMonth)

onMounted(() => {
  fetchSummary()
  fetchMonth()
})
</script>

<template>
  <div class="space-y-3 pb-2 animate-in fade-in zoom-in-95 duration-300">
    <div v-if="store.statisticsError" class="rounded-xl border border-rose-100 bg-rose-50 p-2.5 text-[11px] font-medium text-rose-700 dark:border-rose-900/40 dark:bg-rose-950/30 dark:text-rose-300">
      {{ backendErrorLabel(locale, store.statisticsError) }}
    </div>

    <MonthActivityGrid
      :activity="store.monthActivity"
      :year-activity="store.yearActivity"
      :locale="locale"
      :metric="monthMetric"
      :view-mode="activityView"
      :year="monthYear"
      :month="monthNumber"
      :loading="store.monthActivityLoading"
      :year-loading="store.yearActivityLoading"
      :selected-date="selectedDate"
      @previous="moveMonth(-1)"
      @next="moveMonth(1)"
      @select-day="selectDay"
      @set-metric="monthMetric = $event"
      @set-view="activityView = $event"
    />

    <section class="rounded-2xl border border-gray-50 bg-white p-3 shadow-[0_2px_10px_rgba(0,0,0,0.02)] dark:border-neutral-800 dark:bg-[#1C1C1E]">
      <StatisticsRangePicker
        :locale="locale"
        :preset="preset"
        :custom-start="customStart"
        :custom-end="customEnd"
        @set-preset="setPreset"
        @set-custom-start="setCustomStart"
        @set-custom-end="setCustomEnd"
      />

      <div class="mt-2 border-t border-gray-100 pt-2 dark:border-neutral-800">
        <StatisticsMetricCards :locale="locale" :totals="store.statisticsSummary?.totals ?? null" />
      </div>

      <div class="mt-2 space-y-2">
        <StatisticsTrendChart
          :locale="locale"
          :metric="analysisMetric"
          :points="store.statisticsSummary?.trend ?? []"
          @set-metric="analysisMetric = $event"
        />
        <StatisticsModelList :locale="locale" :models="store.statisticsSummary?.models ?? []" />
      </div>
    </section>
  </div>
</template>
