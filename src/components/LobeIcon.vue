<script setup lang="ts">
import { computed } from 'vue'
import { COLOR_ICON_SLUGS } from '../iconConfig'

const CDN = 'https://unpkg.com/@lobehub/icons-static-svg@1.87.0/icons'

const props = withDefaults(defineProps<{
  slug: string
  size?: number
}>(), {
  size: 20,
})

const emit = defineEmits<{
  error: []
}>()

const src = computed(() => {
  const variant = COLOR_ICON_SLUGS.has(props.slug) ? `${props.slug}-color` : props.slug
  return `${CDN}/${variant}.svg`
})
</script>

<template>
  <img
    :src="src"
    :width="size"
    :height="size"
    class="shrink-0 select-none align-middle"
    :alt="slug"
    loading="lazy"
    @error="emit('error')"
  />
</template>
