<script setup lang="ts">
import { ref, watch } from 'vue'
import { useUpdaterStore } from '../stores/updater'
import ModelPricingSettings from '../components/ModelPricingSettings.vue'
import ApiSourceList from '../components/ApiSourceList.vue'
import CurrencySettings from '../components/CurrencySettings.vue'
import UpdateBanner from '../components/UpdateBanner.vue'
import GeneralSettingsPanel from '../components/settings/GeneralSettingsPanel.vue'
import DataNavigationPanel from '../components/settings/DataNavigationPanel.vue'
import LocalCachePanel from '../components/settings/LocalCachePanel.vue'
import ProxyControlPanel from '../components/settings/ProxyControlPanel.vue'
import NetworkProxyPanel from '../components/settings/NetworkProxyPanel.vue'
import SyncSettingsPanel from '../components/settings/SyncSettingsPanel.vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()

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

    <div v-show="subView === 'main'" class="space-y-5 animate-in fade-in zoom-in-95 duration-300 pb-6">
      <UpdateBanner v-if="updaterStore.hasUpdate" />

      <GeneralSettingsPanel />
      <ProxyControlPanel />

      <DataNavigationPanel
        @open-api-sources="openApiSources"
        @open-model-pricing="openModelPricing"
        @open-currency="openCurrency"
      />

      <div class="space-y-2">
        <div class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm divide-y divide-gray-50 dark:border-neutral-800 dark:bg-[#1C1C1E] dark:divide-neutral-800/50">
          <NetworkProxyPanel />
          <SyncSettingsPanel />
          <LocalCachePanel />
        </div>
      </div>

      <div v-if="store.saving" class="text-center text-xs text-gray-400">
        {{ t(store.settings.locale, 'common.saving') }}
      </div>
      <div v-if="store.error" class="text-center text-xs text-red-500">
        {{ store.error }}
      </div>
    </div>
  </div>
</template>
