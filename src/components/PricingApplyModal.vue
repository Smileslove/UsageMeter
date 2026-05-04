<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { t } from '../i18n'
import type { ModelPricingConfig, ClientToolProfile, ApiSource } from '../types'
import { useMonitorStore } from '../stores/monitor'
import { formatCost } from '../utils/format'
import { ChevronDown, Globe, LayoutGrid } from 'lucide-vue-next'
import LobeIcon from './LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'

const store = useMonitorStore()

const props = defineProps<{
  pricing: ModelPricingConfig
}>()

const emit = defineEmits<{
  close: []
  applied: [count: number]
}>()

// 筛选配置
const matchMode = ref(store.settings.modelPricing?.matchMode ?? 'fuzzy')
const timeRange = ref<'all' | '7d' | '30d' | 'custom'>('all')
const customStart = ref('')
const customEnd = ref('')
const selectedClientTool = ref<string | null>(null)
const selectedApiSourceId = ref<string | null>(null)

// 可选项
const clientToolProfiles = computed<ClientToolProfile[]>(() =>
  store.settings.clientTools?.profiles ?? []
)
const apiSources = computed<ApiSource[]>(() =>
  store.settings.sourceAware?.sources ?? []
)

// 预览结果
const previewLoading = ref(false)
const previewError = ref('')
interface ModelMatchCount {
  model: string
  count: number
}
interface PreviewResult {
  matchedCount: number
  totalCurrentCost: number
  modelCounts: ModelMatchCount[]
}
const previewResult = ref<PreviewResult | null>(null)

// 应用状态
const applyLoading = ref(false)
const applyError = ref('')
const applyDone = ref(false)
const applyCount = ref(0)
const newCost = ref(0)

const timeRangeStartMs = computed(() => {
  if (timeRange.value === 'all') return null
  if (timeRange.value === '7d') return Date.now() - 7 * 24 * 60 * 60 * 1000
  if (timeRange.value === '30d') return Date.now() - 30 * 24 * 60 * 60 * 1000
  if (timeRange.value === 'custom' && customStart.value) return new Date(customStart.value).getTime()
  return null
})

const timeRangeEndMs = computed(() => {
  if (timeRange.value === 'all') return null
  if (timeRange.value !== 'custom') return null
  if (customEnd.value) return new Date(customEnd.value).getTime() + 24 * 60 * 60 * 1000 - 1
  return null
})

const apiSourceKeyPrefixes = computed(() => {
  if (!selectedApiSourceId.value) return null
  const source = apiSources.value.find(s => s.id === selectedApiSourceId.value)
  if (!source || !source.apiKeyPrefixes?.length) return null
  return source.apiKeyPrefixes
})

// 预览请求
let previewTimer: ReturnType<typeof setTimeout> | null = null
let previewRequestId = 0 // 用于标识当前请求，解决竞态问题
const doPreview = async () => {
  if (timeRange.value === 'custom' && !customStart.value) {
    previewResult.value = null
    return
  }

  const currentRequestId = ++previewRequestId
  previewLoading.value = true
  previewError.value = ''
  try {
    const result = await invoke<PreviewResult>('preview_pricing_apply', {
      modelId: props.pricing.modelId,
      matchMode: matchMode.value,
      timeRangeStart: timeRangeStartMs.value,
      timeRangeEnd: timeRangeEndMs.value,
      clientToolFilter: selectedClientTool.value,
      apiSourceKeyPrefixes: apiSourceKeyPrefixes.value,
    })
    // 只处理最新请求的结果，忽略过期请求
    if (currentRequestId === previewRequestId) {
      previewResult.value = result
    }
  } catch (e) {
    // 只处理最新请求的错误
    if (currentRequestId === previewRequestId) {
      previewError.value = e instanceof Error ? e.message : String(e)
      previewResult.value = null
    }
  } finally {
    // 只处理最新请求的 loading 状态
    if (currentRequestId === previewRequestId) {
      previewLoading.value = false
    }
  }
}

const schedulePreview = () => {
  if (applyDone.value) return
  if (previewTimer) clearTimeout(previewTimer)
  previewTimer = setTimeout(doPreview, 300)
}

watch([matchMode, timeRange, customStart, customEnd, selectedClientTool, selectedApiSourceId], schedulePreview)

onMounted(doPreview)

