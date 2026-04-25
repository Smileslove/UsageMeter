<script setup lang="ts">
import { ref, watch, computed, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import type { BillingType, WindowName, DataSource, ThemeMode } from '../types'
import ModelPricingSettings from '../components/ModelPricingSettings.vue'

const store = useMonitorStore()

// 子页面状态
const subView = ref<'main' | 'model-pricing'>('main')

// 监听 subView 变化，触发子组件刷新
const modelPricingKey = ref(0)
watch(subView, (newVal) => {
  if (newVal === 'model-pricing') {
    // 强制重新创建组件
    modelPricingKey.value++
  }
})

// 返回主设置
const goBack = () => {
  subView.value = 'main'
}

// 进入模型价格设置
const openModelPricing = () => {
  subView.value = 'model-pricing'
}

// 本地状态用于双向绑定
const localLocale = ref(store.settings.locale)
const localRefreshInterval = ref(store.settings.refreshIntervalSeconds)
const localBillingType = ref<BillingType>(store.settings.billingType)
const localSummaryWindow = ref<WindowName>(store.settings.summaryWindow)
const localDataSource = ref<DataSource>(store.settings.dataSource)
const localTheme = ref<ThemeMode>(store.settings.theme || 'system')
const localWarningThreshold = ref(store.settings.warningThreshold)
const localCriticalThreshold = ref(store.settings.criticalThreshold)
const localIncludeErrorRequests = ref(store.settings.proxy.includeErrorRequests ?? true)

// 开机自启动状态（从配置初始化，页面加载后同步系统状态）
const autoStartEnabled = ref(store.settings.autoStart)

// 本地配额状态
const localQuotas = ref(JSON.parse(JSON.stringify(store.settings.quotas)))

// 监听 store 变化同步到本地
watch(() => store.settings.locale, (val) => {
  localLocale.value = val
})

watch(() => store.settings.refreshIntervalSeconds, (val) => {
  localRefreshInterval.value = val
})

watch(() => store.settings.billingType, (val) => {
  localBillingType.value = val
})

watch(() => store.settings.summaryWindow, (val) => {
  localSummaryWindow.value = val
})

watch(() => store.settings.dataSource, (val) => {
  localDataSource.value = val
})

watch(() => store.settings.theme, (val) => {
  localTheme.value = val || 'system'
})

watch(() => store.settings.warningThreshold, (val) => {
  localWarningThreshold.value = val
})

watch(() => store.settings.criticalThreshold, (val) => {
  localCriticalThreshold.value = val
})

watch(() => store.settings.proxy.includeErrorRequests, (val) => {
  localIncludeErrorRequests.value = val ?? true
})

watch(() => store.settings.quotas, (val) => {
  localQuotas.value = JSON.parse(JSON.stringify(val))
}, { deep: true })

// 更新本地状态并保存
const handleLocaleChange = async () => {
  store.settings.locale = localLocale.value
  await store.saveSettings()
}

const handleRefreshIntervalChange = async () => {
  const value = Math.max(5, Math.min(300, Number(localRefreshInterval.value) || 30))
  localRefreshInterval.value = value
  store.settings.refreshIntervalSeconds = value
  await store.saveSettings()
}

const handleBillingTypeChange = async () => {
  store.settings.billingType = localBillingType.value
  await store.saveSettings()
}

const handleSummaryWindowChange = async () => {
  // 自动启用选中的汇总窗口
  const quota = localQuotas.value.find((q: any) => q.window === localSummaryWindow.value)
  if (quota && !quota.enabled) {
    quota.enabled = true
    store.settings.quotas = JSON.parse(JSON.stringify(localQuotas.value))
  }
  store.settings.summaryWindow = localSummaryWindow.value
  await store.saveSettings()
}

const handleDataSourceChange = async () => {
  store.settings.dataSource = localDataSource.value
  await store.saveSettings()
}

const handleThemeChange = async () => {
  store.settings.theme = localTheme.value
  await store.saveSettings()
}

const handleWarningThresholdChange = async () => {
  let value = Math.max(0, Math.min(100, Number(localWarningThreshold.value) || 70))
  // 确保警告阈值 < 危险阈值
  if (value >= store.settings.criticalThreshold) {
    value = store.settings.criticalThreshold - 1
  }
  localWarningThreshold.value = value
  store.settings.warningThreshold = value
  await store.saveSettings()
}

const handleCriticalThresholdChange = async () => {
  let value = Math.max(0, Math.min(100, Number(localCriticalThreshold.value) || 90))
  // 确保危险阈值 > 警告阈值
  if (value <= store.settings.warningThreshold) {
    value = store.settings.warningThreshold + 1
  }
  localCriticalThreshold.value = value
  store.settings.criticalThreshold = value
  await store.saveSettings()
}

const handleIncludeErrorRequestsChange = async () => {
  store.settings.proxy.includeErrorRequests = localIncludeErrorRequests.value
  await store.saveSettings()
}

// 获取窗口的 i18n key
const getWindowLabelKey = (window: WindowName): string => {
  const map: Record<WindowName, string> = {
    '5h': 'settings.window5h',
    '24h': 'settings.window24h',
    'today': 'settings.windowToday',
    '7d': 'settings.window7d',
    '30d': 'settings.window30d',
    'current_month': 'settings.windowCurrentMonth'
  }
  return map[window]
}

// 获取配额
const getQuota = (window: WindowName) => {
  return localQuotas.value.find((q: any) => q.window === window)
}

// 切换窗口启用状态
const toggleWindowEnabled = async (window: WindowName) => {
  const quota = getQuota(window)
  if (quota) {
    quota.enabled = !quota.enabled
    store.settings.quotas = JSON.parse(JSON.stringify(localQuotas.value))
    await store.saveSettings()
  }
}

// 更新限额
const updateTokenLimit = async (window: WindowName, value: string) => {
  const quota = getQuota(window)
  if (quota) {
    const num = value ? parseInt(value.replace(/,/g, ''), 10) : null
    quota.tokenLimit = num && num > 0 ? num : null
    store.settings.quotas = JSON.parse(JSON.stringify(localQuotas.value))
    await store.saveSettings()
  }
}

const updateRequestLimit = async (window: WindowName, value: string) => {
  const quota = getQuota(window)
  if (quota) {
    const num = value ? parseInt(value.replace(/,/g, ''), 10) : null
    quota.requestLimit = num && num > 0 ? num : null
    store.settings.quotas = JSON.parse(JSON.stringify(localQuotas.value))
    await store.saveSettings()
  }
}

// 格式化数字显示
const formatNumber = (num: number | null): string => {
  if (num === null) return ''
  return num.toLocaleString()
}

// 是否显示 Token 限额
const showTokenLimit = computed(() => {
  return localBillingType.value === 'token' || localBillingType.value === 'both'
})

// 是否显示请求限额
const showRequestLimit = computed(() => {
  return localBillingType.value === 'request' || localBillingType.value === 'both'
})

// 窗口顺序
const windowOrder: WindowName[] = ['5h', '24h', 'today', '7d', '30d', 'current_month']

// 代理控制
const proxyEnabled = computed(() => store.isProxyRunning)

const toggleProxy = async () => {
  await store.toggleProxy()
}

// 开机自启动控制
const toggleAutoStart = async () => {
  try {
    if (autoStartEnabled.value) {
      await invoke('disable_autostart')
      autoStartEnabled.value = false
    } else {
      await invoke('enable_autostart')
      autoStartEnabled.value = true
    }
    // 同步保存到 settings
    store.settings.autoStart = autoStartEnabled.value
    await store.saveSettings()
  } catch (e) {
    console.error('Failed to toggle autostart:', e)
    // 发生错误时，恢复到系统实际状态
    try {
      autoStartEnabled.value = await invoke('is_autostart_enabled')
    } catch {
      // 忽略错误
    }
  }
}

// 初始化：从系统获取实际的 autostart 状态，与配置同步
onMounted(async () => {
  try {
    const systemState = await invoke<boolean>('is_autostart_enabled')
    autoStartEnabled.value = systemState
    // 如果配置和系统状态不一致，同步配置
    if (store.settings.autoStart !== systemState) {
      store.settings.autoStart = systemState
      await store.saveSettings()
    }
  } catch (e) {
    console.error('Failed to check autostart status:', e)
    // 如果系统查询失败，使用配置中的值
    autoStartEnabled.value = store.settings.autoStart
  }
})

// 代理状态信息
const proxyStatusInfo = computed(() => {
  if (!store.proxyStatus) return null
  const status = store.proxyStatus
  return {
    port: status.port,
    uptime: formatUptime(status.uptimeSeconds),
    totalRequests: status.totalRequests,
    activeConnections: status.activeConnections,
    configTakenOver: status.configTakenOver,
    recordCount: status.recordCount
  }
})

// 格式化运行时间
const formatUptime = (seconds: number): string => {
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`
}
</script>

<template>
  <div class="relative">
    <!-- 模型价格子页面 -->
    <ModelPricingSettings
      v-show="subView === 'model-pricing'"
      :key="modelPricingKey"
      @back="goBack"
    />

    <!-- 主设置页面 -->
    <div v-show="subView !== 'model-pricing'" class="space-y-5 animate-in fade-in zoom-in-95 duration-300 pb-6">
    <div class="space-y-2">
      <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider px-1">{{ t(store.settings.locale, 'settings.general') }}</h3>
      <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden divide-y divide-gray-50 dark:divide-neutral-800/50 shadow-sm">
        <div class="p-3 px-4 flex items-center justify-between text-[13px]">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.locale') }}</span>
          <select
            v-model="localLocale"
            @change="handleLocaleChange"
            class="bg-transparent text-gray-500 dark:text-gray-400 text-sm outline-none text-right tracking-tight cursor-pointer appearance-none"
          >
            <option value="zh-CN">{{ t(store.settings.locale, 'settings.zhCN') }}</option>
            <option value="zh-TW">{{ t(store.settings.locale, 'settings.zhTW') }}</option>
            <option value="en-US">{{ t(store.settings.locale, 'settings.enUS') }}</option>
          </select>
        </div>
        <div class="p-3 px-4 flex items-center justify-between text-[13px]">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.theme') }}</span>
          <div class="flex gap-1.5">
            <button
              v-for="theme in ['light', 'dark', 'system'] as ThemeMode[]"
              :key="theme"
              @click="localTheme = theme; handleThemeChange()"
              :class="[
                'px-2.5 py-1 rounded-md text-xs font-medium transition-all',
                localTheme === theme
                  ? 'bg-blue-500 text-white'
                  : 'bg-gray-100 dark:bg-neutral-700 text-gray-600 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-neutral-600'
              ]"
            >
              {{ t(store.settings.locale, `settings.theme${theme.charAt(0).toUpperCase() + theme.slice(1)}`) }}
            </button>
          </div>
        </div>
        <div class="p-3 px-4 flex items-center justify-between text-[13px]">
          <div class="flex flex-col">
            <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.autoStart') }}</span>
            <span class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.autoStartDesc') }}</span>
          </div>
          <div
            :class="[
              'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
              autoStartEnabled ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
            ]"
            @click="toggleAutoStart"
          >
            <div
              :class="[
                'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                autoStartEnabled ? 'right-[2px]' : 'left-[2px]'
              ]"
            ></div>
          </div>
        </div>
        <div class="p-3 px-4 flex items-center justify-between text-[13px]">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.summaryWindow') }}</span>
          <select
            v-model="localSummaryWindow"
            @change="handleSummaryWindowChange"
            class="bg-transparent text-gray-500 dark:text-gray-400 text-sm outline-none text-right tracking-tight cursor-pointer appearance-none"
          >
            <option v-for="window in windowOrder" :key="window" :value="window">
              {{ t(store.settings.locale, getWindowLabelKey(window)) }}
            </option>
          </select>
        </div>
        <div class="p-3 px-4 flex items-center justify-between text-[13px]">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.warningThreshold') }}</span>
          <div class="flex items-center gap-1">
            <input
              type="number"
              v-model.number="localWarningThreshold"
              @blur="handleWarningThresholdChange"
              @keyup.enter="handleWarningThresholdChange"
              min="0"
              max="99"
              class="w-12 bg-transparent text-gray-500 dark:text-gray-400 text-sm font-mono outline-none text-right p-0"
            />
            <span class="text-xs text-gray-400">%</span>
          </div>
        </div>
        <div class="p-3 px-4 flex items-center justify-between text-[13px]">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.criticalThreshold') }}</span>
          <div class="flex items-center gap-1">
            <input
              type="number"
              v-model.number="localCriticalThreshold"
              @blur="handleCriticalThresholdChange"
              @keyup.enter="handleCriticalThresholdChange"
              min="1"
              max="100"
              class="w-12 bg-transparent text-gray-500 dark:text-gray-400 text-sm font-mono outline-none text-right p-0"
            />
            <span class="text-xs text-gray-400">%</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 数据统计方式 -->
    <div class="space-y-2">
      <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider px-1">{{ t(store.settings.locale, 'settings.dataSource') }}</h3>
      <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden divide-y divide-gray-50 dark:divide-neutral-800/50 shadow-sm">
        <div class="p-3 px-4">
          <div class="flex gap-2">
            <button
              v-for="source in ['ccusage', 'proxy'] as DataSource[]"
              :key="source"
              @click="localDataSource = source; handleDataSourceChange()"
              :class="[
                'flex-1 py-2 px-3 rounded-lg text-xs font-medium transition-all',
                localDataSource === source
                  ? 'bg-blue-500 text-white'
                  : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-neutral-700'
              ]"
            >
              {{ t(store.settings.locale, `settings.dataSource${source.charAt(0).toUpperCase() + source.slice(1)}`) }}
            </button>
          </div>
          <!-- 数据源说明 -->
          <p class="mt-2 text-[10px] text-gray-400 dark:text-gray-500 leading-relaxed">
            {{ t(store.settings.locale, localDataSource === 'ccusage' ? 'settings.dataSourceCcusageDesc' : 'settings.dataSourceProxyDesc') }}
          </p>
        </div>

        <!-- 代理开关 -->
        <div class="p-3 px-4 flex items-center justify-between text-[13px]">
          <div class="flex flex-col">
            <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.interceptRequests') }}</span>
            <span v-if="proxyEnabled && proxyStatusInfo" class="text-[10px] text-gray-400 mt-0.5">
              {{ t(store.settings.locale, 'settings.proxyRunning') }} · {{ t(store.settings.locale, 'settings.port') }} {{ proxyStatusInfo.port }} · {{ proxyStatusInfo.uptime }}
            </span>
            <span v-else-if="localDataSource === 'proxy' && !proxyEnabled" class="text-[10px] text-amber-500 mt-0.5">
              {{ t(store.settings.locale, 'settings.startProxyHint') }}
            </span>
          </div>
          <!-- iOS styled switch -->
          <div
            :class="[
              'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
              proxyEnabled ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
            ]"
            @click="toggleProxy"
          >
            <div
              :class="[
                'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                proxyEnabled ? 'right-[2px]' : 'left-[2px]'
              ]"
            ></div>
          </div>
        </div>

        <!-- 代理详细状态（仅在运行时显示） -->
        <div v-if="proxyEnabled && proxyStatusInfo" class="p-3 px-4 bg-gray-50 dark:bg-neutral-800/50">
          <div class="grid grid-cols-2 gap-2 text-[11px]">
            <div class="flex items-center gap-1.5">
              <span :class="['w-2 h-2 rounded-full', proxyStatusInfo.configTakenOver ? 'bg-green-500' : 'bg-amber-500']"></span>
              <span class="text-gray-500 dark:text-gray-400">
                {{ proxyStatusInfo.configTakenOver ? t(store.settings.locale, 'settings.configTakenOver') : t(store.settings.locale, 'settings.configNotTakenOver') }}
              </span>
            </div>
            <div class="flex items-center gap-1.5">
              <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.requestCount') }}:</span>
              <span class="text-gray-700 dark:text-gray-300 font-mono">{{ proxyStatusInfo.totalRequests }}</span>
            </div>
            <div class="flex items-center gap-1.5">
              <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.recordCount') }}:</span>
              <span class="text-gray-700 dark:text-gray-300 font-mono">{{ proxyStatusInfo.recordCount }}</span>
            </div>
            <div class="flex items-center gap-1.5">
              <span class="text-gray-500 dark:text-gray-400">{{ t(store.settings.locale, 'settings.activeConnections') }}:</span>
              <span class="text-gray-700 dark:text-gray-300 font-mono">{{ proxyStatusInfo.activeConnections }}</span>
            </div>
          </div>
        </div>

        <!-- 包含错误请求（仅代理模式显示） -->
        <div v-if="localDataSource === 'proxy'" class="p-3 px-4 flex items-center justify-between text-[13px]">
          <div class="flex flex-col">
            <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.includeErrorRequests') }}</span>
            <span class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.includeErrorRequestsDesc') }}</span>
          </div>
          <!-- iOS styled switch -->
          <div
            :class="[
              'w-10 h-6 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
              localIncludeErrorRequests ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
            ]"
            @click="localIncludeErrorRequests = !localIncludeErrorRequests; handleIncludeErrorRequestsChange()"
          >
            <div
              :class="[
                'w-[20px] h-[20px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                localIncludeErrorRequests ? 'right-[2px]' : 'left-[2px]'
              ]"
            ></div>
          </div>
        </div>

        <!-- 刷新间隔 -->
        <div class="p-3 px-4 flex items-center justify-between text-[13px]">
          <span class="text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.refreshInterval') }}</span>
          <div class="flex items-center gap-1">
            <input
              type="number"
              v-model.number="localRefreshInterval"
              @blur="handleRefreshIntervalChange"
              @keyup.enter="handleRefreshIntervalChange"
              min="5"
              max="300"
              class="w-12 bg-transparent text-gray-500 dark:text-gray-400 text-sm font-mono outline-none text-right p-0"
            />
            <span class="text-xs text-gray-400">{{ t(store.settings.locale, 'common.seconds') }}</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 模型价格设置入口 -->
    <div class="space-y-2">
      <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider px-1">{{ t(store.settings.locale, 'settings.modelPricing') }}</h3>
      <div
        @click="openModelPricing"
        class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 p-3 px-4 cursor-pointer hover:bg-gray-50 dark:hover:bg-neutral-800/50 transition-colors shadow-sm"
      >
        <div class="flex items-center justify-between">
          <div>
            <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.modelPricing') }}</div>
            <div class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.modelPricingDesc') }}</div>
          </div>
          <svg class="w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
          </svg>
        </div>
      </div>
    </div>

    <!-- 窗口配额设置 -->
    <div class="space-y-2">
      <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider px-1">{{ t(store.settings.locale, 'settings.quotaTitle') }}</h3>

      <!-- 整合的配额卡片 -->
      <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden shadow-sm">
        <!-- 计费类型选择 -->
        <div class="p-3 px-4 border-b border-gray-50 dark:border-neutral-800/50">
          <div class="flex items-center justify-between">
            <span class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.billingType') }}</span>
            <div class="flex gap-1.5">
              <button
                v-for="type in ['token', 'request', 'both'] as BillingType[]"
                :key="type"
                @click="localBillingType = type; handleBillingTypeChange()"
                :class="[
                  'py-1 px-2.5 rounded-md text-[11px] font-medium transition-all',
                  localBillingType === type
                    ? 'bg-blue-500 text-white'
                    : 'bg-gray-100 dark:bg-neutral-800 text-gray-600 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-neutral-700'
                ]"
              >
                {{ t(store.settings.locale, `settings.billingType${type.charAt(0).toUpperCase() + type.slice(1)}`) }}
              </button>
            </div>
          </div>
        </div>

        <!-- 各窗口配额 -->
        <div class="divide-y divide-gray-50 dark:divide-neutral-800/50">
          <div
            v-for="window in windowOrder"
            :key="window"
            :class="[
              'transition-all',
              getQuota(window)?.enabled ? 'bg-white dark:bg-[#1C1C1E]' : 'bg-gray-50/50 dark:bg-neutral-900/30'
            ]"
          >
            <div :class="getQuota(window)?.enabled ? 'p-3 px-4' : 'p-2.5 px-4'">
              <!-- 窗口标题行 -->
              <div class="flex items-center justify-between">
                <span :class="['text-[13px] font-medium transition-colors', getQuota(window)?.enabled ? 'text-gray-700 dark:text-gray-200' : 'text-gray-400 dark:text-gray-500']">
                  {{ t(store.settings.locale, getWindowLabelKey(window)) }}
                </span>
                <!-- iOS 风格开关 -->
                <div
                  :class="[
                    'w-9 h-5 rounded-full relative cursor-pointer flex items-center shrink-0 transition-colors',
                    getQuota(window)?.enabled ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
                  ]"
                  @click="toggleWindowEnabled(window)"
                >
                  <div
                    :class="[
                      'w-[18px] h-[18px] bg-white rounded-full absolute shadow shadow-black/10 transition-all',
                      getQuota(window)?.enabled ? 'right-[1px]' : 'left-[1px]'
                    ]"
                  ></div>
                </div>
              </div>

              <!-- 限额输入（仅在启用时显示） -->
              <div v-if="getQuota(window)?.enabled" class="mt-2.5 flex gap-3">
                <!-- Token 限额 -->
                <div v-if="showTokenLimit" class="flex-1">
                  <div class="flex items-center justify-between">
                    <span class="text-[10px] text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.tokenLimitPlaceholder') }}</span>
                  </div>
                  <input
                    type="text"
                    :value="formatNumber(getQuota(window)?.tokenLimit)"
                    @blur="(e) => updateTokenLimit(window, (e.target as HTMLInputElement).value)"
                    @keyup.enter="(e) => updateTokenLimit(window, (e.target as HTMLInputElement).value)"
                    :placeholder="t(store.settings.locale, 'settings.unlimited')"
                    class="w-full mt-1 bg-gray-50 dark:bg-neutral-800 text-gray-600 dark:text-gray-300 text-xs font-mono outline-none text-right p-1.5 rounded border border-gray-200 dark:border-neutral-700 focus:border-blue-400 focus:ring-1 focus:ring-blue-400"
                  />
                </div>

                <!-- 请求限额 -->
                <div v-if="showRequestLimit" class="flex-1">
                  <div class="flex items-center justify-between">
                    <span class="text-[10px] text-gray-400 dark:text-gray-500">{{ t(store.settings.locale, 'settings.requestLimitPlaceholder') }}</span>
                  </div>
                  <input
                    type="text"
                    :value="formatNumber(getQuota(window)?.requestLimit)"
                    @blur="(e) => updateRequestLimit(window, (e.target as HTMLInputElement).value)"
                    @keyup.enter="(e) => updateRequestLimit(window, (e.target as HTMLInputElement).value)"
                    :placeholder="t(store.settings.locale, 'settings.unlimited')"
                    class="w-full mt-1 bg-gray-50 dark:bg-neutral-800 text-gray-600 dark:text-gray-300 text-xs font-mono outline-none text-right p-1.5 rounded border border-gray-200 dark:border-neutral-700 focus:border-blue-400 focus:ring-1 focus:ring-blue-400"
                  />
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 加载/保存状态 -->
    <div v-if="store.saving" class="text-center text-xs text-gray-400">
      {{ t(store.settings.locale, 'common.saving') }}
    </div>
    <div v-if="store.error" class="text-center text-xs text-red-500">
      {{ store.error }}
    </div>
  </div>
  </div>
</template>
