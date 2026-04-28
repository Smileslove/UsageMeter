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

const rateInputClass = 'min-w-0 bg-transparent text-right font-mono font-semibold text-gray-700 dark:text-gray-200 outline-none [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none'

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
      <button @click="emit('back')" class="flex items-center gap-1 text-blue-500 text-[13px] hover:text-blue-600 transition-colors">
        <ChevronLeft class="w-4 h-4" :stroke-width="2.2" />
        {{ t(store.settings.locale, 'common.dashboard') }}
      </button>
      <h2 class="text-sm font-semibold text-gray-800 dark:text-gray-100">
        {{ t(store.settings.locale, 'settings.currency') }}
      </h2>
      <button
        @click="syncRates"
        :disabled="syncing"
        class="flex items-center justify-center gap-1.5 px-2.5 py-1.5 rounded-lg text-xs font-semibold transition-all bg-blue-500 text-white hover:bg-blue-600 active:scale-[0.98] disabled:opacity-50 disabled:cursor-not-allowed shadow-[0_6px_14px_rgba(59,130,246,0.18)]"
      >
        <RefreshCcw :class="['w-3.5 h-3.5', syncing ? 'animate-spin' : '']" :stroke-width="2.4" />
        <span>{{ syncing ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'settings.currencySync') }}</span>
      </button>
    </div>

    <!-- 同步成功/失败提示 -->
    <div v-if="syncSuccess" class="flex items-center justify-center gap-1.5 rounded-full bg-emerald-50 py-1.5 text-xs font-medium text-emerald-600 dark:bg-emerald-500/10 dark:text-emerald-400">
      <CheckCircle2 class="w-3.5 h-3.5" />
      <span>{{ t(store.settings.locale, 'settings.currencySyncSuccess') }}</span>
    </div>
    <div v-if="syncError" class="flex items-center justify-center gap-1.5 rounded-full bg-red-50 py-1.5 text-xs font-medium text-red-500 dark:bg-red-500/10 dark:text-red-400">
      <AlertCircle class="w-3.5 h-3.5" />
      <span>{{ syncError }}</span>
    </div>

    <div class="bg-white dark:bg-[#1C1C1E] rounded-2xl border border-gray-50 dark:border-neutral-800 shadow-[0_2px_10px_rgba(0,0,0,0.02)] overflow-hidden">
      <div class="p-3">
        <div class="flex items-center justify-between gap-3">
          <div class="flex items-center gap-3 min-w-0">
            <div class="w-9 h-9 rounded-xl bg-blue-50 text-blue-600 dark:bg-blue-500/15 dark:text-blue-300 flex items-center justify-center shrink-0">
              <BadgeDollarSign class="w-[18px] h-[18px]" :stroke-width="2.1" />
            </div>
            <div class="min-w-0">
              <p class="text-[11px] font-medium text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.currencyDisplay') }}</p>
              <div class="flex items-baseline gap-1.5 min-w-0">
                <span class="text-[20px] leading-none font-bold text-gray-900 dark:text-gray-50 tracking-tight">{{ localCurrency.displayCurrency }}</span>
                <span class="text-xs text-gray-400 dark:text-gray-500 truncate">{{ displayCurrencyName }}</span>
              </div>
            </div>
          </div>
          <div class="relative shrink-0">
            <select
              v-model="localCurrency.displayCurrency"
              @change="save"
              class="h-8 pl-3 pr-7 rounded-full bg-gray-50 dark:bg-neutral-800 border border-gray-100 dark:border-neutral-700 text-xs font-semibold text-gray-700 dark:text-gray-200 outline-none cursor-pointer appearance-none focus:border-blue-300 dark:focus:border-blue-500"
            >
              <option v-for="code in displayOptions" :key="code" :value="code">{{ code }}</option>
            </select>
            <ChevronDown class="w-3.5 h-3.5 text-gray-400 pointer-events-none absolute right-2 top-1/2 -translate-y-1/2" />
          </div>
        </div>
        <div class="mt-2 flex items-center gap-1.5 min-w-0 rounded-xl bg-gray-50 dark:bg-neutral-800/70 px-2.5 py-1.5">
          <span class="text-[10px] font-medium text-gray-400 dark:text-gray-500 shrink-0">1 USD</span>
          <span class="text-[10px] text-gray-300 dark:text-gray-600 shrink-0">=</span>
          <span class="min-w-0 flex-1 overflow-x-auto whitespace-nowrap text-[16px] leading-none font-mono font-bold text-gray-900 dark:text-gray-50">{{ displayCurrencyRate.toFixed(4) }}</span>
          <span class="text-[10px] font-semibold text-gray-400 dark:text-gray-500 shrink-0">{{ localCurrency.displayCurrency }}</span>
        </div>
      </div>
      <div class="px-4 py-2.5 border-t border-gray-50 dark:border-neutral-800/70 text-[10px] text-gray-400 dark:text-gray-500 leading-relaxed">
        {{ t(store.settings.locale, 'settings.currencyDataSource') }}
      </div>
    </div>

    <!-- 币种汇率列表 -->
    <div class="space-y-1.5">
      <div
        v-for="code in localCurrency.trackedCurrencies"
        :key="code"
        class="bg-white dark:bg-[#1C1C1E] rounded-2xl border border-gray-50 dark:border-neutral-800 shadow-[0_2px_10px_rgba(0,0,0,0.02)] px-3 py-2"
      >
        <div class="grid grid-cols-[2rem_2.75rem_minmax(0,1fr)_8.25rem_2rem] items-center gap-2 min-w-0">
          <div
            class="w-8 h-8 rounded-xl flex items-center justify-center text-[12px] font-bold shrink-0"
            :class="code === localCurrency.displayCurrency ? 'bg-blue-50 text-blue-600 dark:bg-blue-500/15 dark:text-blue-300' : 'bg-gray-50 text-gray-500 dark:bg-neutral-800 dark:text-gray-400'"
          >
            {{ getCurrencySymbol(code).trim().slice(0, 2) || code.slice(0, 1) }}
          </div>
          <span class="font-mono text-sm font-semibold text-gray-800 dark:text-gray-100 leading-tight tabular-nums">{{ code }}</span>
          <span class="min-w-0 text-[12px] font-medium text-gray-700 dark:text-gray-200 truncate">{{ getCurrencyName(code, store.settings.locale) }}</span>
          <div class="min-w-0 flex items-center gap-1 rounded-lg bg-gray-50 dark:bg-neutral-800/75 border border-gray-50 dark:border-neutral-800 px-2 py-1.5">
            <span
              v-if="code === 'USD'"
              class="min-w-0 flex-1 overflow-x-auto whitespace-nowrap text-right font-mono text-[12px] font-semibold text-gray-600 dark:text-gray-300"
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
            <span class="text-[10px] font-semibold text-gray-400 dark:text-gray-500 shrink-0">{{ code }}</span>
          </div>
          <button
            @click.stop="removeCurrency(code)"
            :disabled="code === 'USD'"
            :class="[
              'w-8 h-8 rounded-lg flex items-center justify-center transition-colors shrink-0',
              code === 'USD'
                ? 'invisible pointer-events-none'
                : 'text-gray-300 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-500/10'
            ]"
            :title="t(store.settings.locale, 'settings.currencyRemove')"
          >
            <Trash2 class="w-4 h-4" :stroke-width="2" />
          </button>
        </div>
      </div>
    </div>

    <!-- 添加币种 -->
    <div class="bg-white dark:bg-[#1C1C1E] rounded-2xl border border-gray-50 dark:border-neutral-800 overflow-hidden shadow-[0_2px_10px_rgba(0,0,0,0.02)]">
      <!-- 搜索和选择 -->
      <div class="p-3">
        <div class="flex items-center justify-between mb-2">
          <div class="flex items-center gap-1.5 text-[12px] font-semibold text-gray-700 dark:text-gray-200">
            <Plus class="w-3.5 h-3.5 text-blue-500" :stroke-width="2.4" />
            <span>{{ t(store.settings.locale, 'settings.currencyAddRate') }}</span>
          </div>
          <span class="text-[10px] text-gray-400 dark:text-gray-500">{{ availableCurrencies.length }}</span>
        </div>
        <!-- 搜索框 -->
        <div class="relative mb-2">
          <Search class="w-3.5 h-3.5 text-gray-400 absolute left-2.5 top-1/2 -translate-y-1/2" />
          <input
            v-model="searchQuery"
            type="text"
            :placeholder="t(store.settings.locale, 'settings.currencySearch')"
            class="w-full pl-8 pr-2.5 py-2 bg-gray-50 dark:bg-neutral-800 border border-gray-100 dark:border-neutral-700 rounded-xl text-xs text-gray-700 dark:text-gray-200 outline-none focus:border-blue-300 dark:focus:border-blue-500 transition-colors"
          />
        </div>

        <!-- 可用货币列表 -->
        <div class="max-h-44 overflow-y-auto pr-0.5">
          <div v-if="availableCurrencies.length === 0" class="text-center text-xs text-gray-400 py-3">
            {{ searchQuery ? t(store.settings.locale, 'settings.currencyNoResults') : t(store.settings.locale, 'settings.currencyAllAdded') }}
          </div>
          <div v-else class="space-y-1">
            <button
              v-for="code in availableCurrencies"
              :key="code"
              @click="addCurrency(code)"
              class="group w-full min-w-0 px-2.5 py-2 rounded-xl text-left transition-all bg-gray-50 text-gray-600 hover:bg-blue-50 hover:text-blue-600 active:scale-[0.99] dark:bg-neutral-800 dark:text-gray-400 dark:hover:bg-blue-500/20"
            >
              <span class="flex items-center gap-2 min-w-0">
                <span class="w-7 h-7 rounded-lg bg-white dark:bg-[#1C1C1E] border border-gray-100 dark:border-neutral-700 flex items-center justify-center text-[11px] font-bold text-gray-500 dark:text-gray-400 shrink-0 group-hover:border-blue-100 group-hover:text-blue-600 dark:group-hover:border-blue-500/20 dark:group-hover:text-blue-300">
                  {{ getCurrencySymbol(code).trim().slice(0, 2) || code.slice(0, 1) }}
                </span>
                <span class="min-w-0 flex-1 flex items-center gap-1.5">
                  <span class="text-xs font-semibold text-gray-700 dark:text-gray-200 group-hover:text-blue-600 dark:group-hover:text-blue-300 shrink-0">{{ code }}</span>
                  <span class="text-[12px] font-medium text-gray-700 dark:text-gray-200 truncate group-hover:text-blue-600 dark:group-hover:text-blue-300">{{ getCurrencyName(code, store.settings.locale) }}</span>
                </span>
                <span class="w-6 h-6 rounded-full flex items-center justify-center bg-white text-gray-300 border border-gray-100 group-hover:bg-blue-500 group-hover:text-white group-hover:border-blue-500 dark:bg-[#1C1C1E] dark:border-neutral-700 dark:group-hover:bg-blue-500 dark:group-hover:border-blue-500 shrink-0 transition-colors">
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
