<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t, windowNameLabel } from '../i18n'
import { WINDOW_ORDER, type WindowName } from '../types'

const emit = defineEmits<{
  back: []
}>()

const store = useMonitorStore()

// 本地配额副本
const localQuotas = ref(JSON.parse(JSON.stringify(store.settings.quotas)))

// 监听 store 同步
watch(() => store.settings.quotas, (val) => {
  localQuotas.value = JSON.parse(JSON.stringify(val))
}, { deep: true })

// 是否显示 Token 限额
const showTokenLimit = computed(() => {
  return store.settings.billingType === 'token' || store.settings.billingType === 'both'
})

// 是否显示请求限额
const showRequestLimit = computed(() => {
  return store.settings.billingType === 'request' || store.settings.billingType === 'both'
})

const getQuota = (window: WindowName) => {
  return localQuotas.value.find((q: any) => q.window === window)
}

const toggleWindowEnabled = async (window: WindowName) => {
  const quota = getQuota(window)
  if (quota) {
    quota.enabled = !quota.enabled
    store.settings.quotas = JSON.parse(JSON.stringify(localQuotas.value))
    await store.saveSettings()
  }
}

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

const formatNumber = (num: number | null): string => {
  if (num === null) return ''
  return num.toLocaleString()
}

const enabledCount = computed(() => {
  return localQuotas.value.filter((q: any) => q.enabled).length
})
</script>

<template>
  <div class="flex flex-col gap-3 animate-in fade-in zoom-in-95 duration-300">
    <!-- 头部 -->
    <div class="flex items-center justify-between px-1">
      <button @click="emit('back')" class="flex items-center gap-1 text-blue-500 text-[13px] hover:text-blue-600 transition-colors">
        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
        </svg>
        {{ t(store.settings.locale, 'common.dashboard') }}
      </button>
      <h2 class="text-sm font-semibold text-gray-800 dark:text-gray-100">
        {{ t(store.settings.locale, 'settings.quotaTitle') }}
      </h2>
      <span class="text-[11px] text-gray-400">
        {{ enabledCount }} / {{ WINDOW_ORDER.length }} {{ t(store.settings.locale, 'common.enabled') }}
      </span>
    </div>

    <!-- 窗口配额列表 -->
    <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden shadow-sm">
      <!-- 列标题 -->
      <div class="flex items-center gap-2 px-3 py-1.5 border-b border-gray-100 dark:border-neutral-700">
        <span class="w-14" />
        <span v-if="showTokenLimit" class="flex-1 text-[10px] font-medium text-gray-400 dark:text-gray-500 text-right pr-0.5">{{ t(store.settings.locale, 'common.token') }}</span>
        <span v-if="showRequestLimit" class="flex-1 text-[10px] font-medium text-gray-400 dark:text-gray-500 text-right pr-0.5">{{ t(store.settings.locale, 'common.requests') }}</span>
        <span class="w-9" />
      </div>
      <div
        v-for="(window, idx) in WINDOW_ORDER"
        :key="window"
        :class="[
          'flex items-center gap-2 px-3 transition-colors',
          getQuota(window)?.enabled ? 'bg-white dark:bg-[#1C1C1E]' : 'bg-gray-50/50 dark:bg-neutral-900/30',
          idx !== WINDOW_ORDER.length - 1 ? 'border-b border-gray-50 dark:border-neutral-800/50' : ''
        ]"
      >
        <!-- 主行：窗口名称 + 可选输入 + 开关 -->
        <div class="flex-1 flex items-center gap-2 py-2 min-w-0">
          <!-- 窗口名称 -->
          <span
            :class="[
              'text-[12px] font-medium shrink-0 transition-colors w-14',
              getQuota(window)?.enabled ? 'text-gray-700 dark:text-gray-200' : 'text-gray-400 dark:text-gray-500'
            ]"
          >
            {{ windowNameLabel(store.settings.locale, window) }}
          </span>

          <!-- 限额输入（仅在启用时显示） -->
          <template v-if="getQuota(window)?.enabled">
            <div v-if="showTokenLimit" class="min-w-0 flex-1">
              <input
                type="text"
                :value="formatNumber(getQuota(window)?.tokenLimit)"
                @blur="(e) => updateTokenLimit(window, (e.target as HTMLInputElement).value)"
                @keyup.enter="(e) => updateTokenLimit(window, (e.target as HTMLInputElement).value)"
                :placeholder="t(store.settings.locale, 'settings.unlimited')"
                :aria-label="`${windowNameLabel(store.settings.locale, window)} ${t(store.settings.locale, 'common.token')}`"
                class="w-full bg-gray-50 dark:bg-neutral-800 text-gray-600 dark:text-gray-300 text-[11px] font-mono outline-none text-right px-1.5 py-1 rounded border border-gray-200 dark:border-neutral-700 focus:border-blue-400 focus:ring-1 focus:ring-blue-400"
              />
            </div>
            <div v-if="showRequestLimit" class="min-w-0 flex-1">
              <input
                type="text"
                :value="formatNumber(getQuota(window)?.requestLimit)"
                @blur="(e) => updateRequestLimit(window, (e.target as HTMLInputElement).value)"
                @keyup.enter="(e) => updateRequestLimit(window, (e.target as HTMLInputElement).value)"
                :placeholder="t(store.settings.locale, 'settings.unlimited')"
                :aria-label="`${windowNameLabel(store.settings.locale, window)} ${t(store.settings.locale, 'common.requests')}`"
                class="w-full bg-gray-50 dark:bg-neutral-800 text-gray-600 dark:text-gray-300 text-[11px] font-mono outline-none text-right px-1.5 py-1 rounded border border-gray-200 dark:border-neutral-700 focus:border-blue-400 focus:ring-1 focus:ring-blue-400"
              />
            </div>
          </template>
          <!-- 未启用时占位 -->
          <div v-else class="flex-1" />
        </div>

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
    </div>
  </div>
</template>
