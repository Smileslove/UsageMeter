<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { CalendarDays, Check } from 'lucide-vue-next'
import { t } from '../../i18n'
import type { AppLocale, StatisticsRangePreset } from '../../types'

const props = defineProps<{
  locale: AppLocale
  preset: StatisticsRangePreset
  customStart: string
  customEnd: string
}>()

const emit = defineEmits<{
  setPreset: [value: StatisticsRangePreset]
  setCustomStart: [value: string]
  setCustomEnd: [value: string]
}>()

const presets: Array<{ value: StatisticsRangePreset; key: string }> = [
  { value: '5h', key: 'statistics.range5h' },
  { value: 'today', key: 'statistics.rangeToday' },
  { value: '1d', key: 'statistics.range1d' },
  { value: '7d', key: 'statistics.range7d' },
  { value: '30d', key: 'statistics.range30d' },
  { value: 'current_month', key: 'statistics.rangeMonth' },
  { value: 'custom', key: 'statistics.rangeCustom' }
]

const pickerRef = ref<HTMLElement | null>(null)
const open = ref(false)

const activePreset = computed(() => presets.find(item => item.value === props.preset) ?? presets[0])
const isCustomRange = computed(() => props.preset === 'custom')
const rangeButtonLabel = computed(() => {
  if (props.preset !== 'custom') return t(props.locale, activePreset.value.key)
  if (!props.customStart || !props.customEnd) return t(props.locale, 'statistics.rangeCustom')
  return formatDateTimeRange(props.customStart, props.customEnd)
})

function formatDateTimeRange(startValue: string, endValue: string): string {
  const start = parseDateTime(startValue)
  const end = parseDateTime(endValue)
  if (!start || !end) return `${startValue}-${endValue}`

  const startDate = formatCompactDate(start)
  const endDate = formatCompactDate(end)
  const startTime = formatCompactTime(start)
  const endTime = formatCompactTime(end)
  return `${startDate} ${startTime} - ${endDate} ${endTime}`
}

function parseDateTime(value: string): Date | null {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : date
}

function formatCompactDate(date: Date): string {
  const year = String(date.getFullYear()).slice(-2)
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}/${month}/${day}`
}

function formatCompactTime(date: Date): string {
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  return `${hours}:${minutes}`
}

function togglePicker() {
  open.value = !open.value
}

function selectPreset(value: StatisticsRangePreset) {
  emit('setPreset', value)
}

function handleDocumentClick(event: MouseEvent) {
  if (!pickerRef.value?.contains(event.target as Node)) {
    open.value = false
  }
}

onMounted(() => {
  document.addEventListener('click', handleDocumentClick)
})

onBeforeUnmount(() => {
  document.removeEventListener('click', handleDocumentClick)
})
</script>

<template>
  <div class="flex items-center justify-between gap-2">
    <p class="text-[11px] font-semibold text-gray-500 dark:text-gray-400">{{ t(locale, 'statistics.rangePicker') }}</p>

    <div ref="pickerRef" class="relative min-w-0 shrink-0" :class="isCustomRange ? 'max-w-[300px]' : 'w-auto'">
      <button
        class="flex h-8 w-full items-center justify-center gap-1 rounded-full border border-gray-100 bg-gray-50 px-2 font-semibold text-gray-700 transition hover:bg-gray-100 dark:border-neutral-700 dark:bg-neutral-800 dark:text-gray-100 dark:hover:bg-neutral-700"
        :class="isCustomRange ? 'text-[11px]' : 'text-[11px]'"
        :title="rangeButtonLabel"
        @click.stop="togglePicker"
      >
        <span class="flex min-w-0 items-center gap-1">
          <CalendarDays class="h-3 w-3 shrink-0 text-gray-400 dark:text-gray-300" />
          <span class="truncate">{{ rangeButtonLabel }}</span>
        </span>
      </button>

      <div
        v-if="open"
        class="absolute right-0 top-9 z-30 w-[300px] rounded-2xl border border-gray-200 bg-white p-2 shadow-[0_12px_28px_rgba(0,0,0,0.10)] dark:border-neutral-600 dark:bg-[#242529] dark:shadow-[0_12px_28px_rgba(0,0,0,0.34)]"
        @click.stop
      >
        <div class="grid grid-cols-4 gap-1">
          <button
            v-for="item in presets"
            :key="item.value"
            class="flex h-6 min-w-0 items-center justify-center gap-0.5 rounded-lg border px-1 text-[10px] font-semibold transition"
            :class="preset === item.value ? 'border-blue-200 bg-blue-50 text-blue-600 dark:border-blue-400/30 dark:bg-blue-500/15 dark:text-blue-300' : 'border-gray-100 bg-gray-50 text-gray-500 hover:border-gray-200 hover:bg-white hover:text-gray-800 dark:border-neutral-700 dark:bg-neutral-800/80 dark:text-gray-400 dark:hover:border-neutral-600 dark:hover:bg-neutral-700/80 dark:hover:text-gray-100'"
            @click="selectPreset(item.value)"
          >
            <Check v-if="preset === item.value" class="h-3 w-3 shrink-0" />
            <span class="truncate">{{ t(locale, item.key) }}</span>
          </button>
        </div>

        <div class="mt-1.5 grid grid-cols-2 gap-1.5 border-t border-gray-100 pt-1.5 dark:border-neutral-700">
          <label class="flex min-w-0 flex-col gap-1 rounded-xl bg-gray-50 px-2 py-1.5 text-[10px] text-gray-500 dark:bg-neutral-800 dark:text-gray-400">
            <span class="flex items-center gap-1">
              <CalendarDays class="h-3.5 w-3.5" />
              {{ t(locale, 'statistics.customStart') }}
            </span>
            <input class="min-w-0 bg-transparent text-[10px] text-gray-800 outline-none dark:text-gray-100" type="datetime-local" :value="customStart" @input="emit('setCustomStart', ($event.target as HTMLInputElement).value)" />
          </label>
          <label class="flex min-w-0 flex-col gap-1 rounded-xl bg-gray-50 px-2 py-1.5 text-[10px] text-gray-500 dark:bg-neutral-800 dark:text-gray-400">
            <span class="flex items-center gap-1">
              <CalendarDays class="h-3.5 w-3.5" />
              {{ t(locale, 'statistics.customEnd') }}
            </span>
            <input class="min-w-0 bg-transparent text-[10px] text-gray-800 outline-none dark:text-gray-100" type="datetime-local" :value="customEnd" @input="emit('setCustomEnd', ($event.target as HTMLInputElement).value)" />
          </label>
        </div>
      </div>
    </div>
  </div>
</template>
