<script setup lang="ts">
/**
 * 覆盖式抽屉外壳：Transition + 遮罩 + aside 容器 + ESC/遮罩关闭 + CSS 过渡。
 * 替代 DesktopSessions / DesktopRequests 中逐字重复的抽屉模板与 CSS。
 */
withDefaults(defineProps<{
  open: boolean
  width?: string
  ariaLabel?: string
}>(), {
  width: 'w-80',
})

const emit = defineEmits<{
  close: []
}>()
</script>

<template>
  <Transition name="drawer-overlay">
    <div v-if="open" class="absolute inset-0 z-20 flex justify-end" @click.self="emit('close')">
      <aside
        class="theme-surface-elevated flex h-full flex-col overflow-y-auto rounded-xl border shadow-[0_4px_24px_rgba(0,0,0,0.08)]"
        :class="width"
        :aria-label="ariaLabel"
      >
        <slot />
      </aside>
    </div>
  </Transition>
</template>

<style scoped>
.drawer-overlay-enter-active,
.drawer-overlay-leave-active {
  transition: opacity 0.15s ease-out;
}
.drawer-overlay-enter-active > aside,
.drawer-overlay-leave-active > aside {
  transition: transform 0.18s ease-out;
}
.drawer-overlay-enter-from,
.drawer-overlay-leave-to {
  opacity: 0;
}
.drawer-overlay-enter-from > aside,
.drawer-overlay-leave-to > aside {
  transform: translateX(100%);
}
</style>
