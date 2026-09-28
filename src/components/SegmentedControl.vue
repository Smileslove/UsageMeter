<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'

const props = withDefaults(defineProps<{
  activeIndex: number
  ariaLabel?: string
  role?: 'group' | 'tablist'
  tag?: 'div' | 'nav'
  tone?: 'accent' | 'soft'
}>(), {
  role: 'group',
  tag: 'div',
  tone: 'accent'
})

const root = ref<HTMLElement | null>(null)
const indicator = ref<HTMLElement | null>(null)
let resizeObserver: ResizeObserver | undefined

function measure() {
  const container = root.value
  const highlight = indicator.value
  if (!container || !highlight) return

  const buttons = [...container.children].filter((element): element is HTMLButtonElement =>
    element instanceof HTMLButtonElement
  )
  const button = buttons[props.activeIndex]
  if (!button) {
    highlight.hidden = true
    container.dataset.ready = 'true'
    return
  }

  const containerRect = container.getBoundingClientRect()
  const buttonRect = button.getBoundingClientRect()
  const scaleX = container.offsetWidth ? containerRect.width / container.offsetWidth : 1
  const scaleY = container.offsetHeight ? containerRect.height / container.offsetHeight : 1
  const left = (buttonRect.left - containerRect.left) / scaleX - container.clientLeft
  const top = (buttonRect.top - containerRect.top) / scaleY - container.clientTop
  highlight.hidden = false
  highlight.style.width = `${buttonRect.width / scaleX}px`
  highlight.style.height = `${buttonRect.height / scaleY}px`
  highlight.style.borderRadius = getComputedStyle(button).borderRadius
  highlight.style.transform = `translate(${left}px, ${top}px)`
  container.dataset.ready = 'true'
}

onMounted(async () => {
  await nextTick()
  measure()
  if (typeof ResizeObserver !== 'undefined' && root.value) {
    resizeObserver = new ResizeObserver(measure)
    resizeObserver.observe(root.value)
    for (const child of root.value.children) {
      if (child instanceof HTMLButtonElement) resizeObserver.observe(child)
    }
  }
})

watch(() => props.activeIndex, () => void nextTick(measure), { flush: 'post' })
onBeforeUnmount(() => resizeObserver?.disconnect())
</script>

<template>
  <component
    :is="tag"
    ref="root"
    class="segmented-control"
    :role="role"
    :aria-label="ariaLabel"
    :data-tone="tone"
  >
    <span ref="indicator" class="segmented-control__indicator" aria-hidden="true"></span>
    <slot />
  </component>
</template>

<style scoped>
.segmented-control {
  position: relative;
}

.segmented-control__indicator {
  position: absolute;
  top: 0;
  left: 0;
  z-index: 0;
  box-sizing: border-box;
  border-radius: 0.375rem;
  background: var(--theme-accent-primary);
  box-shadow: 0 2px 6px color-mix(in srgb, var(--theme-accent-primary) 28%, transparent);
  pointer-events: none;
  transition: transform 220ms cubic-bezier(0.2, 0.8, 0.2, 1), width 220ms cubic-bezier(0.2, 0.8, 0.2, 1), height 220ms cubic-bezier(0.2, 0.8, 0.2, 1);
}

.segmented-control[data-tone='soft'] .segmented-control__indicator {
  background: var(--theme-accent-soft);
  box-shadow: none;
}

.segmented-control:not([data-ready='true']) .segmented-control__indicator {
  transition: none;
}

:slotted(button) {
  position: relative;
  z-index: 1;
}

@media (prefers-reduced-motion: reduce) {
  .segmented-control__indicator {
    transition: none;
  }
}
</style>
