<script setup lang="ts">
import { ref, onMounted, watch, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import type { ModelPricingConfig } from '../types'
import ModelPricingEditModal from './ModelPricingEditModal.vue'
import PricingApplyModal from './PricingApplyModal.vue'
import { formatCost as formatCostUtil } from '../utils/format'

const emit = defineEmits<{
  back: []
}>()

const store = useMonitorStore()

// 搜索结果类型
interface ModelPricingSearchResult {
  pricings: ModelPricingConfig[]
  total: number
}

// 本地状态
const syncedSearchQuery = ref('')
const syncing = ref(false)
const syncError = ref('')
const showEditModal = ref(false)
const editingPricing = ref<ModelPricingConfig | null>(null)
const loadError = ref('')

// 删除确认弹窗状态
const showDeleteConfirm = ref(false)
const deletingModelId = ref<string | null>(null)

// 价格应用弹窗状态
const showApplyModal = ref(false)
const applyingPricing = ref<ModelPricingConfig | null>(null)

// 当前标签页：custom 或 synced
const activeTab = ref<'custom' | 'synced'>('custom')

// 数据库中的价格列表
const customPricingList = ref<ModelPricingConfig[]>([])
const customLoading = ref(true)
const customSearchQuery = ref('')
const syncedPricingList = ref<ModelPricingConfig[]>([])
const syncedTotalCount = ref(0)
const syncedLoading = ref(false)

// 本地匹配模式
const localMatchMode = ref(store.settings.modelPricing?.matchMode ?? 'fuzzy')

// Tooltip 状态
const tooltipContent = ref('')
const tooltipVisible = ref(false)
const tooltipX = ref(0)
const tooltipY = ref(0)

// 监听 store 变化
watch(() => store.settings.modelPricing?.matchMode, (val) => {
  localMatchMode.value = val ?? 'fuzzy'
})

// 分离自定义模型和同步模型
const customPricings = computed(() => customPricingList.value)

const syncedPricings = computed(() => syncedPricingList.value)

// 显示 tooltip（仅当内容被截断时）
const showTooltip = (content: string, event: MouseEvent) => {
  const target = event.target as HTMLElement
  // 检查是否被截断
  if (target.scrollWidth > target.clientWidth) {
    tooltipContent.value = content
    tooltipVisible.value = true
    updateTooltipPosition(event)
  }
}

// 更新 tooltip 位置
const updateTooltipPosition = (event: MouseEvent) => {
  if (tooltipVisible.value) {
    tooltipX.value = event.clientX + 10
    tooltipY.value = event.clientY + 10
  }
}

// 隐藏 tooltip
const hideTooltip = () => {
  tooltipVisible.value = false
}

// 加载自定义模型列表
const loadCustomPricings = async () => {
  customLoading.value = true
  try {
    const jsonString = await invoke<string>('get_custom_model_pricings', {
      query: customSearchQuery.value || null
    })
    customPricingList.value = JSON.parse(jsonString) || []
  } catch (e) {
    console.error('[ModelPricingSettings] Failed to load custom pricings:', e)
    customPricingList.value = []
  } finally {
    customLoading.value = false
  }
}

// 加载同步模型总数
const loadSyncedCount = async () => {
  try {
    syncedTotalCount.value = await invoke<number>('count_synced_model_pricings', {
      query: null
    })
  } catch (e) {
    console.error('[ModelPricingSettings] Failed to load synced count:', e)
    syncedTotalCount.value = 0
  }
}

// 加载同步模型列表
const loadSyncedPricings = async () => {
  syncedLoading.value = true
  loadError.value = ''

  try {
    const jsonString = await invoke<string>('search_model_pricing', {
      query: syncedSearchQuery.value || null,
      limit: 100,
      offset: 0
    })

    const result = JSON.parse(jsonString) as ModelPricingSearchResult
    syncedPricingList.value = result.pricings || []
    syncedTotalCount.value = result.total || 0
  } catch (e) {
    console.error('[ModelPricingSettings] Failed to load synced pricings:', e)
    loadError.value = String(e)
    syncedPricingList.value = []
    syncedTotalCount.value = 0
  } finally {
    syncedLoading.value = false
  }
}

// 组件挂载时加载自定义模型和同步模型总数
onMounted(async () => {
  await Promise.all([loadCustomPricings(), loadSyncedCount()])
})

// 自定义模型搜索防抖
let customSearchTimeout: ReturnType<typeof setTimeout> | null = null
watch(customSearchQuery, () => {
  if (activeTab.value !== 'custom') return
  if (customSearchTimeout) clearTimeout(customSearchTimeout)
  customSearchTimeout = setTimeout(() => {
    loadCustomPricings()
  }, 300)
})

// 同步模型搜索防抖
let syncedSearchTimeout: ReturnType<typeof setTimeout> | null = null
watch(syncedSearchQuery, () => {
  if (activeTab.value !== 'synced') return
  if (syncedSearchTimeout) clearTimeout(syncedSearchTimeout)
  syncedSearchTimeout = setTimeout(() => {
    loadSyncedPricings()
  }, 300)
})

// 切换标签时
watch(activeTab, (newTab) => {
  if (newTab === 'synced') {
    loadSyncedPricings()
  }
})

// 切换匹配模式
const handleMatchModeChange = async () => {
  if (store.settings.modelPricing) {
    store.settings.modelPricing.matchMode = localMatchMode.value
    await store.saveSettings()
  }
}

// 同步价格（从 API 拉取数据到数据库）
const syncPricing = async () => {
  syncing.value = true
  syncError.value = ''

  try {
    await invoke('sync_model_pricing_from_api')

    // 更新同步时间
    if (store.settings.modelPricing) {
      store.settings.modelPricing.lastSyncTime = Date.now()
      await store.saveSettings()
    }

    // 重新加载同步模型总数
    await loadSyncedCount()
    // 如果当前在同步模型页面，重新加载列表
    if (activeTab.value === 'synced') {
      await loadSyncedPricings()
    }
  } catch (e) {
    syncError.value = String(e)
    console.error('[ModelPricingSettings] Failed to sync pricing:', e)
  } finally {
    syncing.value = false
  }
}

// 清空同步数据
const clearing = ref(false)
const clearSyncedPricings = async () => {
  clearing.value = true
  syncError.value = ''

  try {
    await invoke('clear_synced_model_pricings')

    // 清除同步时间
    if (store.settings.modelPricing) {
      store.settings.modelPricing.lastSyncTime = null
      await store.saveSettings()
    }

    // 重新加载同步模型总数
    await loadSyncedCount()
    // 如果当前在同步模型页面，重新加载列表
    if (activeTab.value === 'synced') {
      await loadSyncedPricings()
    }
  } catch (e) {
    syncError.value = String(e)
    console.error('[ModelPricingSettings] Failed to clear synced pricing:', e)
  } finally {
    clearing.value = false
  }
}

// 添加自定义模型
const addCustom = () => {
  editingPricing.value = null
  copySourcePricing.value = null
  showEditModal.value = true
}

// 编辑模型
const editPricing = (pricing: ModelPricingConfig) => {
  editingPricing.value = { ...pricing }
  copySourcePricing.value = null
  showEditModal.value = true
}

// 复制模型
const copySourcePricing = ref<ModelPricingConfig | null>(null)
const copyPricing = (pricing: ModelPricingConfig) => {
  editingPricing.value = null
  copySourcePricing.value = { ...pricing }
  showEditModal.value = true
}

// 打开价格应用弹窗
const openApplyModal = (pricing: ModelPricingConfig) => {
  applyingPricing.value = pricing
  showApplyModal.value = true
}

// 价格应用完成回调
const onPricingApplied = async (_count: number) => {
  showApplyModal.value = false
  applyingPricing.value = null
  // 刷新当前标签页数据以反映新费用
  if (activeTab.value === 'custom') {
    await loadCustomPricings()
  } else {
    await loadSyncedPricings()
  }
}

// 删除模型 - 显示确认弹窗
const confirmDelete = (modelId: string) => {
  deletingModelId.value = modelId
  showDeleteConfirm.value = true
}

// 执行删除
const deletePricing = async () => {
  if (!deletingModelId.value) return

  try {
    await invoke('delete_model_pricing', { modelId: deletingModelId.value })
    await loadCustomPricings()
    showDeleteConfirm.value = false
    deletingModelId.value = null
  } catch (e) {
    console.error('[ModelPricingSettings] Failed to delete pricing:', e)
  }
}

// 保存错误
const saveError = ref('')

// 保存编辑
const savePricing = async (pricing: ModelPricingConfig) => {
  saveError.value = ''
  try {
    // 确保 source 为 custom
    const customPricing = { ...pricing, source: 'custom' }

    if (editingPricing.value) {
      await invoke('update_custom_model_pricing', { pricing: customPricing })
    } else {
      await invoke('add_custom_model_pricing', { pricing: customPricing })
    }

    await loadCustomPricings()
    showEditModal.value = false
  } catch (e) {
    console.error('[ModelPricingSettings] Failed to save pricing:', e)
    saveError.value = String(e)
  }
}

// 格式化价格（支持多货币）
const formatPrice = (price: number | undefined): string => {
  if (price === undefined || price === null) return '-'
  return formatCostUtil(price, store.settings.currency, 2)
}

// 格式化时间
const formatTime = (timestamp: number | null): string => {
  if (!timestamp) return '-'
  const locale = store.settings.locale.replace('_', '-')
  return new Date(timestamp).toLocaleString(locale, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit'
  })
}
</script>

