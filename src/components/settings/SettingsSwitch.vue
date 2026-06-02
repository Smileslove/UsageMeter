<script setup lang="ts">
const props = defineProps<{
  checked: boolean
  disabled?: boolean
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
      'theme-switch relative flex h-6 w-10 shrink-0 items-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-50',
      checked ? 'theme-switch--checked' : 'theme-switch--unchecked'
    ]"
    @click="handleClick"
  >
    <span
      :class="[
        'absolute h-[20px] w-[20px] rounded-full transition-all theme-switch__thumb',
        checked ? 'right-[2px]' : 'left-[2px]'
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