// 执行应用
let applyRequestId = 0
const doApply = async () => {
  if (applyLoading.value || applyDone.value) return
  const currentRequestId = ++applyRequestId
  applyLoading.value = true
  applyError.value = ''
  try {
    const count = await invoke<number>('apply_pricing_to_records', {
      modelId: props.pricing.modelId,
      pricing: props.pricing,
      matchMode: matchMode.value,
      timeRangeStart: timeRangeStartMs.value,
      timeRangeEnd: timeRangeEndMs.value,
      clientToolFilter: selectedClientTool.value,
      apiSourceKeyPrefixes: apiSourceKeyPrefixes.value,
    })
    if (currentRequestId !== applyRequestId) return
    applyCount.value = count
    applyDone.value = true

    // 重新查询以获取更新后的费用
    try {
      const updated = await invoke<PreviewResult>('preview_pricing_apply', {
        modelId: props.pricing.modelId,
        matchMode: matchMode.value,
        timeRangeStart: timeRangeStartMs.value,
        timeRangeEnd: timeRangeEndMs.value,
        clientToolFilter: selectedClientTool.value,
        apiSourceKeyPrefixes: apiSourceKeyPrefixes.value,
      })
      if (currentRequestId !== applyRequestId) return
      newCost.value = updated.totalCurrentCost
    } catch {
      newCost.value = 0
    }
  } catch (e) {
    if (currentRequestId !== applyRequestId) return
    applyError.value = e instanceof Error ? e.message : String(e)
  } finally {
    if (currentRequestId === applyRequestId) {
      applyLoading.value = false
    }
  }
}

const handleClose = () => {
  if (applyDone.value) {
    emit('applied', applyCount.value)
  }
  emit('close')
}

const matchedCount = computed(() => previewResult.value?.matchedCount ?? 0)
const currentCost = computed(() => previewResult.value?.totalCurrentCost ?? 0)

const getClientToolDisplayName = (profile: ClientToolProfile) => {
  return profile.displayName || profile.tool
}

const getApiSourceDisplayName = (source: ApiSource) => {
  return source.displayName || source.id
}

// 下拉框状态
const toolDropdownOpen = ref(false)
const sourceDropdownOpen = ref(false)
const toolDropdownRef = ref<HTMLElement | null>(null)
const sourceDropdownRef = ref<HTMLElement | null>(null)
const iconFailed = ref(false)

const currentToolProfile = computed(() => {
  if (!selectedClientTool.value) return null
  return clientToolProfiles.value.find(p => p.tool === selectedClientTool.value) || null
})
const currentToolIcon = computed(() => {
  if (!currentToolProfile.value) return null
  return currentToolProfile.value.icon || TOOL_LOBE_ICONS[currentToolProfile.value.tool] || null
})
const currentToolLabel = computed(() => {
  if (!selectedClientTool.value) return t(store.settings.locale, 'settings.pricingApplySourceAll')
  return currentToolProfile.value?.displayName || selectedClientTool.value
})

const currentApiSource = computed(() => {
  if (!selectedApiSourceId.value) return null
  return apiSources.value.find(s => s.id === selectedApiSourceId.value) || null
})
const currentApiSourceColor = computed(() => currentApiSource.value?.color || '#9CA3AF')
const currentApiSourceLabel = computed(() => {
  if (!selectedApiSourceId.value) return t(store.settings.locale, 'settings.pricingApplySourceAll')
  return currentApiSource.value ? getApiSourceDisplayName(currentApiSource.value) : t(store.settings.locale, 'settings.pricingApplySourceAll')
})

const selectTool = (tool: string | null) => {
  selectedClientTool.value = tool
  toolDropdownOpen.value = false
  iconFailed.value = false // 切换工具时重置图标加载状态
}
const selectApiSource = (sourceId: string | null) => {
  selectedApiSourceId.value = sourceId
  sourceDropdownOpen.value = false
}

const handleClickOutside = (event: MouseEvent) => {
  if (toolDropdownRef.value && !toolDropdownRef.value.contains(event.target as Node)) {
    toolDropdownOpen.value = false
  }
  if (sourceDropdownRef.value && !sourceDropdownRef.value.contains(event.target as Node)) {
    sourceDropdownOpen.value = false
  }
}
onMounted(() => document.addEventListener('click', handleClickOutside))
onUnmounted(() => {
  if (previewTimer) clearTimeout(previewTimer)
  document.removeEventListener('click', handleClickOutside)
})
</script>

