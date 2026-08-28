<script setup lang="ts">
/**
 * 状态徽标：统一 inline-flex rounded-full border 的标签渲染。
 * 替代 DesktopRequests / SessionWorkspace / PerformanceView 等处重复的状态徽标模板。
 */
withDefaults(defineProps<{
  label: string
  cls?: string
}>(), {
  cls: '',
})
</script>

<template>
  <span class="inline-flex items-center rounded-full border px-1.5 py-px text-xs font-bold leading-none" :class="cls">{{ label }}</span>
</template>