<template>
  <div class="flex flex-col gap-3">
    <!-- 头部 -->
    <div class="flex items-center justify-between px-1">
      <button @click="emit('back')" class="flex items-center gap-1 text-blue-500 text-[13px] hover:text-blue-600 transition-colors">
        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
        </svg>
        {{ t(store.settings.locale, 'common.dashboard') }}
      </button>
      <h2 class="text-sm font-semibold text-gray-800 dark:text-gray-100">
        {{ t(store.settings.locale, 'settings.modelPricingTitle') }}
      </h2>
      <div class="w-20"></div>
    </div>

    <!-- 加载状态 -->
    <div v-if="customLoading" class="text-center py-8 text-gray-400 text-sm">
      {{ t(store.settings.locale, 'common.syncing') }}
    </div>

    <!-- 加载错误 -->
    <div v-else-if="loadError" class="bg-red-50 dark:bg-red-900/20 rounded-xl p-3 text-xs text-red-600 dark:text-red-400">
      {{ t(store.settings.locale, 'common.loadError') }}: {{ loadError }}
    </div>

    <!-- 内容 -->
    <template v-else>
      <!-- 匹配模式设置 -->
      <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 p-3">
        <div class="flex items-center justify-between">
          <div>
            <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.modelPricingMatchMode') }}</div>
            <div class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'settings.modelPricingMatchModeDesc') }}</div>
          </div>
          <div class="flex gap-1">
            <button
              @click="localMatchMode = 'fuzzy'; handleMatchModeChange()"
              :class="[
                'px-2.5 py-1 rounded-md text-xs font-medium transition-all',
                localMatchMode === 'fuzzy' ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400'
              ]"
            >
              {{ t(store.settings.locale, 'settings.modelPricingMatchModeFuzzy') }}
            </button>
            <button
              @click="localMatchMode = 'exact'; handleMatchModeChange()"
              :class="[
                'px-2.5 py-1 rounded-md text-xs font-medium transition-all',
                localMatchMode === 'exact' ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400'
              ]"
            >
              {{ t(store.settings.locale, 'settings.modelPricingMatchModeExact') }}
            </button>
          </div>
        </div>

        <div class="mt-2 space-y-1 text-[10px] leading-snug text-gray-500 dark:text-gray-400">
          <div>{{ t(store.settings.locale, 'settings.modelPricingMatchModeExactDesc') }}</div>
          <div>{{ t(store.settings.locale, 'settings.modelPricingMatchModeFuzzyDesc') }}</div>
        </div>

        <!-- 最后同步时间 -->
        <div v-if="store.settings.modelPricing?.lastSyncTime" class="text-[10px] text-gray-400 mt-2 pt-2 border-t border-gray-50 dark:border-neutral-800/50">
          {{ t(store.settings.locale, 'settings.modelPricingLastSync') }}: {{ formatTime(store.settings.modelPricing.lastSyncTime) }}
        </div>
      </div>

      <!-- 同步错误 -->
      <div v-if="syncError" class="bg-red-50 dark:bg-red-900/20 rounded-xl p-3 text-xs text-red-600 dark:text-red-400">
        {{ t(store.settings.locale, 'settings.modelPricingSyncError') }}: {{ syncError }}
      </div>

      <!-- 标签页切换 -->
      <div class="flex gap-2">
        <button
          @click="activeTab = 'custom'"
          :class="[
            'flex-1 py-2 rounded-xl text-[13px] font-medium transition-all',
            activeTab === 'custom'
              ? 'bg-purple-500 text-white'
              : 'bg-white dark:bg-[#1C1C1E] text-gray-600 dark:text-gray-400 border border-gray-100 dark:border-neutral-800'
          ]"
        >
          {{ t(store.settings.locale, 'settings.modelPricingTabCustom') }} ({{ customPricings.length }})
        </button>
        <button
          @click="activeTab = 'synced'"
          :class="[
            'flex-1 py-2 rounded-xl text-[13px] font-medium transition-all',
            activeTab === 'synced'
              ? 'bg-blue-500 text-white'
              : 'bg-white dark:bg-[#1C1C1E] text-gray-600 dark:text-gray-400 border border-gray-100 dark:border-neutral-800'
          ]"
        >
          {{ t(store.settings.locale, 'settings.modelPricingTabSynced') }} ({{ syncedTotalCount }})
        </button>
      </div>

      <!-- 自定义模型列表 -->
      <template v-if="activeTab === 'custom'">
        <!-- 搜索框 -->
        <div class="relative">
          <svg class="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
          </svg>
          <input
            v-model="customSearchQuery"
            type="text"
            :placeholder="t(store.settings.locale, 'settings.modelPricingSearch')"
            class="w-full pl-9 pr-10 py-2 bg-white dark:bg-[#1C1C1E] border border-gray-100 dark:border-neutral-800 rounded-xl text-sm outline-none focus:border-blue-400 transition-colors"
          />
          <span v-if="customPricings.length > 0" class="absolute right-3 top-1/2 -translate-y-1/2 text-[10px] text-gray-400">{{ customPricings.length }}</span>
        </div>

        <!-- 添加按钮 -->
        <button
          @click="addCustom"
          class="w-full py-2 bg-purple-500 hover:bg-purple-600 rounded-xl text-[12px] font-medium text-white transition-colors flex items-center justify-center gap-1.5"
        >
          <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
          </svg>
          {{ t(store.settings.locale, 'settings.modelPricingAdd') }}
        </button>

        <!-- 列表 -->
        <div v-if="customLoading" class="text-center py-8 text-gray-400 text-sm">
          {{ t(store.settings.locale, 'common.syncing') }}
        </div>
        <div v-else class="space-y-2 max-h-[250px] overflow-y-auto">
          <div v-if="customPricings.length === 0" class="text-center py-8 text-gray-400 text-sm">
            {{ customSearchQuery ? t(store.settings.locale, 'settings.modelPricingNoResults') : t(store.settings.locale, 'settings.modelPricingNoCustom') }}
          </div>

          <div
            v-for="pricing in customPricings"
            :key="pricing.modelId"
            class="bg-white dark:bg-[#1C1C1E] rounded-lg border border-gray-100 dark:border-neutral-800 px-2.5 py-2"
          >
            <!-- 第一行：模型名称 + 模型ID -->
            <div class="flex items-center gap-2 mb-1.5">
              <span class="px-1.5 py-0.5 bg-gray-100 dark:bg-neutral-800 rounded text-[11px] font-medium text-gray-700 dark:text-gray-300 shrink-0">
                {{ pricing.displayName || pricing.modelId }}
              </span>
              <span
                v-if="pricing.displayName"
                class="text-[10px] text-gray-500 dark:text-gray-400 truncate flex-1"
                @mouseenter="showTooltip(pricing.modelId, $event)"
                @mousemove="updateTooltipPosition"
                @mouseleave="hideTooltip"
              >{{ pricing.modelId }}</span>

              <!-- 操作按钮 -->
              <div class="flex gap-0.5 shrink-0 ml-auto">
                <button @click="editPricing(pricing)" class="p-1 hover:bg-gray-100 dark:hover:bg-neutral-700 rounded transition-colors">
                  <svg class="w-3.5 h-3.5 text-gray-400 hover:text-blue-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                  </svg>
                </button>
                <button @click="copyPricing(pricing)" class="p-1 hover:bg-gray-100 dark:hover:bg-neutral-700 rounded transition-colors" :title="t(store.settings.locale, 'settings.modelPricingCopy')">
                  <svg class="w-3.5 h-3.5 text-gray-400 hover:text-purple-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <rect x="9" y="9" width="13" height="13" rx="2" ry="2" stroke-width="2" /><path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
                  </svg>
                </button>
                <button @click="openApplyModal(pricing)" class="p-1 hover:bg-gray-100 dark:hover:bg-neutral-700 rounded transition-colors" :title="t(store.settings.locale, 'settings.pricingApply')">
                  <svg class="w-3.5 h-3.5 text-gray-400 hover:text-orange-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <rect width="16" height="20" x="4" y="2" rx="2" stroke-width="2" /><line x1="8" x2="16" y1="6" y2="6" stroke-width="2" /><path d="M16 10h.01M12 10h.01M8 10h.01M16 14h.01M12 14h.01M8 14h.01M16 18h.01M12 18h.01M8 18h.01" stroke-width="2" stroke-linecap="round" />
                  </svg>
                </button>
                <button @click="confirmDelete(pricing.modelId)" class="p-1 hover:bg-gray-100 dark:hover:bg-neutral-700 rounded transition-colors">
                  <svg class="w-3.5 h-3.5 text-gray-400 hover:text-red-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                  </svg>
                </button>
              </div>
            </div>

            <!-- 第二行：价格信息 -->
            <div class="flex items-center gap-3 text-[10px] text-gray-500 font-mono">
              <span><span class="text-gray-400">输入:</span>{{ formatPrice(pricing.inputPrice) }}/M</span>
              <span><span class="text-gray-400">输出:</span>{{ formatPrice(pricing.outputPrice) }}/M</span>
              <span v-if="pricing.cacheReadPrice"><span class="text-gray-400">缓存读:</span>{{ formatPrice(pricing.cacheReadPrice) }}/M</span>
              <span v-if="pricing.cacheWritePrice"><span class="text-gray-400">缓存写:</span>{{ formatPrice(pricing.cacheWritePrice) }}/M</span>
            </div>
          </div>
        </div>
      </template>

      <!-- 同步模型列表 -->
      <template v-else>
        <!-- 搜索框 -->
        <div class="relative">
          <svg class="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
          </svg>
          <input
            v-model="syncedSearchQuery"
            type="text"
            :placeholder="t(store.settings.locale, 'settings.modelPricingSearch')"
            class="w-full pl-9 pr-10 py-2 bg-white dark:bg-[#1C1C1E] border border-gray-100 dark:border-neutral-800 rounded-xl text-sm outline-none focus:border-blue-400 transition-colors"
          />
          <span v-if="syncedTotalCount > 0" class="absolute right-3 top-1/2 -translate-y-1/2 text-[10px] text-gray-400">{{ syncedTotalCount }}</span>
        </div>

        <!-- 更新和清空按钮 -->
        <div class="flex gap-2">
          <button
            @click="syncPricing"
            :disabled="syncing"
            class="flex-1 py-2 bg-blue-500 hover:bg-blue-600 disabled:bg-blue-300 rounded-xl text-[12px] font-medium text-white transition-colors flex items-center justify-center gap-1.5"
          >
            <svg v-if="syncing" class="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
              <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
              <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
            </svg>
            <svg v-else class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
            </svg>
            {{ syncing ? t(store.settings.locale, 'common.syncing') : t(store.settings.locale, 'settings.modelPricingSync') }}
          </button>
          <button
            @click="clearSyncedPricings"
            :disabled="clearing"
            class="flex-1 py-2 bg-gray-500 hover:bg-gray-600 disabled:bg-gray-300 rounded-xl text-[12px] font-medium text-white transition-colors flex items-center justify-center gap-1.5"
          >
            <svg v-if="clearing" class="w-4 h-4 animate-spin" fill="none" viewBox="0 0 24 24">
              <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
              <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
            </svg>
            <svg v-else class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
            </svg>
            {{ clearing ? t(store.settings.locale, 'settings.modelPricingClearing') : t(store.settings.locale, 'settings.modelPricingClear') }}
          </button>
        </div>

        <!-- 列表 -->
        <div v-if="syncedLoading" class="text-center py-8 text-gray-400 text-sm">
          {{ t(store.settings.locale, 'common.syncing') }}
        </div>
        <div v-else class="space-y-1 max-h-[300px] overflow-y-auto">
          <div v-if="syncedPricings.length === 0" class="text-center py-8 text-gray-400 text-sm">
            {{ syncedSearchQuery ? t(store.settings.locale, 'settings.modelPricingNoResults') : t(store.settings.locale, 'settings.modelPricingNoData') }}
          </div>

          <div
            v-for="pricing in syncedPricings"
            :key="pricing.modelId"
            class="bg-white dark:bg-[#1C1C1E] rounded-lg border border-gray-100 dark:border-neutral-800 px-2.5 py-2"
          >
            <!-- 第一行：模型名称 + 模型ID + 操作按钮 -->
            <div class="flex items-center gap-2 mb-1.5">
              <span class="px-1.5 py-0.5 bg-gray-100 dark:bg-neutral-800 rounded text-[11px] font-medium text-gray-700 dark:text-gray-300 shrink-0">
                {{ pricing.displayName || pricing.modelId }}
              </span>
              <span
                v-if="pricing.displayName"
                class="text-[10px] text-gray-500 dark:text-gray-400 truncate flex-1"
                @mouseenter="showTooltip(pricing.modelId, $event)"
                @mousemove="updateTooltipPosition"
                @mouseleave="hideTooltip"
              >{{ pricing.modelId }}</span>
              <button @click="openApplyModal(pricing)" class="p-1 hover:bg-gray-100 dark:hover:bg-neutral-700 rounded transition-colors shrink-0 ml-auto" :title="t(store.settings.locale, 'settings.pricingApply')">
                <svg class="w-3.5 h-3.5 text-gray-400 hover:text-orange-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <rect width="16" height="20" x="4" y="2" rx="2" stroke-width="2" /><line x1="8" x2="16" y1="6" y2="6" stroke-width="2" /><path d="M16 10h.01M12 10h.01M8 10h.01M16 14h.01M12 14h.01M8 14h.01M16 18h.01M12 18h.01M8 18h.01" stroke-width="2" stroke-linecap="round" />
                </svg>
              </button>
            </div>

            <!-- 第二行：价格信息 -->
            <div class="flex items-center gap-3 text-[10px] text-gray-500 font-mono">
              <span><span class="text-gray-400">输入:</span>{{ formatPrice(pricing.inputPrice) }}/M</span>
              <span><span class="text-gray-400">输出:</span>{{ formatPrice(pricing.outputPrice) }}/M</span>
              <span v-if="pricing.cacheReadPrice"><span class="text-gray-400">缓存读:</span>{{ formatPrice(pricing.cacheReadPrice) }}/M</span>
              <span v-if="pricing.cacheWritePrice"><span class="text-gray-400">缓存写:</span>{{ formatPrice(pricing.cacheWritePrice) }}/M</span>
            </div>
          </div>
        </div>
      </template>
    </template>

    <!-- 自定义 Tooltip -->
    <Teleport to="body">
      <div
        v-if="tooltipVisible"
        class="fixed z-[9999] px-2 py-1 text-[11px] text-white bg-gray-800 dark:bg-gray-700 rounded-md shadow-lg max-w-[300px] break-all pointer-events-none"
        :style="{ left: tooltipX + 'px', top: tooltipY + 'px' }"
      >
        {{ tooltipContent }}
      </div>
    </Teleport>

    <!-- 编辑模态框 -->
    <Teleport to="body">
      <div v-if="showEditModal" class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50" @click.self="showEditModal = false">
        <div class="bg-white dark:bg-[#1C1C1E] rounded-2xl shadow-xl overflow-hidden" @click.stop>
          <!-- 保存错误提示 -->
          <div v-if="saveError" class="mx-3 mt-3 px-2.5 py-2 bg-red-50 dark:bg-red-900/20 rounded-lg text-xs text-red-600 dark:text-red-400">
            {{ t(store.settings.locale, 'settings.modelPricingSaveError') }}: {{ saveError }}
          </div>
          <ModelPricingEditModal
            :pricing="editingPricing"
            :copy-source="copySourcePricing"
            :locale="store.settings.locale"
            @save="savePricing"
            @close="showEditModal = false; copySourcePricing = null"
          />
        </div>
      </div>
    </Teleport>

    <!-- 删除确认弹窗 -->
    <Teleport to="body">
      <div v-if="showDeleteConfirm" class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50" @click.self="showDeleteConfirm = false">
        <div class="bg-white dark:bg-[#1C1C1E] rounded-2xl shadow-xl w-[280px] overflow-hidden" @click.stop>
          <div class="p-4">
            <div class="text-center">
              <div class="w-10 h-10 mx-auto mb-3 rounded-full bg-red-100 dark:bg-red-900/30 flex items-center justify-center">
                <svg class="w-5 h-5 text-red-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                </svg>
              </div>
              <h3 class="text-sm font-medium text-gray-900 dark:text-gray-100 mb-1">
                {{ t(store.settings.locale, 'settings.modelPricingDeleteTitle') }}
              </h3>
              <p class="text-xs text-gray-500 dark:text-gray-400">
                {{ t(store.settings.locale, 'settings.modelPricingConfirmDelete') }}
              </p>
            </div>
          </div>
          <div class="flex border-t border-gray-100 dark:border-neutral-800">
            <button
              @click="showDeleteConfirm = false"
              class="flex-1 py-2.5 text-xs font-medium text-gray-600 dark:text-gray-400 hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors"
            >
              {{ t(store.settings.locale, 'common.cancel') }}
            </button>
            <button
              @click="deletePricing"
              class="flex-1 py-2.5 text-xs font-medium text-red-500 hover:bg-red-50 dark:hover:bg-red-900/20 transition-colors border-l border-gray-100 dark:border-neutral-800"
            >
              {{ t(store.settings.locale, 'common.confirm') }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- 价格应用弹窗 -->
    <Teleport to="body">
      <div v-if="showApplyModal && applyingPricing" class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50" @click.self="showApplyModal = false">
        <div class="bg-white dark:bg-[#1C1C1E] rounded-2xl shadow-xl overflow-hidden" @click.stop>
          <PricingApplyModal
            :pricing="applyingPricing"
            @close="showApplyModal = false"
            @applied="onPricingApplied"
          />
        </div>
      </div>
    </Teleport>
  </div>
</template>
