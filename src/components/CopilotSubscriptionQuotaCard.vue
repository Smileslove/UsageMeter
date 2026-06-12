<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { Search } from 'lucide-vue-next'
import LobeIcon from './LobeIcon.vue'
import type { CredentialStatus, QuotaTier } from '../types'

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

const quota = computed(() => store.copilotQuota?.quota)
const tiers = computed<QuotaTier[]>(() => quota.value?.tiers ?? [])
const loading = computed(() => store.copilotQuotaLoading)
const success = computed(() => store.copilotQuota?.success)
const planLabel = computed(() => quota.value?.planLabel ?? '')
const accountLabel = computed(() => quota.value?.accountLabel ?? '')
const showCard = computed(() => store.hasCopilotAuth || !!store.copilotAuthStatus?.authenticated || !!quota.value)
const displayPlanLabel = computed(() => {
  const plan = planLabel.value?.trim().toLowerCase()
  if (!plan) return ''
  const key = `copilot.plan.${plan}`
  const translated = t(locale.value, key)
  return translated === key ? planLabel.value : translated
})

const displayError = computed(() => {
  const result = store.copilotQuota
  if (!result || result.success) return ''
  const status: CredentialStatus = result.credentialStatus
  if (status === 'notConfigured') return t(locale.value, 'subscription.errorNotConfigured')
  if (status === 'expired') return t(locale.value, 'copilot.error.tokenExpired')
  if (typeof status === 'object' && 'refreshFailed' in status) return t(locale.value, 'subscription.errorRefreshFailed')
  if (typeof status === 'object' && 'queryFailed' in status) {
    if (status.queryFailed.error === 'no_subscription') {
      return t(locale.value, 'copilot.login.noSubscription')
    }
    return t(locale.value, 'copilot.error.fetchFailed')
  }
  return t(locale.value, 'subscription.errorUnknown')
})

const updatedAt = computed(() => {
  const ts = quota.value?.updatedAt
  if (!ts) return ''
  return new Date(ts).toLocaleTimeString(locale.value)
})

const premiumTier = computed(() => tiers.value.find(tier => tier.name === 'copilot_premium'))
const chatTier = computed(() => tiers.value.find(tier => tier.name === 'copilot_chat'))
const completionsTier = computed(() => tiers.value.find(tier => tier.name === 'copilot_completions'))

function getTierColorClass(utilization: number): string {
  if (utilization >= 90) return 'text-red-500 dark:text-red-400'
  if (utilization >= 70) return 'text-amber-500 dark:text-amber-400'
  return 'text-cyan-500 dark:text-cyan-400'
}

function getTierDotClass(utilization: number): string {
  if (utilization >= 90) return 'bg-red-400/80 dark:bg-red-400/70'
  if (utilization >= 70) return 'bg-amber-400/85 dark:bg-amber-400/70'
  return 'bg-cyan-400/85 dark:bg-cyan-300/70'
}

function formatTierCounter(tier?: QuotaTier): string {
  if (!tier) return '--'
  if (tier.maxValue == null || tier.remainingValue == null) {
    return t(locale.value, 'copilot.quota.unlimited')
  }
  return `${Math.max(0, Math.round(tier.remainingValue))} / ${Math.max(0, Math.round(tier.maxValue))}`
}

function formatResetDate(resetsAt?: string): string {
  if (!resetsAt) return '--'
  return t(locale.value, 'copilot.quota.resetsAt', { date: resetsAt })
}

async function refresh() {
  await store.refreshCopilotQuota()
}
</script>

