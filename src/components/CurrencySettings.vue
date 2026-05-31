<script setup lang="ts">
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import {
  AlertCircle,
  BadgeDollarSign,
  CheckCircle2,
  ChevronDown,
  ChevronLeft,
  Plus,
  RefreshCcw,
  Search,
  Trash2
} from 'lucide-vue-next'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { getCurrencyName, getAllCurrencyCodes, getCurrencySymbol } from '../utils/format'
import type { CurrencySettings as CurrencySettingsType } from '../types'

const emit = defineEmits<{
  back: []
}>()

const store = useMonitorStore()

// 同步状态
const syncing = ref(false)
const syncError = ref('')
const syncSuccess = ref(false)

// 搜索关键字
const searchQuery = ref('')

// 所有支持的货币代码
const ALL_CURRENCY_CODES = getAllCurrencyCodes()

// 本地货币设置副本（使用 JSON 深拷贝避免 Pinia 代理克隆错误）
const localCurrency = ref(JSON.parse(JSON.stringify(store.settings.currency)) as CurrencySettingsType)

// 可用的显示货币选项
const displayOptions = computed(() =>
  localCurrency.value.trackedCurrencies.filter(c =>
    localCurrency.value.exchangeRates[c] !== undefined
  )
)

// 根据搜索关键字过滤货币列表
const filteredCurrencies = computed(() => {
  const query = searchQuery.value.trim().toLowerCase()
  if (!query) return ALL_CURRENCY_CODES
  return ALL_CURRENCY_CODES.filter(code => {
    const name = getCurrencyName(code, store.settings.locale).toLowerCase()
    return code.toLowerCase().includes(query) || name.includes(query)
  })
})

// 未追踪的货币（可供添加）
const availableCurrencies = computed(() =>
  filteredCurrencies.value.filter(c => !localCurrency.value.trackedCurrencies.includes(c))
)

const displayCurrencyName = computed(() =>
  getCurrencyName(localCurrency.value.displayCurrency, store.settings.locale)
)

const displayCurrencyRate = computed(() =>
  localCurrency.value.exchangeRates[localCurrency.value.displayCurrency] ?? 1
)

const rateInputClass = 'min-w-0 bg-transparent text-right font-mono font-semibold text-[var(--theme-text-primary)] outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'

// 添加币种
async function addCurrency(code: string) {
  if (localCurrency.value.trackedCurrencies.includes(code)) return
  localCurrency.value.trackedCurrencies.push(code)
  localCurrency.value.exchangeRates[code] = 0
  await save()
}

// 删除币种
async function removeCurrency(code: string) {
  if (code === 'USD') return
  const idx = localCurrency.value.trackedCurrencies.indexOf(code)
  if (idx >= 0) localCurrency.value.trackedCurrencies.splice(idx, 1)
  delete localCurrency.value.exchangeRates[code]
  if (localCurrency.value.displayCurrency === code) {
    localCurrency.value.displayCurrency = 'USD'
  }
  await save()
}

// 同步汇率
async function syncRates() {
  syncing.value = true
  syncError.value = ''
  syncSuccess.value = false
  try {
    const currencies = localCurrency.value.trackedCurrencies.filter(c => c !== 'USD')
    const rates = await invoke<Record<string, number>>('get_exchange_rates', { currencies })
    for (const [code, rate] of Object.entries(rates)) {
      localCurrency.value.exchangeRates[code] = rate
    }
    localCurrency.value.lastRateUpdate = Date.now()
    syncSuccess.value = true
    await save()
    setTimeout(() => { syncSuccess.value = false }, 3000)
  } catch (e) {
    syncError.value = t(store.settings.locale, 'settings.currencySyncError')
  } finally {
    syncing.value = false
  }
}

// 保存设置
async function save() {
  store.settings.currency = JSON.parse(JSON.stringify(localCurrency.value)) as CurrencySettingsType
  await store.saveSettings()
}
</script>

