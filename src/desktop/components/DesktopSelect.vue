<script setup lang="ts">
/**
 * 自定义下拉选择组件：替代原生 <select>，提供统一的桌面级视觉。
 * 触发器显示当前选中项标签 + ChevronDown 图标；
 * 弹出层为 theme-surface-elevated 浮层，点击外部/Escape 关闭。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { ChevronDown, Check } from 'lucide-vue-next'

export interface SelectOption {
  value: string | number | null
  label: string
}

const props = withDefaults(defineProps<{
  modelValue: string | number | null
  options: SelectOption[]
  ariaLabel?: string
  disabled?: boolean
  compact?: boolean
}>(), {
  disabled: false,
  compact: false,
})

const emit = defineEmits<{
  'update:modelValue': [value: string | number | null]
  change: [value: string | number | null]
}>()

const open = ref(false)
const triggerRef = ref<HTMLElement | null>(null)
const panelRef = ref<HTMLElement | null>(null)

const selectedLabel = computed(() => {
  const found = props.options.find(o => o.value === props.modelValue)
  return found?.label ?? ''
})

function toggle() {
  if (props.disabled) return
  open.value = !open.value
}

function select(value: string | number | null) {
  emit('update:modelValue', value)
  emit('change', value)
  open.value = false
}

function onPointerDown(e: PointerEvent) {
  if (panelRef.value?.contains(e.target as Node)) return
  if (triggerRef.value?.contains(e.target as Node)) return
  open.value = false
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && open.value) {
    open.value = false
  }
}

onMounted(() => {
  document.addEventListener('pointerdown', onPointerDown, true)
  document.addEventListener('keydown', onKeydown)
})

onUnmounted(() => {
  document.removeEventListener('pointerdown', onPointerDown, true)
  document.removeEventListener('keydown', onKeydown)
})

watch(open, isOpen => {
  if (isOpen) {
    requestAnimationFrame(() => {
      const panel = panelRef.value
      if (!panel) return
      const rect = panel.getBoundingClientRect()
      const margin = 8
      if (rect.right > window.innerWidth - margin) {
        panel.style.left = 'auto'
        panel.style.right = '0'
      }
      if (rect.bottom > window.innerHeight - margin) {
        panel.style.top = 'auto'
        panel.style.bottom = '100%'
      }
    })
  }
})
</script>

<template>
  <div class="relative">
    <button
      ref="triggerRef"
      type="button"
      class="theme-input inline-flex items-center justify-between gap-1 rounded-lg text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
      :class="[
        compact ? 'h-6 px-1.5' : 'h-7 px-2',
        disabled ? 'cursor-not-allowed opacity-50' : 'hover:border-[var(--theme-accent-primary)]'
      ]"
      :aria-label="ariaLabel"
      :aria-expanded="open"
      :disabled="disabled"
      @click="toggle"
    >
      <span class="truncate text-xs font-medium text-[var(--theme-text-secondary)]">{{ selectedLabel }}</span>
      <ChevronDown
        class="shrink-0 transition-transform duration-150"
        :class="[open ? 'rotate-180' : '', compact ? 'h-2.5 w-2.5' : 'h-3 w-3']"
        :style="{ color: 'var(--theme-text-quaternary)' }"
        aria-hidden="true"
      />
    </button>

    <Transition name="select-pop">
      <div
        v-if="open"
        ref="panelRef"
        class="theme-surface-elevated absolute left-0 top-[calc(100%+4px)] z-50 max-h-60 min-w-full overflow-y-auto rounded-lg border p-1 shadow-lg"
        role="listbox"
      >
        <button
          v-for="opt in options"
          :key="String(opt.value)"
          type="button"
          role="option"
          class="flex w-full items-center justify-between gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors"
          :class="opt.value === modelValue
            ? 'bg-[var(--theme-accent-soft)] font-semibold text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]'"
          :aria-selected="opt.value === modelValue"
          @click="select(opt.value)"
        >
          <span class="truncate">{{ opt.label }}</span>
          <Check
            v-if="opt.value === modelValue"
            class="shrink-0"
            :class="compact ? 'h-2.5 w-2.5' : 'h-3 w-3'"
            aria-hidden="true"
          />
        </button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.select-pop-enter-active,
.select-pop-leave-active {
  transition: opacity 0.12s ease-out, transform 0.12s ease-out;
}
.select-pop-enter-from,
.select-pop-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
</style>