<template>
  <div v-if="showCard" class="metric-card metric-card-copilot group !bg-white border-cyan-200/90 dark:!bg-[#1C1C1E] dark:border-cyan-500/15">
    <div class="flex items-stretch">
      <div class="metric-rail text-cyan-600 dark:text-cyan-300">
        <div class="metric-rail-icon text-cyan-500 dark:text-cyan-300">
          <Search class="h-3.5 w-3.5 shrink-0" />
        </div>
        <p class="writing-vertical metric-rail-title">{{ t(locale, 'subscription.usageQuery') }}</p>
      </div>

      <div class="metric-body">
        <div class="flex items-center justify-between gap-3">
          <div class="flex min-w-0 items-center gap-2">
            <div class="h-6 w-6 flex items-center justify-center">
              <LobeIcon slug="githubcopilot" :size="18" class="text-cyan-600 dark:text-cyan-300" @error="() => {}" />
            </div>
            <p class="text-[12px] font-semibold text-gray-900 dark:text-gray-50 truncate">
              {{ t(locale, 'copilot.label') }}
            </p>
            <span v-if="displayPlanLabel" class="shrink-0 rounded-md border border-cyan-200/80 bg-cyan-50 px-1 py-px text-[9px] font-medium text-cyan-700 dark:border-cyan-500/20 dark:bg-cyan-500/10 dark:text-cyan-300">
              {{ displayPlanLabel }}
            </span>
          </div>
          <div class="flex items-center gap-1.5">
            <span class="text-[10px] text-gray-400">{{ t(locale, 'subscription.updatedAt') }}</span>
            <span class="text-[10px] text-gray-400 font-mono">{{ updatedAt || '--' }}</span>
            <button @click="refresh" :disabled="loading" class="p-0.5 rounded hover:bg-gray-100 dark:hover:bg-gray-700 transition-colors disabled:opacity-50" :title="t(locale, 'subscription.refresh')">
              <svg class="w-3 h-3 text-gray-400" :class="{ 'animate-spin': loading }" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
              </svg>
            </button>
          </div>
        </div>

        <p v-if="accountLabel" class="mt-0.5 truncate text-[9px] font-mono text-gray-400 dark:text-gray-500">
          {{ accountLabel }}
        </p>

        <div class="mt-2">
          <div v-if="loading && tiers.length === 0" class="text-[10px] text-gray-400">
            {{ t(locale, 'common.syncing') }}
          </div>

          <div v-else-if="!success && displayError" class="text-[10px] text-red-500">
            {{ displayError }}
          </div>

          <template v-else>
            <div v-if="premiumTier" class="rounded-lg border border-gray-100/90 bg-gray-50/60 px-2 py-2 dark:border-white/10 dark:bg-white/5">
              <div class="flex items-center justify-between gap-2">
                <div class="flex min-w-0 items-center gap-1.5">
                  <span class="h-1.5 w-1.5 rounded-full" :class="getTierDotClass(premiumTier.utilization)" />
                  <span class="text-[10px] text-gray-500 dark:text-gray-400 whitespace-nowrap">{{ t(locale, 'copilot.quota.premium') }}</span>
                </div>
                <span :class="['text-[10px] font-mono font-semibold', getTierColorClass(premiumTier.utilization)]">
                  {{ formatTierCounter(premiumTier) }}
                </span>
              </div>
              <div class="mt-1 h-1.5 overflow-hidden rounded-full bg-gray-200/80 dark:bg-white/10">
                <div class="h-full rounded-full bg-cyan-500 transition-all duration-300" :style="{ width: `${Math.min(100, premiumTier.utilization)}%` }"></div>
              </div>
              <div class="mt-1 text-[9px] text-gray-400 dark:text-gray-500">
                {{ formatResetDate(premiumTier.resetsAt) }}
              </div>
            </div>

            <div class="mt-1.5 grid grid-cols-2 gap-1.5">
              <div class="rounded-lg border border-gray-100/90 bg-gray-50/60 px-1.5 py-1.5 dark:border-white/10 dark:bg-white/5">
                <div class="flex items-center justify-between gap-2">
                  <span class="text-[10px] text-gray-500 dark:text-gray-400">{{ t(locale, 'copilot.quota.chat') }}</span>
                  <span class="text-[10px] font-medium text-cyan-600 dark:text-cyan-300">{{ formatTierCounter(chatTier) }}</span>
                </div>
              </div>
              <div class="rounded-lg border border-gray-100/90 bg-gray-50/60 px-1.5 py-1.5 dark:border-white/10 dark:bg-white/5">
                <div class="flex items-center justify-between gap-2">
                  <span class="text-[10px] text-gray-500 dark:text-gray-400">{{ t(locale, 'copilot.quota.completions') }}</span>
                  <span class="text-[10px] font-medium text-cyan-600 dark:text-cyan-300">{{ formatTierCounter(completionsTier) }}</span>
                </div>
              </div>
            </div>
          </template>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.metric-card {
  --metric-separator-soft: color-mix(in srgb, var(--theme-text-primary) 8%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-text-primary) 16%, transparent);
  min-width: 0;
  overflow: hidden;
  min-height: 0;
  border-radius: 1rem;
  border-width: 1px;
  background: var(--theme-surface-gradient);
  border-color: var(--theme-border-default);
  box-shadow: var(--theme-shadow-inline);
}

.metric-card-copilot {
  --metric-separator-soft: color-mix(in srgb, var(--theme-chart-requests) 18%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-chart-requests) 34%, transparent);
}

.metric-rail {
  position: relative;
  display: flex;
  width: 2.5rem;
  flex-shrink: 0;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.375rem;
  padding: 0.5rem 0.25rem;
}

.metric-rail::after {
  position: absolute;
  top: 0.75rem;
  bottom: 0.75rem;
  right: 0;
  width: 1px;
  content: '';
  background: linear-gradient(to bottom, transparent, var(--metric-separator-strong) 12%, var(--metric-separator-strong) 88%, transparent);
}

.metric-rail-icon {
  display: flex;
  width: 1.125rem;
  height: 1.125rem;
  align-items: center;
  justify-content: center;
  transform: translateY(-0.1875rem);
}

.metric-rail-title {
  display: block;
  flex-shrink: 0;
  overflow: visible;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0;
  line-height: 1.08;
  transform: translateY(-0.1875rem);
  white-space: nowrap;
}

.metric-body {
  min-width: 0;
  flex: 1 1 0%;
  padding: 0.625rem 0.625rem 0.5rem;
}

.writing-vertical {
  writing-mode: vertical-rl;
  text-orientation: upright;
}
</style>
