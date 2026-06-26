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
  const component = icons[key]
  if (!component && import.meta.env.DEV) {
    console.warn(`[DynamicIcon] Unknown icon: ${props.name}`)
  }
  const resolvedComponent = (component ?? icons.Globe) as typeof icons.Globe
  return h(resolvedComponent, { size: props.size || 16, color: props.color })
})
</script>

<template>
  <iconVNode />
</template>
