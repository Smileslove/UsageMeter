<script setup lang="ts">
/**
 * 空状态卡片：图标 + 标题 + 描述 + 可选操作按钮。
 * 替代 DesktopActivity / ActivityTimeline / SessionWorkspace 等处重复的空状态模板。
 */
import type { Component } from 'vue'

defineProps<{
  icon?: Component
  title?: string
  description?: string
  actionLabel?: string
}>()

defineEmits<{
  action: []
}>()
</script>

<template>
  <div class="flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center">
    <component v-if="icon" :is="icon" class="h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
    <p v-if="title" class="mt-2 text-xs font-semibold text-[var(--theme-text-secondary)]">{{ title }}</p>
    <p v-if="description" class="mx-auto mt-1 max-w-sm text-xs leading-snug text-[var(--theme-text-tertiary)]">{{ description }}</p>
    <slot name="action" />
  </div>
</template>
