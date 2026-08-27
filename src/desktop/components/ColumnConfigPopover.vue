<script setup lang="ts" generic="K extends string">
/**
 * 列配置浮层：齿轮按钮 + checkbox 列表。
 * 替代 DesktopRequests / DesktopSessions 中逐字重复的列配置 UI。
 * 自带外部点击关闭（修复原实现缺失此功能的问题）。
 */
import { onMounted, onUnmounted, ref } from 'vue'
import { Settings2 } from 'lucide-vue-next'
import { t } from '../../i18n'

export interface ColumnConfigItem<K extends string> {
  key: K
  defaultVisible: boolean
  labelKey?: string
}

defineProps<{
  columns: ColumnConfigItem<K>[]
  isVisible: (key: K) => boolean
  toggleColumn: (key: K) => void
  buttonLabel: string
  titleLabel: string
  labelPrefix: string
  locale: string
}>()

const show = ref(false)
const triggerRef = ref<HTMLElement | null>(null)
const panelRef = ref<HTMLElement | null>(null)

function toggle() {
  show.value = !show.value
}

function onPointerDown(e: PointerEvent) {
  if (panelRef.value?.contains(e.target as Node)) return
  if (triggerRef.value?.contains(e.target as Node)) return
  show.value = false
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && show.value) show.value = false
}

onMounted(() => {
  document.addEventListener('pointerdown', onPointerDown, true)
  document.addEventListener('keydown', onKeydown)
})

onUnmounted(() => {
  document.removeEventListener('pointerdown', onPointerDown, true)
  document.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <div ref="triggerRef" class="relative">
    <button
      type="button"
      class="inline-flex h-7 items-center gap-1 rounded-lg border border-[var(--theme-border-default)] px-2 text-xs font-medium text-[var(--theme-text-secondary)] transition-colors hover:border-[var(--theme-accent-primary)] hover:text-[var(--theme-accent-primary)]"
      :aria-label="buttonLabel"
      :title="buttonLabel"
      @click="toggle"
    >
      <Settings2 class="h-3 w-3" aria-hidden="true" />
      <span class="hidden sm:inline">{{ buttonLabel }}</span>
    </button>
    <Transition name="popover">
      <div
        v-if="show"
        ref="panelRef"
        class="theme-surface-elevated absolute right-0 top-8 z-30 w-40 rounded-lg border p-2 shadow-lg"
        :aria-label="titleLabel"
      >
        <p class="mb-1.5 px-1 text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ titleLabel }}</p>
        <label
          v-for="col in columns"
          :key="col.key"
          class="flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 text-xs text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
        >
          <input
            type="checkbox"
            class="h-3 w-3 accent-[var(--theme-accent-primary)]"
            :checked="isVisible(col.key)"
            @change="toggleColumn(col.key)"
          />
          <span>{{ t(locale, col.labelKey ?? `${labelPrefix}${col.key.charAt(0).toUpperCase()}${col.key.slice(1)}`) }}</span>
        </label>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.popover-enter-active,
.popover-leave-active {
  transition: opacity 0.12s ease-out, transform 0.12s ease-out;
}
.popover-enter-from,
.popover-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
</style>
