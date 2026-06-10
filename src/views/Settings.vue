<script setup lang="ts">
import { ref, watch } from 'vue'
import ModelPricingSettings from '../components/ModelPricingSettings.vue'
import ApiSourceList from '../components/ApiSourceList.vue'
import CurrencySettings from '../components/CurrencySettings.vue'
import GeneralSettingsPanel from '../components/settings/GeneralSettingsPanel.vue'
import DataNavigationPanel from '../components/settings/DataNavigationPanel.vue'
import LocalCachePanel from '../components/settings/LocalCachePanel.vue'
import LocalCacheManagementPanel from '../components/settings/LocalCacheManagementPanel.vue'
import ProxyControlPanel from '../components/settings/ProxyControlPanel.vue'
import NetworkProxyPanel from '../components/settings/NetworkProxyPanel.vue'
import SyncSettingsPanel from '../components/settings/SyncSettingsPanel.vue'
import WslScanPanel from '../components/settings/WslScanPanel.vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

const store = useMonitorStore()

const subView = ref<'main' | 'model-pricing' | 'api-sources' | 'currency'>('main')
const modelPricingKey = ref(0)

watch(subView, (newVal) => {
  if (newVal === 'model-pricing') {
    modelPricingKey.value++
  }
})

const goBack = () => {
  subView.value = 'main'
}

const openModelPricing = () => {
  subView.value = 'model-pricing'
}

const openApiSources = () => {
  subView.value = 'api-sources'
}

const openCurrency = () => {
  subView.value = 'currency'
}
</script>

<template>
  <div class="relative">
    <ModelPricingSettings
      v-show="subView === 'model-pricing'"
      :key="modelPricingKey"
      @back="goBack"
    />

    <ApiSourceList
      v-show="subView === 'api-sources'"
      :onBack="goBack"
    />

    <CurrencySettings
      v-show="subView === 'currency'"
      @back="goBack"
    />

    <div v-show="subView === 'main'" class="space-y-4 animate-in fade-in zoom-in-95 duration-300 pb-4">
      <section class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">
          {{ t(store.settings.locale, 'settings.appSection') }}
        </h3>
        <GeneralSettingsPanel />
      </section>

      <section class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">
          {{ t(store.settings.locale, 'settings.dataPipelineSection') }}
        </h3>
        <div class="space-y-2">
          <ProxyControlPanel />
          <div class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm dark:border-neutral-800 dark:bg-[#1C1C1E]">
            <LocalCachePanel />
          </div>
          <div class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm divide-y divide-gray-50 dark:border-neutral-800 dark:bg-[#1C1C1E] dark:divide-neutral-800/50">
            <SyncSettingsPanel />
            <LocalCacheManagementPanel />
            <WslScanPanel />
          </div>
        </div>
      </section>

      <section class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">
          {{ t(store.settings.locale, 'settings.pricingAndSourcesSection') }}
        </h3>
        <DataNavigationPanel
          @open-api-sources="openApiSources"
          @open-model-pricing="openModelPricing"
          @open-currency="openCurrency"
        />
      </section>


      <section class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">
          {{ t(store.settings.locale, 'settings.networkSection') }}
        </h3>
        <div class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm divide-y divide-gray-50 dark:border-neutral-800 dark:bg-[#1C1C1E] dark:divide-neutral-800/50">
          <NetworkProxyPanel />
        </div>
      </section>

      <div v-if="store.saving" class="text-center text-xs text-gray-400">
        {{ t(store.settings.locale, 'common.saving') }}
      </div>
      <div v-if="store.error" class="text-center text-xs text-red-500">
        {{ store.error }}
      </div>
    </div>
  </div>
</template>
