<script setup lang="ts">
defineProps<{
  open: boolean
  title: string
  body: string
  confirmLabel: string
  cancelLabel: string
  busy?: boolean
  tone?: 'danger' | 'warning'
}>()

const emit = defineEmits<{
  (e: 'cancel'): void
  (e: 'confirm'): void
}>()
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4"
      @click.self="emit('cancel')"
    >
      <div class="w-[320px] overflow-hidden rounded-2xl border border-white/70 bg-white shadow-xl dark:border-neutral-800 dark:bg-[#1C1C1E]" @click.stop>
        <div class="p-4">
          <h3 class="text-sm font-semibold text-gray-900 dark:text-gray-100">
            {{ title }}
          </h3>
          <p class="mt-2 text-xs leading-relaxed text-gray-500 dark:text-gray-400">
            {{ body }}
          </p>
        </div>
        <div class="flex border-t border-gray-100 dark:border-neutral-800">
          <button
            class="flex-1 py-2.5 text-xs font-medium text-gray-600 transition-colors hover:bg-gray-50 dark:text-gray-400 dark:hover:bg-neutral-800"
            :disabled="busy"
            @click="emit('cancel')"
          >
            {{ cancelLabel }}
          </button>
          <button
            :class="[
              'flex-1 border-l py-2.5 text-xs font-medium transition-colors disabled:opacity-50 dark:border-neutral-800',
              tone === 'warning'
                ? 'border-gray-100 text-amber-600 hover:bg-amber-50 dark:text-amber-400 dark:hover:bg-amber-500/10'
                : 'border-gray-100 text-red-600 hover:bg-red-50 dark:text-red-400 dark:hover:bg-red-500/10'
            ]"
            :disabled="busy"
            @click="emit('confirm')"
          >
            {{ confirmLabel }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>
