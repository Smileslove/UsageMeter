<script setup lang="ts">
import { ref, watch } from 'vue'
import { t } from '../i18n'
import type { ModelPricingConfig } from '../types'
import { useMonitorStore } from '../stores/monitor'
import { getCurrencySymbol } from '../utils/format'

const store = useMonitorStore()

const props = defineProps<{
  pricing: ModelPricingConfig | null
  locale: string
}>()

const emit = defineEmits<{
  save: [pricing: ModelPricingConfig]
  close: []
}>()

// 输入币种（默认跟随显示货币）
const inputCurrency = ref(store.settings.currency.displayCurrency)

// 表单状态
const modelId = ref('')
const displayName = ref('')
const inputPrice = ref(0)
const outputPrice = ref(0)
const cacheReadPrice = ref<number | undefined>(undefined)
const cacheWritePrice = ref<number | undefined>(undefined)

// 是否编辑模式
const isEdit = ref(false)

// 初始化
watch(() => props.pricing, (pricing) => {
  inputCurrency.value = store.settings.currency.displayCurrency
  if (pricing) {
    isEdit.value = true
    modelId.value = pricing.modelId
    displayName.value = pricing.displayName || ''
    // 将存储的 USD 价格转换为当前输入币种显示
    const r = store.settings.currency.exchangeRates[inputCurrency.value] || 1.0
    inputPrice.value = parseFloat((pricing.inputPrice * r).toFixed(4))
    outputPrice.value = parseFloat((pricing.outputPrice * r).toFixed(4))
    cacheReadPrice.value = pricing.cacheReadPrice != null ? parseFloat((pricing.cacheReadPrice * r).toFixed(4)) : undefined
    cacheWritePrice.value = pricing.cacheWritePrice != null ? parseFloat((pricing.cacheWritePrice * r).toFixed(4)) : undefined
  } else {
    isEdit.value = false
    modelId.value = ''
    displayName.value = ''
    inputPrice.value = 0
    outputPrice.value = 0
    cacheReadPrice.value = undefined
    cacheWritePrice.value = undefined
  }
}, { immediate: true })

// 输入币种变化时重新转换显示值
watch(inputCurrency, (newCurrency) => {
  if (!props.pricing) return
  const r = store.settings.currency.exchangeRates[newCurrency] || 1.0
  inputPrice.value = parseFloat((props.pricing.inputPrice * r).toFixed(4))
  outputPrice.value = parseFloat((props.pricing.outputPrice * r).toFixed(4))
  cacheReadPrice.value = props.pricing.cacheReadPrice != null ? parseFloat((props.pricing.cacheReadPrice * r).toFixed(4)) : undefined
  cacheWritePrice.value = props.pricing.cacheWritePrice != null ? parseFloat((props.pricing.cacheWritePrice * r).toFixed(4)) : undefined
})

// 保存（将输入币种价格转换为 USD 存储）
const handleSave = () => {
  if (!modelId.value.trim()) return
  if (inputPrice.value <= 0 || outputPrice.value <= 0) return

  const r = store.settings.currency.exchangeRates[inputCurrency.value] || 1.0

  const pricing: ModelPricingConfig = {
    modelId: modelId.value.trim(),
    displayName: displayName.value.trim() || undefined,
    inputPrice: parseFloat((inputPrice.value / r).toFixed(6)),
    outputPrice: parseFloat((outputPrice.value / r).toFixed(6)),
    cacheReadPrice: cacheReadPrice.value != null ? parseFloat((cacheReadPrice.value / r).toFixed(6)) : undefined,
    cacheWritePrice: cacheWritePrice.value != null ? parseFloat((cacheWritePrice.value / r).toFixed(6)) : undefined,
    source: 'custom',
    lastUpdated: Date.now()
  }

  emit('save', pricing)
}

// 验证
const isValid = () => {
  return modelId.value.trim() && inputPrice.value > 0 && outputPrice.value > 0
}
</script>

