<script setup lang="ts">
/**
 * 搜索输入框：Search 图标 + theme-input + placeholder。
 * 替代 DesktopRequests / DesktopSessions / DesktopProjects 等处重复的搜索框模板。
 */
import { Search } from 'lucide-vue-next'

withDefaults(defineProps<{
  modelValue: string
  placeholder?: string
  ariaLabel?: string
  width?: string
  disabled?: boolean
}>(), {
  width: 'w-44',
  disabled: false,
})

defineEmits<{
  'update:modelValue': [value: string]
}>()
</script>

<template>
  <div class="relative">
    <Search class="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
    <input
      :value="modelValue"
      type="text"
      class="theme-input h-7 rounded-lg pl-7 pr-2 text-xs outline-none"
      :class="width"
      :placeholder="placeholder"
      :aria-label="ariaLabel ?? placeholder"
      :disabled="disabled"
      @input="$emit('update:modelValue', ($event.target as HTMLInputElement).value)"
    />
  </div>
</template>
