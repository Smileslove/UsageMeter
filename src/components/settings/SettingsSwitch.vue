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
      'relative flex h-6 w-10 shrink-0 items-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-50',
      checked ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
    ]"
    @click="handleClick"
  >
    <span
      :class="[
        'absolute h-[20px] w-[20px] rounded-full bg-white shadow shadow-black/10 transition-all',
        checked ? 'right-[2px]' : 'left-[2px]'
      ]"
    ></span>
  </button>
</template>
