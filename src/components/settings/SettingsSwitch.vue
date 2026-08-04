<script setup lang="ts">
const props = defineProps<{
  checked: boolean
  disabled?: boolean
  compact?: boolean
}>()

const emit = defineEmits<{
  (e: 'toggle'): void
}>()

const handleClick = () => {
  if (props.disabled) {
    return
  }
  emit('toggle')
}
</script>

<template>
  <button
    type="button"
    :disabled="disabled"
    :aria-pressed="checked"
    :class="[
      'theme-switch relative flex shrink-0 items-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-50',
      props.compact ? 'h-5 w-8' : 'h-6 w-10',
      checked ? 'theme-switch--checked' : 'theme-switch--unchecked'
    ]"
    @click="handleClick"
  >
    <span
      :class="[
        'absolute rounded-full transition-all theme-switch__thumb',
        props.compact ? 'h-4 w-4' : 'h-5 w-5',
        checked ? (props.compact ? 'right-0.5' : 'right-[2px]') : (props.compact ? 'left-0.5' : 'left-[2px]')
      ]"
    ></span>
  </button>
</template>

<style scoped>
.theme-switch--checked {
  background: var(--theme-accent-primary);
}

.theme-switch--unchecked {
  background: var(--theme-border-strong);
}

.theme-switch__thumb {
  background: var(--theme-bg-elevated);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.14);
}
</style>
