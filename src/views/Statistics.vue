<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useMonitorStore } from '../stores/monitor'
import { fetchMonthActivityAction, fetchYearActivityAction } from '../stores/monitorDomains'
import type { ActivityMaterializationDone, DayActivity, StatisticsBucket, StatisticsMetric, StatisticsRangePreset } from '../types'
import ActivityGrid from '../components/statistics/ActivityGrid.vue'
import StatisticsRangePicker from '../components/statistics/StatisticsRangePicker.vue'
import StatisticsMetricCards from '../components/statistics/StatisticsMetricCards.vue'
import StatisticsTrendChart from '../components/statistics/StatisticsTrendChart.vue'
import StatisticsModelList from '../components/statistics/StatisticsModelList.vue'
import { backendErrorLabel } from '../i18n'

const store = useMonitorStore()
const preset = ref<StatisticsRangePreset>('today')
const monthMetric = ref<StatisticsMetric>('cost')
const analysisMetric = ref<StatisticsMetric>('cost')
const currentMonth = ref(new Date())
const activityView = ref<'month' | 'year'>('month')
const selectedDate = ref('')
const customStart = ref(toDateTimeInput(startOfLocalDay(new Date())))
const customEnd = ref(toDateTimeInput(new Date()))
// 标记是否已经初始化完成，用于区分用户操作和初始化
const initialized = ref(false)
let customRangeTimer: ReturnType<typeof setTimeout> | null = null
let unlistenActivityMaterialization: UnlistenFn | null = null

const locale = computed(() => store.settings.locale)
const dayBoundaryHour = computed(() => store.settings.dayBoundaryMode === 'night_owl' ? 4 : 0)

function toDateInput(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function toDateTimeInput(date: Date): string {
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')
  return `${toDateInput(date)}T${hours}:${minutes}:${seconds}`
}

function startOfLocalDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate(), 0, 0, 0, 0)
}

function startOfBusinessDay(date: Date): Date {
  const candidate = new Date(date.getFullYear(), date.getMonth(), date.getDate(), dayBoundaryHour.value, 0, 0, 0)
  if (date.getHours() < dayBoundaryHour.value) {
    candidate.setDate(candidate.getDate() - 1)
  }
  return candidate
}

function businessDayStartFromLabel(dateStr: string): Date {
  const [year, month, day] = dateStr.split('-').map(Number)
  return new Date(year, (month || 1) - 1, day || 1, dayBoundaryHour.value, 0, 0, 0)
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
    return { start: startOfBusinessDay(now), end: now }
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
    return { start: businessDayStartFromLabel(`${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-01`), end: now }
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
    bucket: bucket.value
  })
}

function scheduleSummaryFetch() {
  if (customRangeTimer) {
    clearTimeout(customRangeTimer)
    customRangeTimer = null
  }

  if (preset.value === 'custom') {
    customRangeTimer = setTimeout(() => {
      fetchSummary()
      customRangeTimer = null
    }, 160)
    return
  }

  fetchSummary()
}

function fetchMonth() {
  if (activityView.value === 'year') {
    store.fetchYearActivity(monthYear.value, monthMetric.value)
    return
  }
  store.fetchMonthActivity(monthYear.value, monthNumber.value, monthMetric.value)
}

/**
 * 后端后台物化完成事件回调：仅当 payload 与当前视图的 kind + year 匹配时静默刷新
 * （silent 不翻转 loading，避免打断用户正在观察的加载态）。
 *
 * 匹配规则刻意放宽为 kind + year：后端并发场景会 emit 最新 pending 视图的 scope
 * （而非发起请求时的 scope），month/metric 可能与当前视图不一致；静默刷新按当前
 * monthNumber/monthMetric 拉取最新数据，month/metric 不参与匹配时刷新无害，
 * 并可覆盖用户快速切换月份的场景。
 */
function handleActivityMaterializationDone(payload: ActivityMaterializationDone) {
  if (!payload.ok) return
  if (payload.kind !== activityView.value) return
  if (payload.year !== monthYear.value) return
  if (payload.kind === 'month') {
    void fetchMonthActivityAction(store, monthYear.value, monthNumber.value, monthMetric.value, { silent: true })
  } else {
    void fetchYearActivityAction(store, monthYear.value, monthMetric.value, { silent: true })
  }
}

