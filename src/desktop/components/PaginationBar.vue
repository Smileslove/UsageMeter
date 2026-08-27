<script setup lang="ts">
/**
 * 通用分页控件：首页/上页/页码序列/下页/末页 + 每页大小选择 + 跳转输入。
 * 替代 DesktopRequests / DesktopSessions 中逐字重复的分页 UI。
 */
import { computed, ref } from 'vue'
import { ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight } from 'lucide-vue-next'
import DesktopSelect from './DesktopSelect.vue'
import type { SelectOption } from './DesktopSelect.vue'

const props = withDefaults(defineProps<{
  currentPage: number
  totalPages: number
  total: number
  pageNumbers: (number | '...')[]
  hasPrev: boolean
  hasNext: boolean
  pageSize: number
  pageSizeOptions: SelectOption[]
  loading?: boolean
  pageSizeLabel: string
  pageFirstLabel: string
  pagePrevLabel: string
  pageNextLabel: string
  pageLastLabel: string
  jumpToLabel: string
  pageOfLabel: string
}>(), {
  loading: false,
})

const emit = defineEmits<{
  goto: [page: number]
  next: []
  prev: []
  'update:pageSize': [size: number]
}>()

const jumpPageInput = ref('')

function gotoPageNumber(num: number | '...') {
  if (num === '...') return
  emit('goto', num - 1)
}

function jumpPage() {
  const num = parseInt(jumpPageInput.value, 10)
  if (Number.isFinite(num) && num >= 1 && num <= props.totalPages) {
    emit('goto', num - 1)
  }
  jumpPageInput.value = ''
}

function onPageSizeChange(value: string | number | null) {
  if (typeof value === 'number') emit('update:pageSize', value)
}

const pageStart = computed(() =>
  props.total === 0 ? 0 : props.currentPage * props.pageSize + 1,
)
const pageEnd = computed(() =>
  Math.min((props.currentPage + 1) * props.pageSize, props.total),
)
</script>

<template>
  <div class="flex shrink-0 items-center justify-between gap-3 text-xs text-[var(--theme-text-tertiary)]">
    <div class="flex shrink-0 items-center gap-2">
      <span>{{ pageStart }}–{{ pageEnd }} / {{ total }}</span>
      <DesktopSelect
        :model-value="pageSize"
        :options="pageSizeOptions"
        :aria-label="pageSizeLabel"
        compact
        @change="onPageSizeChange"
      />
    </div>
    <div class="flex items-center gap-2">
      <div class="flex items-center gap-1">
        <button
          type="button"
          class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
          :disabled="!hasPrev || loading"
          :aria-label="pageFirstLabel"
          :title="pageFirstLabel"
          @click="emit('goto', 0)"
        >
          <ChevronsLeft class="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
          :disabled="!hasPrev || loading"
          :aria-label="pagePrevLabel"
          @click="emit('prev')"
        >
          <ChevronLeft class="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <template v-for="(num, idx) in pageNumbers" :key="idx">
          <span v-if="num === '...'" class="px-1 text-[var(--theme-text-quaternary)]">…</span>
          <button
            v-else
            type="button"
            class="inline-flex h-7 min-w-[28px] items-center justify-center rounded-lg border px-1.5 font-mono text-xs transition-colors"
            :class="num === currentPage + 1
              ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-soft)] font-semibold text-[var(--theme-accent-primary)]'
              : 'border-[var(--theme-border-default)] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]'"
            :disabled="loading"
            :aria-current="num === currentPage + 1 ? 'page' : undefined"
            @click="gotoPageNumber(num)"
          >{{ num }}</button>
        </template>
        <button
          type="button"
          class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
          :disabled="!hasNext || loading"
          :aria-label="pageNextLabel"
          @click="emit('next')"
        >
          <ChevronRight class="h-3.5 w-3.5" aria-hidden="true" />
        </button>
        <button
          type="button"
          class="inline-flex h-7 w-7 items-center justify-center rounded-lg border border-[var(--theme-border-default)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:cursor-not-allowed disabled:opacity-40"
          :disabled="!hasNext || loading"
          :aria-label="pageLastLabel"
          :title="pageLastLabel"
          @click="emit('goto', totalPages - 1)"
        >
          <ChevronsRight class="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      </div>
      <div v-if="totalPages > 7" class="flex items-center gap-1">
        <span class="text-[var(--theme-text-quaternary)]">{{ jumpToLabel }}</span>
        <input
          v-model="jumpPageInput"
          type="number"
          min="1"
          :max="totalPages"
          class="theme-input h-7 w-12 rounded-lg px-1 text-center font-mono text-xs outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
          :aria-label="jumpToLabel"
          @keydown.enter="jumpPage"
        />
        <span class="text-[var(--theme-text-quaternary)]">{{ pageOfLabel }}</span>
      </div>
      <slot name="extra" />
    </div>
  </div>
</template>
