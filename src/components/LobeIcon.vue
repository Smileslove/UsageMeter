<script setup lang="ts">
import { computed } from 'vue'
import { COLOR_ICON_SLUGS } from '../iconConfig'
import reasonixIconUrl from '../assets/tool-icons/reasonix.svg'
import qoderCnIconUrl from '../assets/tool-icons/qoder-cn.png'
import qoderworkIconUrl from '../assets/tool-icons/qoderwork.png'
import qoderworkCnIconUrl from '../assets/tool-icons/qoderwork-cn.png'

const CDN = 'https://unpkg.com/@lobehub/icons-static-svg@1.87.0/icons'

/** slug → 本地资源 URL（不走 Lobe CDN 的图标） */
const LOCAL_ICONS: Record<string, string> = {
  reasonix: reasonixIconUrl,
  'qoder-cn': qoderCnIconUrl,
  qoderwork: qoderworkIconUrl,
  'qoderwork-cn': qoderworkCnIconUrl,
}

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
  if (LOCAL_ICONS[props.slug]) {
    return LOCAL_ICONS[props.slug]
  }
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