function moveMonth(delta: number) {
  if (activityView.value === 'year') {
    currentMonth.value = new Date(currentMonth.value.getFullYear() + delta, currentMonth.value.getMonth(), 1)
  } else {
    currentMonth.value = new Date(currentMonth.value.getFullYear(), currentMonth.value.getMonth() + delta, 1)
  }
  // 切换月份/年后，自动更新下方的时间范围
  if (initialized.value) {
    updateRangeFromActivityView()
  }
}

function selectDay(day: DayActivity) {
  selectedDate.value = day.date
  const dayStart = businessDayStartFromLabel(day.date)
  customStart.value = toDateTimeInput(dayStart)
  const todayStr = toDateInput(startOfBusinessDay(new Date()))
  customEnd.value = day.date === todayStr ? toDateTimeInput(new Date()) : toDateTimeInput(addDays(dayStart, 1))
  preset.value = 'custom'
}

/**
 * 根据当前月份/年度视图自动设置时间范围
 * 后端使用半开区间 [start, end)，所以结束时间需要使用次日/下月/下年的 00:00:00
 */
function updateRangeFromActivityView() {
  const year = currentMonth.value.getFullYear()

  if (activityView.value === 'year') {
    // 年度视图：设置范围为该年的第一天到次年第一天
    customStart.value = toDateTimeInput(businessDayStartFromLabel(`${year}-01-01`))
    customEnd.value = toDateTimeInput(businessDayStartFromLabel(`${year + 1}-01-01`))
  } else {
    // 月份视图：设置范围为该月的第一天到下月第一天
    const month = currentMonth.value.getMonth() + 1
    const monthStr = String(month).padStart(2, '0')
    customStart.value = toDateTimeInput(businessDayStartFromLabel(`${year}-${monthStr}-01`))
    // 计算下月第一天的日期
    const nextMonthDate = new Date(year, month, 1)
    customEnd.value = toDateTimeInput(businessDayStartFromLabel(toDateInput(nextMonthDate)))
  }
  preset.value = 'custom'
}

/**
 * 处理视图模式切换（月份/年度）
 */
function setActivityView(mode: 'month' | 'year') {
  activityView.value = mode
  // 切换视图后，自动更新下方的时间范围
  if (initialized.value) {
    updateRangeFromActivityView()
  }
}

// 切换分析指标（analysisMetric）无需重新请求后端：trend 每个点已包含全部指标字段，
// 图表在前端按 metric 本地取值即可。
// range 是每次重算都会返回新对象的 computed，bucket 返回字符串，引用比较即可触发，无需 deep。
watch([range, bucket], scheduleSummaryFetch)
watch([activityView, monthYear, monthNumber, monthMetric], fetchMonth)
watch(
  () => [
    store.settings.dayBoundaryMode,
    store.settings.sourceAware.activeSourceFilter,
    store.settings.clientTools.activeToolFilter
  ],
  () => {
    fetchSummary()
    fetchMonth()
  }
)

onMounted(async () => {
  // 先注册事件监听再发首次请求：物化完成事件可能在首次 fetch 返回后极短时间内 emit，
  // 若 listen 晚于 fetch 注册，事件会丢失导致页面停留在全零快照。
  try {
    unlistenActivityMaterialization = await listen<ActivityMaterializationDone>(
      'activity_materialization_done',
      event => handleActivityMaterializationDone(event.payload)
    )
  } catch (error) {
    // 事件监听失败不影响主流程（正常请求路径仍可用）
    console.error('Failed to listen activity_materialization_done:', error)
  }
  await Promise.all([fetchSummary(), fetchMonth()])
  initialized.value = true
})

onUnmounted(() => {
  if (customRangeTimer) {
    clearTimeout(customRangeTimer)
    customRangeTimer = null
  }
  unlistenActivityMaterialization?.()
  unlistenActivityMaterialization = null
})

</script>

<template>
  <div class="space-y-3 pb-2 animate-in fade-in zoom-in-95 duration-300">
    <div v-if="store.statisticsError" class="rounded-xl border border-rose-100 bg-rose-50 p-2.5 text-[11px] font-medium text-rose-700 dark:border-rose-900/40 dark:bg-rose-950/30 dark:text-rose-300">
      {{ backendErrorLabel(locale, store.statisticsError) }}
    </div>

    <ActivityGrid
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
      @set-view="setActivityView"
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
