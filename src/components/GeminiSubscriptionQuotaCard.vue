<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { Clock, Search } from 'lucide-vue-next'
import LobeIcon from './LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'
import type { CredentialStatus, QuotaTier } from '../types'

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

const quota = computed(() => store.geminiQuota?.quota)
const tiers = computed<QuotaTier[]>(() => quota.value?.tiers ?? [])
const loading = computed(() => store.geminiQuotaLoading)
const success = computed(() => store.geminiQuota?.success)
const planLabel = computed(() => quota.value?.planLabel ?? '')
const accountLabel = computed(() => quota.value?.accountLabel ?? '')

const displayError = computed(() => {
  const result = store.geminiQuota
  if (!result || result.success) return ''
  const status: CredentialStatus = result.credentialStatus
  if (status === 'notConfigured') return t(locale.value, 'subscription.errorNotConfigured')
  if (status === 'expired') return t(locale.value, 'subscription.errorExpired')
  if (typeof status === 'object' && 'refreshFailed' in status) return t(locale.value, 'subscription.errorRefreshFailed')
  if (typeof status === 'object' && 'queryFailed' in status) return t(locale.value, 'subscription.errorQueryFailed')
  return t(locale.value, 'subscription.errorUnknown')
})

const updatedAt = computed(() => {
  const ts = quota.value?.updatedAt
  if (!ts) return ''
  return new Date(ts).toLocaleTimeString(locale.value)
})

interface FamilyRow {
  key: string
  label: string
  tier: QuotaTier
}

const familyRows = computed<FamilyRow[]>(() => {
  const defs: { name: string; labelKey: string }[] = [
    { name: 'gemini_pro', labelKey: 'subscription.geminiPro' },
    { name: 'gemini_flash', labelKey: 'subscription.geminiFlash' },
    { name: 'gemini_flash_lite', labelKey: 'subscription.geminiFlashLite' }
  ]
  return defs
    .map(def => {
      const tier = tiers.value.find(item => item.name === def.name)
      return tier ? { key: def.name, label: t(locale.value, def.labelKey), tier } : null
    })
    .filter((row): row is FamilyRow => row !== null)
})

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

function formatResetTimeRelative(resetsAt?: string): string {
  if (!resetsAt) return '--'
  const resetDate = new Date(resetsAt)
  if (Number.isNaN(resetDate.getTime())) return '--'
  const now = new Date()
  const diffMs = resetDate.getTime() - now.getTime()

  if (diffMs <= 0) return t(locale.value, 'subscription.resetNow')

  const diffMins = Math.floor(diffMs / 60000)
  const diffHours = Math.floor(diffMins / 60)
  const diffDays = Math.floor(diffHours / 24)
  const dayUnit = t(locale.value, 'subscription.unitDayShort')
  const hourUnit = t(locale.value, 'subscription.unitHourShort')
  const minuteUnit = t(locale.value, 'subscription.unitMinuteShort')

  if (diffDays > 0) {
    const remainHours = diffHours % 24
    if (remainHours > 0) {
      return `${diffDays}${dayUnit}${remainHours}${hourUnit}`
    }
    return `${diffDays}${dayUnit}`
  }
  if (diffHours > 0) {
    const remainMins = diffMins % 60
    if (remainMins > 0) {
      return `${diffHours}${hourUnit}${remainMins}${minuteUnit}`
    }
    return `${diffHours}${hourUnit}`
  }
  return `${diffMins}${minuteUnit}`
}

async function refresh() {
  await store.refreshGeminiQuota()
}
</script>

<template>
  <div v-if="store.hasGeminiOAuth" class="metric-card metric-card-codex group !bg-white border-cyan-200/90 dark:!bg-[#1C1C1E] dark:border-cyan-500/15">
    <div class="flex items-stretch">
      <div class="metric-rail text-cyan-600 dark:text-cyan-300">
        <div class="metric-rail-icon text-cyan-500 dark:text-cyan-300">
          <Search class="h-3.5 w-3.5 shrink-0" />
        </div>
        <p class="writing-vertical metric-rail-title">{{ t(locale, 'subscription.usageQuery') }}</p>
      </div>

      <div class="metric-body">
        <div class="divide-y divide-gray-100 dark:divide-gray-800/70">
          <div class="py-2 first:pt-1 last:pb-1">
            <div class="flex items-center justify-between gap-3">
              <div class="flex min-w-0 items-center gap-2">
                <div class="h-6 w-6 flex items-center justify-center">
                  <LobeIcon :slug="TOOL_LOBE_ICONS.gemini" :size="18" class="text-cyan-600 dark:text-cyan-300" @error="() => {}" />
                </div>
                <p class="text-[12px] font-semibold text-gray-900 dark:text-gray-50 truncate">
                  {{ t(locale, 'subscription.geminiQuota') }}
                </p>
                <span v-if="planLabel" class="shrink-0 rounded-md border border-cyan-200/80 bg-cyan-50 px-1 py-px text-[9px] font-medium text-cyan-700 dark:border-cyan-500/20 dark:bg-cyan-500/10 dark:text-cyan-300">
                  {{ planLabel }}
                </span>
              </div>
              <div class="flex items-center gap-1.5">
                <span class="text-[10px] text-gray-400">
                  {{ t(locale, 'subscription.updatedAt') }}
                </span>
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
                <div v-if="familyRows.length" class="space-y-1.5">
                  <div
                    v-for="row in familyRows"
                    :key="row.key"
                    class="rounded-lg border border-gray-100/90 bg-gray-50/60 px-1.5 py-1.5 dark:border-white/10 dark:bg-white/5"
                  >
                    <div class="flex items-center justify-between gap-2">
                      <div class="flex min-w-0 items-center gap-1.5">
                        <span class="h-1.5 w-1.5 rounded-full" :class="getTierDotClass(row.tier.utilization)" />
                        <span class="text-[10px] text-gray-500 dark:text-gray-400 whitespace-nowrap">{{ row.label }}</span>
                      </div>

                      <div class="flex shrink-0 items-center gap-1.5">
                        <span :class="['text-[10px] font-mono font-semibold', getTierColorClass(row.tier.utilization)]">{{ row.tier.utilization.toFixed(1) }}%</span>
                        <span class="flex items-center gap-0.5 rounded-md border border-gray-100 bg-white/70 px-1 py-0.5 text-[9px] text-gray-400 dark:border-white/10 dark:bg-white/5 dark:text-gray-500">
                          <Clock class="h-2.5 w-2.5 shrink-0" aria-hidden="true" />
                          <span class="font-mono whitespace-nowrap">{{ formatResetTimeRelative(row.tier.resetsAt) }}</span>
                        </span>
                      </div>
                    </div>
                  </div>
                </div>

                <div v-else class="text-[10px] text-gray-400">
                  {{ t(locale, 'common.noData') }}
                </div>
              </template>
            </div>
          </div>
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

.metric-card-codex {
  --metric-separator-soft: color-mix(in srgb, var(--theme-chart-requests) 18%, transparent);
  --metric-separator-strong: color-mix(in srgb, var(--theme-chart-requests) 34%, transparent);
}

.writing-vertical {
  writing-mode: vertical-rl;
  text-orientation: upright;
}
</style>
