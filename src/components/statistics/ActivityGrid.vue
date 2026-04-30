<script setup lang="ts">
import { computed } from 'vue'
import { ChevronLeft, ChevronRight } from 'lucide-vue-next'
import { t } from '../../i18n'
import { METRICS } from './activityUtils'
import MonthActivityGrid from './MonthActivityGrid.vue'
import YearActivityGrid from './YearActivityGrid.vue'
import type { AppLocale, DayActivity, MonthActivity, StatisticsMetric, YearActivity } from '../../types'

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

const periodLabel = computed(() => {
  if (props.viewMode === 'year') return String(props.year)
  return `${props.year}-${String(props.month).padStart(2, '0')}`
})
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
            v-for="item in METRICS"
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

    <MonthActivityGrid
      v-if="viewMode === 'month'"
      :activity="activity"
      :locale="locale"
      :metric="metric"
      :year="year"
      :month="month"
      :loading="loading"
      :selected-date="selectedDate"
      @select-day="emit('selectDay', $event)"
    />

    <YearActivityGrid
      v-else
      :year-activity="yearActivity"
      :locale="locale"
      :metric="metric"
      :year="year"
      :loading="yearLoading"
      :selected-date="selectedDate"
      @select-day="emit('selectDay', $event)"
    />
  </section>
</template>
