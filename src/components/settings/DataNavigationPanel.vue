<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'

const store = useMonitorStore()

const emit = defineEmits<{
  (e: 'open-api-sources'): void
  (e: 'open-model-pricing'): void
  (e: 'open-currency'): void
}>()

const unnamedSourceCount = computed(() => {
  return store.settings.sourceAware.sources.filter(source => source.autoDetected && !source.displayName).length
})

const sourceSummary = computed(() => {
  if (store.settings.sourceAware.sources.length > 0) {
    return `${store.settings.sourceAware.sources.length} ${t(store.settings.locale, 'sources.sourcesCount')}`
  }
  return t(store.settings.locale, 'sources.noSources')
})
</script>

<template>
  <div class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm divide-y divide-gray-50 dark:border-neutral-800 dark:bg-[#1C1C1E] dark:divide-neutral-800/50">
    <div
      class="cursor-pointer py-2 px-4 transition-colors hover:bg-gray-50 dark:hover:bg-neutral-800/50"
      @click="emit('open-api-sources')"
    >
      <div class="flex items-center justify-between">
        <div>
          <div class="flex items-center gap-2">
            <div class="text-[13px] text-gray-700 dark:text-gray-200">
              {{ t(store.settings.locale, 'sources.manage') }}
            </div>
            <span
              v-if="unnamedSourceCount > 0"
              class="rounded-full bg-red-100 px-1.5 py-0.5 text-[10px] font-medium text-red-600 dark:bg-red-500/20 dark:text-red-400"
            >
              {{ unnamedSourceCount }}
            </span>
          </div>
          <div class="text-[10px] text-gray-400">
            {{ sourceSummary }}
          </div>
        </div>
        <svg class="h-4 w-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
        </svg>
      </div>
    </div>

    <div
      class="cursor-pointer py-2 px-4 transition-colors hover:bg-gray-50 dark:hover:bg-neutral-800/50"
      @click="emit('open-model-pricing')"
    >
      <div class="flex items-center justify-between">
        <div>
          <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.modelPricing') }}</div>
          <div class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.modelPricingDesc') }}</div>
        </div>
        <svg class="h-4 w-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
        </svg>
      </div>
    </div>

    <div
      class="cursor-pointer py-2 px-4 transition-colors hover:bg-gray-50 dark:hover:bg-neutral-800/50"
      @click="emit('open-currency')"
    >
      <div class="flex items-center justify-between">
        <div>
          <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.currency') }}</div>
          <div class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.currencyDesc') }}</div>
        </div>
        <svg class="h-4 w-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
        </svg>
      </div>
    </div>
  </div>
</template>
