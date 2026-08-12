<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)
</script>

<template>
  <section class="flex flex-col gap-4">
    <div
      class="flex flex-col items-center justify-center rounded-lg border border-dashed border-[var(--theme-border-strong)] px-6 py-20 text-center"
    >
      <h2 class="text-[15px] font-semibold text-[var(--theme-text-primary)]">
        {{ t(locale, 'desktop.nav.settings') }}
      </h2>
      <p class="mt-2 max-w-md text-sm leading-6 text-[var(--theme-text-secondary)]">
        {{ t(locale, 'desktop.emptyPlaceholder') }}
      </p>
    </div>
  </section>
</template>
