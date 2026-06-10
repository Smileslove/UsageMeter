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
import ConfirmDialog from '../components/settings/ConfirmDialog.vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { quitApplication } from '../utils/appExit'

const store = useMonitorStore()

const subView = ref<'main' | 'model-pricing' | 'api-sources' | 'currency'>('main')
const modelPricingKey = ref(0)
const quitDialogOpen = ref(false)
const quitBusy = ref(false)
const quitFailed = ref(false)

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

const openQuitDialog = () => {
  quitFailed.value = false
  quitDialogOpen.value = true
}

const closeQuitDialog = () => {
  if (quitBusy.value) return
  quitDialogOpen.value = false
}

const confirmQuit = async () => {
  if (quitBusy.value) return
  quitBusy.value = true
  quitFailed.value = false

  try {
    await quitApplication(store)
  } catch (error) {
    console.error('[Settings] Failed to quit app:', error)
    quitFailed.value = true
  } finally {
    quitBusy.value = false
  }
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

      <section class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">
          {{ t(store.settings.locale, 'settings.appActionsSection') }}
        </h3>
        <div class="overflow-hidden rounded-xl border border-red-100/80 bg-white shadow-sm dark:border-red-500/20 dark:bg-[#1C1C1E]">
          <div class="flex items-center justify-between gap-3 px-4 py-3">
            <div class="min-w-0">
              <div class="text-[11px] font-medium text-gray-800 dark:text-gray-100">
                {{ t(store.settings.locale, 'settings.quitApp') }}
              </div>
              <div class="mt-0.5 text-[10px] leading-relaxed text-gray-400 dark:text-gray-500">
                {{ t(store.settings.locale, 'settings.quitAppDesc') }}
              </div>
            </div>
            <button
              class="shrink-0 rounded-xl border border-red-200 bg-red-50 px-3 py-1.5 text-[11px] font-semibold text-red-600 transition-colors hover:bg-red-100 disabled:cursor-not-allowed disabled:opacity-60 dark:border-red-500/20 dark:bg-red-500/10 dark:text-red-300 dark:hover:bg-red-500/15"
              :disabled="quitBusy"
              @click="openQuitDialog"
            >
              {{ t(store.settings.locale, 'settings.quitApp') }}
            </button>
          </div>
        </div>
        <div v-if="quitFailed" class="px-1 text-xs text-red-500">
          {{ t(store.settings.locale, 'settings.quitAppFailed') }}
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

  <ConfirmDialog
    :open="quitDialogOpen"
    :title="t(store.settings.locale, 'settings.quitAppConfirmTitle')"
    :body="t(store.settings.locale, 'settings.quitAppConfirmBody')"
    :confirm-label="t(store.settings.locale, 'settings.quitApp')"
    :cancel-label="t(store.settings.locale, 'common.cancel')"
    :busy="quitBusy"
    tone="danger"
    @cancel="closeQuitDialog"
    @confirm="confirmQuit"
  />
</template>