<template>
  <div class="p-3 w-72">
    <!-- 标题 -->
    <div class="flex items-center justify-between mb-3">
      <h3 class="text-[13px] font-semibold text-gray-800 dark:text-gray-100">
        {{ t(store.settings.locale, 'settings.pricingApplyTitle') }}
      </h3>
      <button @click="handleClose" class="p-1 hover:bg-gray-100 dark:hover:bg-neutral-800 rounded-lg transition-colors">
        <svg class="w-3.5 h-3.5 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
        </svg>
      </button>
    </div>

    <!-- 模型信息（一行） -->
    <div class="mb-3 flex items-center gap-2 px-2 py-1.5 bg-gray-50 dark:bg-neutral-800 rounded-lg">
      <span class="px-1.5 py-0.5 bg-gray-200 dark:bg-neutral-700 rounded text-[11px] font-medium text-gray-700 dark:text-gray-300 shrink-0">
        {{ pricing.displayName || pricing.modelId }}
      </span>
      <span v-if="pricing.displayName" class="text-[10px] text-gray-400 truncate">{{ pricing.modelId }}</span>
    </div>

    <!-- 匹配模式 -->
    <div class="mb-2">
      <label class="block text-[10px] text-gray-500 mb-1">{{ t(store.settings.locale, 'settings.pricingApplyMatchMode') }}</label>
      <div class="flex gap-1">
        <button
          @click="matchMode = 'fuzzy'"
          :class="[
            'flex-1 py-1.5 rounded-lg text-[11px] font-medium transition-all',
            matchMode === 'fuzzy' ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400'
          ]"
        >
          {{ t(store.settings.locale, 'settings.modelPricingMatchModeFuzzy') }}
        </button>
        <button
          @click="matchMode = 'exact'"
          :class="[
            'flex-1 py-1.5 rounded-lg text-[11px] font-medium transition-all',
            matchMode === 'exact' ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400'
          ]"
        >
          {{ t(store.settings.locale, 'settings.modelPricingMatchModeExact') }}
        </button>
      </div>
    </div>

    <!-- 范围筛选 -->
    <div class="mb-2">
      <label class="block text-[10px] text-gray-500 mb-1">{{ t(store.settings.locale, 'settings.pricingApplyTimeRange') }}</label>
      <!-- 时间范围 -->
      <div class="grid grid-cols-4 gap-1 mb-1.5">
        <button
          @click="timeRange = 'all'"
          :class="[
            'py-1.5 rounded-lg text-[11px] font-medium transition-all',
            timeRange === 'all' ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400'
          ]"
        >
          {{ t(store.settings.locale, 'settings.pricingApplyAllTime') }}
        </button>
        <button
          @click="timeRange = '7d'"
          :class="[
            'py-1.5 rounded-lg text-[11px] font-medium transition-all',
            timeRange === '7d' ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400'
          ]"
        >
          {{ t(store.settings.locale, 'settings.pricingApplyLast7d') }}
        </button>
        <button
          @click="timeRange = '30d'"
          :class="[
            'py-1.5 rounded-lg text-[11px] font-medium transition-all',
            timeRange === '30d' ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400'
          ]"
        >
          {{ t(store.settings.locale, 'settings.pricingApplyLast30d') }}
        </button>
        <button
          @click="timeRange = 'custom'"
          :class="[
            'py-1.5 rounded-lg text-[11px] font-medium transition-all',
            timeRange === 'custom' ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400'
          ]"
        >
          {{ t(store.settings.locale, 'settings.pricingApplyCustom') }}
        </button>
      </div>
      <!-- 自定义日期 -->
      <div v-if="timeRange === 'custom'" class="flex gap-2 mb-1.5">
        <div class="flex-1">
          <input v-model="customStart" type="date" class="w-full px-2 py-1 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-[11px] outline-none focus:border-blue-400 transition-colors" />
        </div>
        <div class="flex-1">
          <input v-model="customEnd" type="date" class="w-full px-2 py-1 bg-gray-50 dark:bg-neutral-800 border border-gray-200 dark:border-neutral-700 rounded-lg text-[11px] outline-none focus:border-blue-400 transition-colors" />
        </div>
      </div>
      <!-- 来源筛选（一行两个下拉按钮） -->
      <div class="flex gap-2">
        <!-- API 来源（左侧） -->
        <div v-if="apiSources.length > 0" class="flex-1">
          <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(store.settings.locale, 'settings.pricingApplySourceApi') }}</label>
          <div ref="sourceDropdownRef" class="relative">
            <button
              @click="sourceDropdownOpen = !sourceDropdownOpen"
              :class="[
                'w-full flex items-center gap-1.5 px-2 py-1.5 rounded-lg border text-[11px] transition-all',
                selectedApiSourceId !== null
                  ? 'bg-blue-50 dark:bg-blue-500/15 border-blue-300 dark:border-blue-500/40 text-blue-700 dark:text-blue-300'
                  : 'bg-gray-50 dark:bg-neutral-800 border-gray-200 dark:border-neutral-700 text-gray-600 dark:text-gray-400'
              ]"
            >
              <Globe v-if="!selectedApiSourceId" class="w-3.5 h-3.5 shrink-0" />
              <span
                v-else
                class="w-2.5 h-2.5 rounded-full shrink-0"
                :style="{ backgroundColor: currentApiSourceColor }"
              ></span>
              <span class="truncate flex-1 text-left">{{ currentApiSourceLabel }}</span>
              <ChevronDown :class="['w-3 h-3 shrink-0 transition-transform', sourceDropdownOpen && 'rotate-180']" />
            </button>
            <Transition
              enter-active-class="transition ease-out duration-100"
              enter-from-class="transform opacity-0 scale-95"
              enter-to-class="transform opacity-100 scale-100"
              leave-active-class="transition ease-in duration-75"
              leave-from-class="transform opacity-100 scale-100"
              leave-to-class="transform opacity-0 scale-95"
            >
              <div
                v-if="sourceDropdownOpen"
                class="absolute top-full left-0 mt-1 w-full bg-white dark:bg-[#1C1C1E] rounded-xl shadow-lg border border-gray-100 dark:border-neutral-800 z-50 max-h-[160px] overflow-y-auto"
              >
                <button
                  @click="selectApiSource(null)"
                  :class="[
                    'w-full flex items-center gap-2 px-2.5 py-2 text-[11px] text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors sticky top-0 bg-white dark:bg-[#1C1C1E]',
                    !selectedApiSourceId ? 'text-blue-600 dark:text-blue-400 font-medium' : 'text-gray-700 dark:text-gray-200'
                  ]"
                >
                  <Globe class="w-3.5 h-3.5" />
                  {{ t(store.settings.locale, 'settings.pricingApplySourceAll') }}
                </button>
                <div class="border-t border-gray-50 dark:border-neutral-800">
                  <button
                    v-for="source in apiSources"
                    :key="source.id"
                    @click="selectApiSource(source.id)"
                    :class="[
                      'w-full flex items-center gap-2 px-2.5 py-2 text-[11px] text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors',
                      selectedApiSourceId === source.id ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium' : 'text-gray-700 dark:text-gray-200'
                    ]"
                  >
                    <span
                      class="w-2.5 h-2.5 rounded-full shrink-0"
                      :style="{ backgroundColor: source.color }"
                    ></span>
                    <span class="truncate">{{ getApiSourceDisplayName(source) }}</span>
                  </button>
                </div>
              </div>
            </Transition>
          </div>
        </div>
        <!-- 软件来源（右侧） -->
        <div v-if="clientToolProfiles.length > 0" class="flex-1">
          <label class="block text-[10px] text-gray-500 mb-0.5">{{ t(store.settings.locale, 'settings.pricingApplySourceClientTool') }}</label>
          <div ref="toolDropdownRef" class="relative">
            <button
              @click="toolDropdownOpen = !toolDropdownOpen"
              :class="[
                'w-full flex items-center gap-1.5 px-2 py-1.5 rounded-lg border text-[11px] transition-all',
                selectedClientTool !== null
                  ? 'bg-blue-50 dark:bg-blue-500/15 border-blue-300 dark:border-blue-500/40 text-blue-700 dark:text-blue-300'
                  : 'bg-gray-50 dark:bg-neutral-800 border-gray-200 dark:border-neutral-700 text-gray-600 dark:text-gray-400'
              ]"
            >
              <LayoutGrid v-if="!selectedClientTool" class="w-3.5 h-3.5 shrink-0" />
              <LobeIcon
                v-else-if="currentToolIcon && !iconFailed"
                :slug="currentToolIcon"
                :size="14"
                @error="iconFailed = true"
              />
              <span v-else class="w-2 h-2 rounded-full bg-gray-400 shrink-0"></span>
              <span class="truncate flex-1 text-left">{{ currentToolLabel }}</span>
              <ChevronDown :class="['w-3 h-3 shrink-0 transition-transform', toolDropdownOpen && 'rotate-180']" />
            </button>
            <Transition
              enter-active-class="transition ease-out duration-100"
              enter-from-class="transform opacity-0 scale-95"
              enter-to-class="transform opacity-100 scale-100"
              leave-active-class="transition ease-in duration-75"
              leave-from-class="transform opacity-100 scale-100"
              leave-to-class="transform opacity-0 scale-95"
            >
              <div
                v-if="toolDropdownOpen"
                class="absolute top-full left-0 mt-1 w-full bg-white dark:bg-[#1C1C1E] rounded-xl shadow-lg border border-gray-100 dark:border-neutral-800 z-50 max-h-[160px] overflow-y-auto"
              >
                <button
                  @click="selectTool(null)"
                  :class="[
                    'w-full flex items-center gap-2 px-2.5 py-2 text-[11px] text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors sticky top-0 bg-white dark:bg-[#1C1C1E]',
                    !selectedClientTool ? 'text-blue-600 dark:text-blue-400 font-medium' : 'text-gray-700 dark:text-gray-200'
                  ]"
                >
                  <LayoutGrid class="w-3.5 h-3.5" />
                  {{ t(store.settings.locale, 'settings.pricingApplySourceAll') }}
                </button>
                <div class="border-t border-gray-50 dark:border-neutral-800">
                  <button
                    v-for="profile in clientToolProfiles"
                    :key="profile.id"
                    @click="selectTool(profile.tool)"
                    :class="[
                      'w-full flex items-center gap-2 px-2.5 py-2 text-[11px] text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors',
                      selectedClientTool === profile.tool ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium' : 'text-gray-700 dark:text-gray-200'
                    ]"
                  >
                    <LobeIcon
                      v-if="profile.icon || TOOL_LOBE_ICONS[profile.tool]"
                      :slug="profile.icon || TOOL_LOBE_ICONS[profile.tool]"
                      :size="14"
                      @error="() => {}"
                    />
                    <span v-else class="w-2 h-2 rounded-full bg-gray-400 shrink-0"></span>
                    <span class="truncate">{{ getClientToolDisplayName(profile) }}</span>
                  </button>
                </div>
              </div>
            </Transition>
          </div>
        </div>
      </div>
    </div>

    <!-- 分隔线 -->
    <div class="border-t border-gray-100 dark:border-neutral-800 my-2.5"></div>

    <!-- 匹配记录数（始终显示） -->
    <div class="flex items-center justify-between px-2 py-1.5 bg-gray-50 dark:bg-neutral-800 rounded-lg mb-1.5">
      <span class="text-[10px] text-gray-500">{{ t(store.settings.locale, 'settings.pricingApplyMatched') }}</span>
      <span class="text-[12px] font-semibold text-gray-800 dark:text-gray-100">
        <template v-if="previewLoading">
          <svg class="w-3.5 h-3.5 animate-spin text-blue-400 inline" fill="none" viewBox="0 0 24 24">
            <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
            <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
          </svg>
        </template>
        <template v-else-if="previewError">--</template>
        <template v-else>{{ matchedCount || '--' }}</template>
      </span>
    </div>

    <!-- 当前费用（始终显示） -->
    <div class="flex items-center justify-between px-2 py-1.5 bg-gray-50 dark:bg-neutral-800 rounded-lg mb-1.5">
      <span class="text-[10px] text-gray-500">{{ t(store.settings.locale, 'settings.pricingApplyCurrentCost') }}</span>
      <span class="text-[12px] font-mono text-gray-800 dark:text-gray-100">
        <template v-if="previewLoading">--</template>
        <template v-else-if="previewError">--</template>
        <template v-else>{{ matchedCount ? formatCost(currentCost, store.settings.currency) : '--' }}</template>
      </span>
    </div>

    <!-- 匹配模型列表（始终显示） -->
    <div class="px-2 py-1.5 bg-gray-50 dark:bg-neutral-800 rounded-lg mb-1.5">
      <div class="text-[10px] text-gray-500 mb-1">{{ t(store.settings.locale, 'settings.pricingApplyModels') }}</div>
      <div v-if="previewResult && previewResult.modelCounts.length > 0" class="max-h-[100px] overflow-y-auto space-y-0.5">
        <div
          v-for="mc in previewResult.modelCounts"
          :key="mc.model"
          class="flex items-center justify-between text-[10px]"
        >
          <span class="text-gray-600 dark:text-gray-300 truncate flex-1 mr-2">{{ mc.model }}</span>
          <span class="text-gray-400 font-mono shrink-0">{{ mc.count }}</span>
        </div>
      </div>
      <div v-else class="text-[10px] text-gray-400">--</div>
    </div>

    <!-- 预览错误 -->
    <div v-if="previewError" class="text-[10px] text-red-500 mb-1.5">{{ previewError }}</div>

    <!-- 应用错误 -->
    <div v-if="applyError" class="text-[10px] text-red-500 mb-1.5">{{ applyError }}</div>

    <!-- 应用进度 -->
    <div v-if="applyLoading" class="mb-2.5">
      <div class="flex items-center gap-2 mb-1">
        <svg class="w-3.5 h-3.5 animate-spin text-orange-500" fill="none" viewBox="0 0 24 24">
          <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
          <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
        </svg>
        <span class="text-[11px] text-orange-600 dark:text-orange-400">{{ t(store.settings.locale, 'settings.pricingApplyConfirm') }}...</span>
      </div>
      <div class="w-full h-1.5 bg-gray-100 dark:bg-neutral-800 rounded-full overflow-hidden">
        <div class="h-full bg-orange-500 rounded-full animate-pulse" style="width: 100%"></div>
      </div>
    </div>

    <!-- 完成状态 -->
    <div v-if="applyDone" class="mb-2.5 p-2 bg-green-50 dark:bg-green-900/20 rounded-lg">
      <div class="flex items-center gap-1.5 mb-1.5">
        <svg class="w-3.5 h-3.5 text-green-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
        </svg>
        <span class="text-[11px] text-green-700 dark:text-green-300">
          {{ t(store.settings.locale, 'settings.pricingApplySuccess').replace('{count}', String(applyCount)) }}
        </span>
      </div>
      <!-- 更新后价格 -->
      <div class="space-y-0.5 text-[10px]">
        <div class="flex items-center justify-between">
          <span class="text-green-600 dark:text-green-400">{{ t(store.settings.locale, 'settings.pricingApplyCurrentCost') }}</span>
          <span class="font-mono text-gray-500 line-through">{{ formatCost(currentCost, store.settings.currency) }}</span>
        </div>
        <div class="flex items-center justify-between">
          <span class="text-green-600 dark:text-green-400">{{ t(store.settings.locale, 'settings.pricingApplyNewCost') }}</span>
          <span class="font-mono font-semibold text-green-700 dark:text-green-300">{{ formatCost(newCost, store.settings.currency) }}</span>
        </div>
      </div>
    </div>

    <!-- 按钮 -->
    <div class="flex gap-2 pt-2 border-t border-gray-100 dark:border-neutral-800">
      <button
        @click="handleClose"
        class="flex-1 py-1.5 bg-gray-100 dark:bg-neutral-800 hover:bg-gray-200 dark:hover:bg-neutral-700 rounded-lg text-xs text-gray-600 dark:text-gray-300 transition-colors"
      >
        {{ applyDone ? t(store.settings.locale, 'common.confirm') : t(store.settings.locale, 'common.cancel') }}
      </button>
      <button
        v-if="!applyDone && matchedCount > 0"
        @click="doApply"
        :disabled="applyLoading"
        class="flex-1 py-1.5 bg-orange-500 hover:bg-orange-600 disabled:opacity-50 disabled:cursor-not-allowed rounded-lg text-xs text-white transition-colors"
      >
        {{ t(store.settings.locale, 'settings.pricingApply') }}
      </button>
    </div>
  </div>
</template>
