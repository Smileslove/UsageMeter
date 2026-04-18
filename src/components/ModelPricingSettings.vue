<script setup lang="ts">
import { ref, onMounted, watch, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import type { ModelPricingConfig } from '../types'
import ModelPricingEditModal from './ModelPricingEditModal.vue'

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
const searchQuery = ref('')
const syncing = ref(false)
const syncError = ref('')
const showEditModal = ref(false)
const editingPricing = ref<ModelPricingConfig | null>(null)
const loading = ref(true)
const loadError = ref('')

// 当前标签页：custom 或 synced
const activeTab = ref<'custom' | 'synced'>('custom')

// 数据库中的价格列表
const pricingList = ref<ModelPricingConfig[]>([])
const totalCount = ref(0)

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
const customPricings = computed(() =>
  pricingList.value.filter(p => p.source === 'custom')
)

const syncedPricings = computed(() =>
  pricingList.value.filter(p => p.source !== 'custom')
)

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

// 加载价格列表
const loadPricings = async () => {
  loading.value = true
  loadError.value = ''

  try {
    const jsonString = await invoke<string>('search_model_pricing', {
      query: searchQuery.value || null,
      limit: 100,
      offset: 0
    })

    const result = JSON.parse(jsonString) as ModelPricingSearchResult
    pricingList.value = result.pricings || []
    totalCount.value = result.total || 0
  } catch (e) {
    console.error('[ModelPricingSettings] Failed to load pricings:', e)
    loadError.value = String(e)
    pricingList.value = []
    totalCount.value = 0
  } finally {
    loading.value = false
  }
}

// 组件挂载时加载数据
onMounted(() => {
  loadPricings()
})

// 搜索防抖
let searchTimeout: ReturnType<typeof setTimeout> | null = null
watch(searchQuery, () => {
  if (activeTab.value !== 'synced') return
  if (searchTimeout) clearTimeout(searchTimeout)
  searchTimeout = setTimeout(() => {
    loadPricings()
  }, 300)
})

// 切换标签时
watch(activeTab, (newTab) => {
  if (newTab === 'synced') {
    loadPricings()
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

    // 重新加载列表
    await loadPricings()
  } catch (e) {
    syncError.value = String(e)
    console.error('[ModelPricingSettings] Failed to sync pricing:', e)
  } finally {
    syncing.value = false
  }
}

// 添加自定义模型
const addCustom = () => {
  editingPricing.value = null
  showEditModal.value = true
}

// 编辑模型
const editPricing = (pricing: ModelPricingConfig) => {
  editingPricing.value = { ...pricing }
  showEditModal.value = true
}

// 删除模型
const deletePricing = async (modelId: string) => {
  if (!confirm(t(store.settings.locale, 'settings.modelPricingConfirmDelete'))) return

  try {
    await invoke('delete_model_pricing', { modelId })
    await loadPricings()
  } catch (e) {
    console.error('[ModelPricingSettings] Failed to delete pricing:', e)
  }
}

// 保存编辑
const savePricing = async (pricing: ModelPricingConfig) => {
  try {
    // 确保 source 为 custom
    const customPricing = { ...pricing, source: 'custom' }

    if (editingPricing.value) {
      await invoke('update_custom_model_pricing', { pricing: customPricing })
    } else {
      await invoke('add_custom_model_pricing', { pricing: customPricing })
    }

    await loadPricings()
    showEditModal.value = false
  } catch (e) {
    console.error('[ModelPricingSettings] Failed to save pricing:', e)
  }
}

// 格式化价格
const formatPrice = (price: number | undefined): string => {
  if (price === undefined || price === null) return '-'
  return `$${price.toFixed(2)}`
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
      <button
        @click="syncPricing"
        :disabled="syncing"
        class="flex items-center justify-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium transition-all bg-blue-500 text-white hover:bg-blue-600 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        <svg v-if="syncing" class="w-3 h-3 animate-spin" fill="none" viewBox="0 0 24 24">
          <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
          <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
        </svg>
        <span v-if="!syncing">{{ t(store.settings.locale, 'settings.modelPricingSync') }}</span>
        <span v-else>{{ t(store.settings.locale, 'settings.modelPricingSyncing') }}</span>
      </button>
    </div>

    <!-- 加载状态 -->
    <div v-if="loading" class="text-center py-8 text-gray-400 text-sm">
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
          {{ t(store.settings.locale, 'settings.modelPricingTabSynced') }} ({{ totalCount }})
        </button>
      </div>

      <!-- 自定义模型列表 -->
      <template v-if="activeTab === 'custom'">
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
        <div class="space-y-2 max-h-[300px] overflow-y-auto">
          <div v-if="customPricings.length === 0" class="text-center py-8 text-gray-400 text-sm">
            {{ t(store.settings.locale, 'settings.modelPricingNoCustom') }}
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
                <button @click="deletePricing(pricing.modelId)" class="p-1 hover:bg-gray-100 dark:hover:bg-neutral-700 rounded transition-colors">
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
            v-model="searchQuery"
            type="text"
            :placeholder="t(store.settings.locale, 'settings.modelPricingSearch')"
            class="w-full pl-9 pr-10 py-2 bg-white dark:bg-[#1C1C1E] border border-gray-100 dark:border-neutral-800 rounded-xl text-sm outline-none focus:border-blue-400 transition-colors"
          />
          <span v-if="totalCount > 0" class="absolute right-3 top-1/2 -translate-y-1/2 text-[10px] text-gray-400">{{ totalCount }}</span>
        </div>

        <!-- 列表 -->
        <div class="space-y-1 max-h-[300px] overflow-y-auto">
          <div v-if="syncedPricings.length === 0" class="text-center py-8 text-gray-400 text-sm">
            {{ searchQuery ? t(store.settings.locale, 'settings.modelPricingNoResults') : t(store.settings.locale, 'settings.modelPricingNoData') }}
          </div>

          <div
            v-for="pricing in syncedPricings"
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
          <ModelPricingEditModal
            :pricing="editingPricing"
            :locale="store.settings.locale"
            @save="savePricing"
            @close="showEditModal = false"
          />
        </div>
      </div>
    </Teleport>
  </div>
</template>
