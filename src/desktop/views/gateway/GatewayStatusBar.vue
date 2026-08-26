<script setup lang="ts">
/**
 * 桌面网关页「状态带」（设计文档 10.2）：
 * 监听状态 / 监听地址 / 活跃连接 / 近期请求 / 错误率 + 右侧唯一主操作「新增上游」。
 * 纯展示组件：状态由父组件计算后传入，主操作通过 create 事件上抛。
 */
import { computed } from 'vue'
import { Activity, AlertTriangle, Plus, RadioTower, RefreshCw } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t } from '../../../i18n'

defineProps<{
  listenerStatus: boolean
  listenerAddress: string
  activeConnections: number
  recentRequests: number
  errorRate: string
}>()

const emit = defineEmits<{
  (e: 'create'): void
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)
</script>

<template>
  <div class="theme-surface flex flex-wrap items-center gap-x-6 gap-y-2 rounded-xl border px-4 py-3">
    <div class="flex items-center gap-2">
      <span
        class="inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-semibold leading-none"
        :class="listenerStatus ? 'theme-status-success' : 'theme-status-danger'"
      >
        <RadioTower class="h-3 w-3" aria-hidden="true" />
        {{ listenerStatus ? t(locale, 'gateway.running') : t(locale, 'gateway.stopped') }}
      </span>
    </div>
    <div class="min-w-0">
      <div class="text-xs uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.gateway.address') }}</div>
      <div class="truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="listenerAddress">{{ listenerAddress }}</div>
    </div>
    <div class="flex items-center gap-1.5">
      <Activity class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <span class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.gateway.activeConnections') }}: <b class="font-mono">{{ activeConnections }}</b></span>
    </div>
    <div class="flex items-center gap-1.5">
      <RefreshCw class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <span class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.gateway.recentRequests') }}: <b class="font-mono">{{ recentRequests }}</b></span>
    </div>
    <div class="flex items-center gap-1.5">
      <AlertTriangle class="h-3.5 w-3.5 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <span class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.gateway.errorRate') }}: <b class="font-mono" :class="{ 'text-red-500': errorRate !== '—' && parseFloat(errorRate) > 5 }">{{ errorRate }}</b></span>
    </div>
    <!-- 右侧唯一主操作 -->
    <button
      type="button"
      class="theme-button-accent ml-auto inline-flex h-8 items-center gap-1.5 rounded-lg px-3.5 text-xs font-semibold"
      @click="emit('create')"
    >
      <Plus class="h-4 w-4" aria-hidden="true" />
      {{ t(locale, 'gateway.newProfile') }}
    </button>
  </div>
</template>
