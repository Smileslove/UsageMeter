<script setup lang="ts">
import { computed, h } from 'vue'
import * as icons from 'lucide-vue-next'

const props = defineProps<{
  name: string
  size?: number
  color?: string
}>()

const iconVNode = computed(() => {
  const key = props.name as keyof typeof icons
  const component = (icons[key] || icons.Globe) as any
  return h(component, { size: props.size || 16, color: props.color })
})
</script>

<template>
  <iconVNode />
</template>
