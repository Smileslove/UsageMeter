<script setup lang="ts">
import { computed } from 'vue'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'

const nav = useDesktopNavigationStore()
const store = useMonitorStore()
const locale = computed(() => store.settings.locale)
</script>

<template>
  <section class="flex flex-col gap-4">
    <div
      class="flex flex-col items-center justify-center rounded-lg border border-dashed border-[var(--theme-border-strong)] px-6 py-16 text-center"
    >
      <h2 class="text-[15px] font-semibold text-[var(--theme-text-primary)]">
        {{ t(locale, 'desktop.nav.activity') }}
      </h2>
      <p class="mt-3 text-sm font-medium text-[var(--theme-text-secondary)]">
        {{ t(locale, 'desktop.activityNotAvailable') }}
      </p>
      <p class="mt-1.5 max-w-md text-sm leading-6 text-[var(--theme-text-tertiary)]">
        {{ t(locale, 'desktop.activityNotAvailableDesc') }}
      </p>
      <button
        type="button"
        class="theme-button-secondary mt-5 rounded-lg px-4 py-2 text-sm font-medium transition-colors duration-150"
        @click="nav.navigate('settings')"
      >
        {{ t(locale, 'desktop.manageDataSources') }}
      </button>
    </div>
  </section>
</template>
