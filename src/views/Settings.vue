<script setup lang="ts">
import { ref, watch, computed, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../stores/monitor'
import { t, windowNameLabel } from '../i18n'
import { WINDOW_ORDER, type BillingType, type WindowName, type DataSource, type ThemeMode, type ToolTakeoverStatus } from '../types'
import ModelPricingSettings from '../components/ModelPricingSettings.vue'
import ApiSourceList from '../components/ApiSourceList.vue'
import WindowQuotaSettings from '../components/WindowQuotaSettings.vue'
import CurrencySettings from '../components/CurrencySettings.vue'
import LobeIcon from '../components/LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'

const store = useMonitorStore()

// 子页面状态
const subView = ref<'main' | 'model-pricing' | 'api-sources' | 'window-quotas' | 'currency'>('main')

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

// 进入 API 来源管理
const openApiSources = () => {
  subView.value = 'api-sources'
}

// 进入窗口配额管理
const openWindowQuotas = () => {
  subView.value = 'window-quotas'
}

// 进入货币设置
const openCurrency = () => {
  subView.value = 'currency'
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
const takeoverStatuses = ref<ToolTakeoverStatus[]>([])
const takeoverLoading = ref<Record<string, boolean>>({})
const showCodexOauthWarning = ref(false)
let resolveCodexOauthWarning: ((accepted: boolean) => void) | null = null

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

// 代理控制
const proxyEnabled = computed(() => store.isProxyRunning)

const toggleProxy = async () => {
  await store.toggleProxy()
  await loadTakeoverStatuses()
}

const loadTakeoverStatuses = async () => {
  try {
    takeoverStatuses.value = await invoke<ToolTakeoverStatus[]>('get_takeover_statuses')
  } catch {
    takeoverStatuses.value = []
  }
}

const managedToolProfiles = computed(() => {
  return store.settings.clientTools.profiles.filter(profile => ['claude_code', 'codex'].includes(profile.tool))
})

const takeoverStatusFor = (tool: string) => {
  return takeoverStatuses.value.find(status => status.tool === tool)
}

const takeoverActiveFor = (tool: string) => {
  return takeoverStatusFor(tool)?.takeoverActive ?? false
}

const takeoverEnabledFor = (tool: string) => {
  return takeoverStatusFor(tool)?.enabled ?? managedToolProfiles.value.find(profile => profile.tool === tool)?.enabled ?? false
}

const getToolIcon = (tool: string, icon?: string) => {
  return icon || TOOL_LOBE_ICONS[tool] || null
}

const confirmCodexOauthRisk = () => new Promise<boolean>((resolve) => {
  resolveCodexOauthWarning = resolve
  showCodexOauthWarning.value = true
})

const closeCodexOauthWarning = (accepted: boolean) => {
  showCodexOauthWarning.value = false
  resolveCodexOauthWarning?.(accepted)
  resolveCodexOauthWarning = null
}

const toggleToolTakeover = async (tool: string) => {
  const nextEnabled = !takeoverEnabledFor(tool)
  if (tool === 'codex' && nextEnabled && takeoverStatusFor(tool)?.authMode === 'chat_gpt') {
    const accepted = await confirmCodexOauthRisk()
    if (!accepted) {
      return
    }
  }
  takeoverLoading.value = { ...takeoverLoading.value, [tool]: true }
  try {
    await invoke('set_takeover_for_app', { app: tool, enabled: nextEnabled })
    await store.loadSettings()
    await store.getProxyStatus()
    await loadTakeoverStatuses()
  } catch {
    await loadTakeoverStatuses()
  } finally {
    takeoverLoading.value = { ...takeoverLoading.value, [tool]: false }
  }
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
  await loadTakeoverStatuses()
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

    <!-- API 来源管理子页面 -->
    <ApiSourceList
      v-show="subView === 'api-sources'"
      :onBack="goBack"
    />

    <!-- 窗口配额管理子页面 -->
    <WindowQuotaSettings
      v-show="subView === 'window-quotas'"
      @back="goBack"
    />

    <!-- 货币设置子页面 -->
    <CurrencySettings
      v-show="subView === 'currency'"
      @back="goBack"
    />

    <!-- 主设置页面 -->
    <div v-show="subView === 'main'" class="space-y-5 animate-in fade-in zoom-in-95 duration-300 pb-6">
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
              <option v-for="window in WINDOW_ORDER" :key="window" :value="window">
                {{ windowNameLabel(store.settings.locale, window) }}
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

          <!-- 工具接管 -->
          <div v-if="localDataSource === 'proxy'" class="p-3 px-4">
            <div class="mb-2 flex items-center justify-between">
              <div>
                <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.proxyTools') }}</div>
                <div class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.proxyToolsDesc') }}</div>
              </div>
              <span class="text-[10px] text-gray-400">
                {{ managedToolProfiles.filter(profile => takeoverEnabledFor(profile.tool)).length }}/{{ managedToolProfiles.length }}
              </span>
            </div>
            <div class="grid grid-cols-2 gap-2">
              <button
                v-for="profile in managedToolProfiles"
                :key="profile.id"
                :class="[
                  'min-w-0 rounded-xl border p-2.5 text-left transition-all',
                  takeoverEnabledFor(profile.tool)
                    ? 'border-green-200 bg-green-50/80 dark:border-green-500/30 dark:bg-green-500/10'
                    : 'border-gray-100 bg-gray-50/70 dark:border-neutral-800 dark:bg-neutral-900/60',
                  takeoverLoading[profile.tool] ? 'opacity-60 pointer-events-none' : 'hover:border-blue-200 dark:hover:border-blue-500/40'
                ]"
                @click="toggleToolTakeover(profile.tool)"
              >
                <div class="flex items-center justify-between gap-2">
                  <div class="flex min-w-0 items-center gap-2">
                    <div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-white shadow-[0_1px_4px_rgba(0,0,0,0.04)] dark:bg-neutral-800">
                      <LobeIcon
                        v-if="getToolIcon(profile.tool, profile.icon)"
                        :slug="getToolIcon(profile.tool, profile.icon)!"
                        :size="17"
                        @error="() => {}"
                      />
                      <span v-else class="h-2.5 w-2.5 rounded-full bg-gray-400"></span>
                    </div>
                    <span class="truncate text-[12px] font-medium text-gray-700 dark:text-gray-200">{{ profile.displayName || profile.tool }}</span>
                  </div>
                  <span
                    :class="[
                      'relative flex h-5 w-9 shrink-0 items-center rounded-full transition-colors',
                      takeoverEnabledFor(profile.tool) ? 'bg-green-500' : 'bg-gray-300 dark:bg-neutral-600'
                    ]"
                  >
                    <span
                      :class="[
                        'absolute h-[17px] w-[17px] rounded-full bg-white shadow shadow-black/10 transition-all',
                        takeoverEnabledFor(profile.tool) ? 'right-[2px]' : 'left-[2px]'
                      ]"
                    ></span>
                  </span>
                </div>
                <div class="mt-2 truncate text-[10px] text-gray-400">
                  {{ takeoverActiveFor(profile.tool) ? t(store.settings.locale, 'settings.configTakenOver') : t(store.settings.locale, 'settings.configNotTakenOver') }}
                  <template v-if="takeoverStatusFor(profile.tool)?.activeSourceId"> · {{ takeoverStatusFor(profile.tool)?.activeSourceId }}</template>
                </div>
                <div v-if="takeoverStatusFor(profile.tool)?.lastError" class="mt-1 truncate text-[10px] text-red-500">
                  {{ takeoverStatusFor(profile.tool)?.lastError }}
                </div>
              </button>
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
  
      <!-- 数据与配额 -->
      <div class="space-y-2">
        <h3 class="text-xs font-semibold text-gray-500 dark:text-gray-400 uppercase tracking-wider px-1">{{ t(store.settings.locale, 'settings.dataManagement') }}</h3>
        <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden divide-y divide-gray-50 dark:divide-neutral-800/50 shadow-sm">
  
          <!-- API 来源入口（仅代理模式显示） -->
          <div
            v-if="localDataSource === 'proxy'"
            @click="openApiSources"
            class="p-3 px-4 cursor-pointer hover:bg-gray-50 dark:hover:bg-neutral-800/50 transition-colors"
          >
            <div class="flex items-center justify-between">
              <div>
                <div class="flex items-center gap-2">
                  <div class="text-[13px] text-gray-700 dark:text-gray-200">
                    {{ t(store.settings.locale, 'sources.manage') }}
                  </div>
                  <span
                    v-if="store.settings.sourceAware.sources.filter(s => s.autoDetected && !s.displayName).length > 0"
                    class="px-1.5 py-0.5 text-[10px] font-medium bg-red-100 text-red-600 dark:bg-red-500/20 dark:text-red-400 rounded-full"
                  >
                    {{ store.settings.sourceAware.sources.filter(s => s.autoDetected && !s.displayName).length }}
                  </span>
                </div>
                <div class="text-[10px] text-gray-400 mt-0.5">
                  {{ store.settings.sourceAware.sources.length > 0
                    ? `${store.settings.sourceAware.sources.length} ${t(store.settings.locale, 'sources.sourcesCount')}`
                    : t(store.settings.locale, 'sources.noSources')
                  }}
                </div>
              </div>
              <svg class="w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </div>
          </div>
  
          <!-- 模型价格入口 -->
          <div
            @click="openModelPricing"
            class="p-3 px-4 cursor-pointer hover:bg-gray-50 dark:hover:bg-neutral-800/50 transition-colors"
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
  
          <!-- 货币设置入口 -->
          <div
            @click="openCurrency"
            class="p-3 px-4 cursor-pointer hover:bg-gray-50 dark:hover:bg-neutral-800/50 transition-colors"
          >
            <div class="flex items-center justify-between">
              <div>
                <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.currency') }}</div>
                <div class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.currencyDesc') }}</div>
              </div>
              <svg class="w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </div>
          </div>

          <!-- 计费类型 -->
          <div class="p-3 px-4">
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
  
          <!-- 窗口配额入口 -->
          <div
            @click="openWindowQuotas"
            class="p-3 px-4 cursor-pointer hover:bg-gray-50 dark:hover:bg-neutral-800/50 transition-colors"
          >
            <div class="flex items-center justify-between">
              <div>
                <div class="text-[13px] text-gray-700 dark:text-gray-200">{{ t(store.settings.locale, 'settings.manageQuotas') }}</div>
                <div class="text-[10px] text-gray-400 mt-0.5">{{ t(store.settings.locale, 'settings.manageQuotasDesc') }}</div>
              </div>
              <svg class="w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
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

    <!-- Codex OAuth 风险确认 -->
    <Teleport to="body">
      <div
        v-if="showCodexOauthWarning"
        class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
        @click.self="closeCodexOauthWarning(false)"
      >
        <div class="w-[320px] overflow-hidden rounded-2xl bg-white shadow-xl dark:bg-[#1C1C1E]" @click.stop>
          <div class="p-4">
            <div class="mx-auto mb-3 flex h-10 w-10 items-center justify-center rounded-full bg-amber-100 dark:bg-amber-500/20">
              <svg class="h-5 w-5 text-amber-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v4m0 4h.01M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z" />
              </svg>
            </div>
            <h3 class="mb-2 text-center text-sm font-semibold text-gray-900 dark:text-gray-100">
              {{ t(store.settings.locale, 'settings.codexOauthRiskTitle') }}
            </h3>
            <div class="space-y-2 text-xs leading-relaxed text-gray-500 dark:text-gray-400">
              <p>{{ t(store.settings.locale, 'settings.codexOauthRiskBody') }}</p>
              <p>{{ t(store.settings.locale, 'settings.codexOauthRiskAccount') }}</p>
              <p class="font-medium text-amber-600 dark:text-amber-400">{{ t(store.settings.locale, 'settings.codexOauthRiskAccept') }}</p>
            </div>
          </div>
          <div class="flex border-t border-gray-100 dark:border-neutral-800">
            <button
              @click="closeCodexOauthWarning(false)"
              class="flex-1 py-2.5 text-xs font-medium text-gray-600 transition-colors hover:bg-gray-50 dark:text-gray-400 dark:hover:bg-neutral-800"
            >
              {{ t(store.settings.locale, 'common.cancel') }}
            </button>
            <button
              @click="closeCodexOauthWarning(true)"
              class="flex-1 border-l border-gray-100 py-2.5 text-xs font-medium text-amber-600 transition-colors hover:bg-amber-50 dark:border-neutral-800 dark:text-amber-400 dark:hover:bg-amber-500/10"
            >
              {{ t(store.settings.locale, 'settings.codexOauthRiskContinue') }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>
