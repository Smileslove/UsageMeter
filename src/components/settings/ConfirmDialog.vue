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
      class="theme-modal-backdrop fixed inset-0 z-[90] flex items-center justify-center"
      @click.self="emit('cancel')"
    >
      <div class="theme-modal-shell max-w-[360px]" role="dialog" aria-modal="true" @click.stop>
        <div class="theme-modal-body !overflow-visible">
          <h3 class="text-sm font-semibold text-[var(--theme-text-primary)]">
            {{ title }}
          </h3>
          <p class="mt-2 text-xs leading-relaxed text-[var(--theme-text-secondary)]">
            {{ body }}
          </p>
        </div>
        <div class="theme-modal-actions !p-0">
          <button
            class="theme-button-secondary min-h-11 flex-1 rounded-none border-0 text-xs font-medium shadow-none"
            :disabled="busy"
            @click="emit('cancel')"
          >
            {{ cancelLabel }}
          </button>
          <button
            :class="[
              'min-h-11 flex-1 border-l text-xs font-medium transition-colors disabled:opacity-50',
              tone === 'warning'
                ? 'border-[var(--theme-border-subtle)] text-amber-600 hover:bg-amber-500/10 dark:text-amber-400'
                : 'border-[var(--theme-border-subtle)] text-red-600 hover:bg-red-500/10 dark:text-red-400'
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