<template>
  <div class="p-3 w-72">
    <!-- 标题 -->
    <div class="flex items-center justify-between mb-3">
      <h3 class="text-[13px] font-semibold text-gray-800 dark:text-gray-100">
        {{ isEdit ? t(props.locale, 'settings.modelPricingEdit') : t(props.locale, 'settings.modelPricingAdd') }}
      </h3>
      <button @click="emit('close')" class="p-1 hover:bg-gray-100 dark:hover:bg-neutral-800 rounded-lg transition-colors">
        <svg class="w-3.5 h-3.5 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
        </svg>
      </button>
    </div>

    <!-- 表单 -->
    <div class="space-y-2">
      <!-- 模型 ID -->
      <div>
        <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(props.locale, 'settings.modelPricingModelId') }}</label>
        <input
          v-model="modelId"
          :disabled="isEdit"
          type="text"
          :placeholder="'claude-3-sonnet-20240229'"
          class="w-full px-2.5 py-1.5 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-xs outline-none focus:border-blue-400 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
        />
      </div>

      <!-- 显示名称 -->
      <div>
        <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(props.locale, 'settings.modelPricingDisplayName') }}</label>
        <input
          v-model="displayName"
          type="text"
          :placeholder="t(props.locale, 'settings.modelPricingDisplayNamePlaceholder')"
          class="w-full px-2.5 py-1.5 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-xs outline-none focus:border-blue-400 transition-colors"
        />
      </div>

      <!-- 输入币种选择 -->
      <div>
        <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(props.locale, 'settings.currencyInputUnit') }}</label>
        <select
          v-model="inputCurrency"
          class="w-full px-2.5 py-1.5 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-xs outline-none focus:border-blue-400 transition-colors cursor-pointer appearance-none"
        >
          <option v-for="code in store.settings.currency.trackedCurrencies" :key="code" :value="code">
            {{ code }} ({{ getCurrencySymbol(code) }})
          </option>
        </select>
      </div>

      <!-- 输入/输出价格 -->
      <div class="grid grid-cols-2 gap-2">
        <div>
          <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(props.locale, 'settings.modelPricingInput') }}</label>
          <div class="relative">
            <input
              v-model.number="inputPrice"
              type="number"
              step="0.01"
              min="0"
              placeholder="3.00"
              class="w-full px-2.5 py-1.5 pr-10 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-xs outline-none focus:border-blue-400 transition-colors font-mono"
            />
            <span class="absolute right-2 top-1/2 -translate-y-1/2 text-[9px] text-gray-400">{{ getCurrencySymbol(inputCurrency) }}/M</span>
          </div>
        </div>
        <div>
          <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(props.locale, 'settings.modelPricingOutput') }}</label>
          <div class="relative">
            <input
              v-model.number="outputPrice"
              type="number"
              step="0.01"
              min="0"
              placeholder="15.00"
              class="w-full px-2.5 py-1.5 pr-10 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-xs outline-none focus:border-blue-400 transition-colors font-mono"
            />
            <span class="absolute right-2 top-1/2 -translate-y-1/2 text-[9px] text-gray-400">{{ getCurrencySymbol(inputCurrency) }}/M</span>
          </div>
        </div>
      </div>

      <!-- 缓存价格（可选） -->
      <div class="grid grid-cols-2 gap-2">
        <div>
          <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(props.locale, 'settings.modelPricingCacheRead') }}</label>
          <div class="relative">
            <input
              v-model.number="cacheReadPrice"
              type="number"
              step="0.01"
              min="0"
              placeholder="0"
              class="w-full px-2.5 py-1.5 pr-10 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-xs outline-none focus:border-blue-400 transition-colors font-mono"
            />
            <span class="absolute right-2 top-1/2 -translate-y-1/2 text-[9px] text-gray-400">{{ getCurrencySymbol(inputCurrency) }}/M</span>
          </div>
        </div>
        <div>
          <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(props.locale, 'settings.modelPricingCacheWrite') }}</label>
          <div class="relative">
            <input
              v-model.number="cacheWritePrice"
              type="number"
              step="0.01"
              min="0"
              placeholder="0"
              class="w-full px-2.5 py-1.5 pr-10 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-xs outline-none focus:border-blue-400 transition-colors font-mono"
            />
            <span class="absolute right-2 top-1/2 -translate-y-1/2 text-[9px] text-gray-400">{{ getCurrencySymbol(inputCurrency) }}/M</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 按钮 -->
    <div class="flex gap-2 mt-3 pt-2 border-t border-gray-100 dark:border-neutral-800">
      <button
        @click="emit('close')"
        class="flex-1 py-1.5 bg-gray-100 dark:bg-neutral-800 hover:bg-gray-200 dark:hover:bg-neutral-700 rounded-lg text-xs text-gray-600 dark:text-gray-300 transition-colors"
      >
        {{ t(props.locale, 'common.cancel') || '取消' }}
      </button>
      <button
        @click="handleSave"
        :disabled="!isValid()"
        class="flex-1 py-1.5 bg-blue-500 hover:bg-blue-600 disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-xs text-white transition-colors"
      >
        {{ t(props.locale, 'common.save') }}
      </button>
    </div>
  </div>
</template>