<template>
  <div class="flex flex-col gap-3 animate-in fade-in zoom-in-95 duration-300">
    <!-- 头部 -->
    <div class="flex items-center justify-between gap-2 px-1">
      <button @click="emit('back')" class="flex items-center gap-1 text-[13px] text-[var(--theme-accent-primary)] transition-colors hover:opacity-85">
        <ChevronLeft class="w-4 h-4" :stroke-width="2.2" />
        {{ t(store.settings.locale, 'common.dashboard') }}
      </button>
      <h2 class="text-sm font-semibold text-[var(--theme-text-primary)]">
        {{ t(store.settings.locale, 'settings.currency') }}
      </h2>
      <button
        @click="syncRates"
        :disabled="syncing"
        class="theme-button-accent flex items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-semibold transition-all active:scale-[0.98] disabled:cursor-not-allowed disabled:opacity-50"
      >
        <RefreshCcw :class="['w-3.5 h-3.5', syncing ? 'animate-spin' : '']" :stroke-width="2.4" />
        <span>{{ syncing ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'settings.currencySync') }}</span>
      </button>
    </div>

    <!-- 同步成功/失败提示 -->
    <div v-if="syncSuccess" class="theme-status-success flex items-center justify-center gap-1.5 rounded-full border py-1.5 text-xs font-medium">
      <CheckCircle2 class="w-3.5 h-3.5" />
      <span>{{ t(store.settings.locale, 'settings.currencySyncSuccess') }}</span>
    </div>
    <div v-if="syncError" class="theme-status-danger flex items-center justify-center gap-1.5 rounded-full border py-1.5 text-xs font-medium">
      <AlertCircle class="w-3.5 h-3.5" />
      <span>{{ syncError }}</span>
    </div>

    <div class="theme-surface rounded-2xl border overflow-hidden">
      <div class="p-3">
        <div class="flex items-center justify-between gap-3">
          <div class="flex items-center gap-3 min-w-0">
            <div class="theme-accent-soft flex h-9 w-9 items-center justify-center rounded-xl shrink-0 border">
              <BadgeDollarSign class="w-[18px] h-[18px]" :stroke-width="2.1" />
            </div>
            <div class="min-w-0">
              <p class="text-[11px] font-medium text-[var(--theme-text-tertiary)]">{{ t(store.settings.locale, 'settings.currencyDisplay') }}</p>
              <div class="flex items-baseline gap-1.5 min-w-0">
                <span class="text-[20px] leading-none font-bold tracking-tight text-[var(--theme-text-primary)]">{{ localCurrency.displayCurrency }}</span>
                <span class="truncate text-xs text-[var(--theme-text-tertiary)]">{{ displayCurrencyName }}</span>
              </div>
            </div>
          </div>
          <div class="relative shrink-0">
            <select
              v-model="localCurrency.displayCurrency"
              @change="save"
              class="theme-input h-8 cursor-pointer appearance-none rounded-full pl-3 pr-7 text-xs font-semibold"
            >
              <option v-for="code in displayOptions" :key="code" :value="code">{{ code }}</option>
            </select>
            <ChevronDown class="pointer-events-none absolute right-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[var(--theme-text-tertiary)]" />
          </div>
        </div>
        <div class="theme-surface-muted mt-2 flex min-w-0 items-center gap-1.5 rounded-xl border px-2.5 py-1.5">
          <span class="shrink-0 text-[10px] font-medium text-[var(--theme-text-tertiary)]">1 USD</span>
          <span class="shrink-0 text-[10px] text-[var(--theme-text-quaternary)]">=</span>
          <span class="min-w-0 flex-1 overflow-x-auto whitespace-nowrap text-[16px] leading-none font-mono font-bold text-[var(--theme-text-primary)]">{{ displayCurrencyRate.toFixed(4) }}</span>
          <span class="shrink-0 text-[10px] font-semibold text-[var(--theme-text-tertiary)]">{{ localCurrency.displayCurrency }}</span>
        </div>
      </div>
      <div class="border-t px-4 py-2.5 text-[10px] leading-relaxed text-[var(--theme-text-tertiary)] theme-divider">
        {{ t(store.settings.locale, 'settings.currencyDataSource') }}
      </div>
    </div>

    <!-- 币种汇率列表 -->
    <div class="space-y-1.5">
      <div
        v-for="code in localCurrency.trackedCurrencies"
        :key="code"
        class="theme-surface rounded-2xl border px-3 py-2"
      >
        <div class="grid grid-cols-[2rem_2.75rem_minmax(0,1fr)_8.25rem_2rem] items-center gap-2 min-w-0">
          <div
            class="flex h-8 w-8 items-center justify-center rounded-xl text-[12px] font-bold shrink-0"
            :class="code === localCurrency.displayCurrency ? 'theme-accent-soft border' : 'theme-surface-muted border text-[var(--theme-text-secondary)]'"
          >
            {{ getCurrencySymbol(code).trim().slice(0, 2) || code.slice(0, 1) }}
          </div>
          <span class="font-mono text-sm font-semibold leading-tight tabular-nums text-[var(--theme-text-primary)]">{{ code }}</span>
          <span class="min-w-0 truncate text-[12px] font-medium text-[var(--theme-text-primary)]">{{ getCurrencyName(code, store.settings.locale) }}</span>
          <div class="theme-surface-muted min-w-0 flex items-center gap-1 rounded-lg border px-2 py-1.5">
            <span
              v-if="code === 'USD'"
              class="min-w-0 flex-1 overflow-x-auto whitespace-nowrap text-right font-mono text-[12px] font-semibold text-[var(--theme-text-secondary)]"
            >
              1.0000
            </span>
            <input
              v-else
              type="number"
              :value="localCurrency.exchangeRates[code] ?? 0"
              @blur="(e: Event) => { const v = parseFloat((e.target as HTMLInputElement).value); if (!isNaN(v) && v > 0) { localCurrency.exchangeRates[code] = v; save() } }"
              step="0.0001"
              min="0.0001"
              :class="['w-full text-[12px]', rateInputClass]"
            />
            <span class="shrink-0 text-[10px] font-semibold text-[var(--theme-text-tertiary)]">{{ code }}</span>
          </div>
          <button
            @click.stop="removeCurrency(code)"
            :disabled="code === 'USD'"
            :class="[
              'w-8 h-8 rounded-lg flex items-center justify-center transition-colors shrink-0',
              code === 'USD'
                ? 'invisible pointer-events-none'
                : 'text-[var(--theme-text-quaternary)] hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-500/10'
            ]"
            :title="t(store.settings.locale, 'settings.currencyRemove')"
          >
            <Trash2 class="w-4 h-4" :stroke-width="2" />
          </button>
        </div>
      </div>
    </div>

    <!-- 添加币种 -->
    <div class="theme-surface rounded-2xl border overflow-hidden">
      <!-- 搜索和选择 -->
      <div class="p-3">
        <div class="flex items-center justify-between mb-2">
          <div class="flex items-center gap-1.5 text-[12px] font-semibold text-[var(--theme-text-primary)]">
            <Plus class="w-3.5 h-3.5 text-blue-500" :stroke-width="2.4" />
            <span>{{ t(store.settings.locale, 'settings.currencyAddRate') }}</span>
          </div>
          <span class="text-[10px] text-[var(--theme-text-tertiary)]">{{ availableCurrencies.length }}</span>
        </div>
        <!-- 搜索框 -->
        <div class="relative mb-2">
          <Search class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[var(--theme-text-tertiary)]" />
          <input
            v-model="searchQuery"
            type="text"
            :placeholder="t(store.settings.locale, 'settings.currencySearch')"
            class="theme-input w-full rounded-xl py-2 pl-8 pr-2.5 text-xs"
          />
        </div>

        <!-- 可用货币列表 -->
        <div class="max-h-44 overflow-y-auto pr-0.5">
          <div v-if="availableCurrencies.length === 0" class="py-3 text-center text-xs text-[var(--theme-text-tertiary)]">
            {{ searchQuery ? t(store.settings.locale, 'settings.currencyNoResults') : t(store.settings.locale, 'settings.currencyAllAdded') }}
          </div>
          <div v-else class="space-y-1">
            <button
              v-for="code in availableCurrencies"
              :key="code"
              @click="addCurrency(code)"
              class="theme-surface-muted group w-full min-w-0 rounded-xl px-2.5 py-2 text-left text-[var(--theme-text-secondary)] transition-all active:scale-[0.99] hover:bg-blue-50 hover:text-blue-600"
            >
              <span class="flex items-center gap-2 min-w-0">
                <span class="theme-surface flex h-7 w-7 items-center justify-center rounded-lg border text-[11px] font-bold text-[var(--theme-text-secondary)] shrink-0 group-hover:border-blue-100 group-hover:text-blue-600">
                  {{ getCurrencySymbol(code).trim().slice(0, 2) || code.slice(0, 1) }}
                </span>
                <span class="min-w-0 flex-1 flex items-center gap-1.5">
                  <span class="shrink-0 text-xs font-semibold text-[var(--theme-text-primary)] group-hover:text-blue-600">{{ code }}</span>
                  <span class="truncate text-[12px] font-medium text-[var(--theme-text-primary)] group-hover:text-blue-600">{{ getCurrencyName(code, store.settings.locale) }}</span>
                </span>
                <span class="theme-surface flex h-6 w-6 items-center justify-center rounded-full border text-[var(--theme-text-quaternary)] shrink-0 transition-colors group-hover:bg-blue-500 group-hover:text-white group-hover:border-blue-500">
                  <Plus class="w-3.5 h-3.5" :stroke-width="2.4" />
                </span>
              </span>
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
