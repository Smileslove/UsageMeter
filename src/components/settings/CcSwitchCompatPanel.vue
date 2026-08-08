<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { resolveTakeoverConflict } from '../../api/proxyApi'
import { useMonitorStore } from '../../stores/monitor'
import { useCcSwitchCompatStore } from '../../stores/ccswitchCompat'
import { t } from '../../i18n'
import { formatToolDisplayName } from '../../utils/toolDisplay'
import { ShieldCheck, Eraser } from 'lucide-vue-next'

const store = useMonitorStore()
const compat = useCcSwitchCompatStore()

const locale = computed(() => store.settings.locale)

function errorKeyForCode(code: string): string {
  if (code.startsWith('ccswitchRunning')) return 'settings.ccswitch.errorRunning'
  if (code.startsWith('ccswitchNotInstalled')) return 'settings.ccswitch.errorNotInstalled'
  if (code.startsWith('ccswitchSchemaMismatch')) return 'settings.ccswitch.errorSchemaMismatch'
  if (code.startsWith('ccswitchProxyActive')) return 'settings.ccswitch.errorProxyActive'
  return 'settings.ccswitch.errorGeneric'
}

const cleanErrorKey = computed(() => {
  const code = compat.cleanErrorCode
  return code ? errorKeyForCode(code) : null
})

/** 后端自动清理留下的持久化错误（如 schema 不符），与手动清理错误去重显示 */
const autoCleanErrorKey = computed(() => {
  const code = compat.lastErrorCode
  if (!code) return null
  const key = errorKeyForCode(code)
  return key === cleanErrorKey.value ? null : key
})

const reclaimErrorCode = ref<string | null>(null)
const reclaimErrorKey = computed(() => {
  const code = reclaimErrorCode.value
  if (!code) return null
  if (code.startsWith('ccswitchProxyActive')) return 'settings.ccswitch.errorProxyActive'
  return 'settings.ccswitch.errorGeneric'
})

const lastCleanText = computed(() => {
  if (!compat.lastCleanAtMs) {
    return t(locale.value, 'settings.ccswitch.neverCleaned')
  }
  const time = new Date(compat.lastCleanAtMs).toLocaleString(locale.value)
  return t(locale.value, 'settings.ccswitch.lastCleanAt', { time })
})

const lastReportText = computed(() => {
  const report = compat.lastReport
  if (!report) return null
  if (report.cleaned === 0 && report.unresolved === 0) {
    return t(locale.value, 'settings.ccswitch.cleanResultClean')
  }
  return t(locale.value, 'settings.ccswitch.cleanResult', {
    cleaned: report.cleaned,
    unresolved: report.unresolved
  })
})

async function forceReclaim(tool: string) {
  reclaimErrorCode.value = null
  try {
    await resolveTakeoverConflict(tool, 'force_reclaim')
  } catch (e) {
    reclaimErrorCode.value = String(e)
    console.error('Failed to force reclaim takeover:', e)
  }
  await compat.refresh()
}

let unlistenDetected: UnlistenFn | null = null
let unlistenReleased: UnlistenFn | null = null
let unlistenCleaned: UnlistenFn | null = null
let refreshTimer: ReturnType<typeof setInterval> | null = null

onMounted(async () => {
  await compat.refresh()
  unlistenDetected = await listen('external_manager_detected', () => compat.refresh())
  unlistenReleased = await listen('external_manager_released', () => compat.refresh())
  unlistenCleaned = await listen('ccswitch_db_cleaned', () => compat.refresh())
  refreshTimer = setInterval(() => compat.refresh(), 30000)
})

onUnmounted(() => {
  if (unlistenDetected) unlistenDetected()
  if (unlistenReleased) unlistenReleased()
  if (unlistenCleaned) unlistenCleaned()
  if (refreshTimer) clearInterval(refreshTimer)
})
</script>

<template>
  <div
    v-if="compat.installed"
    class="overflow-hidden rounded-xl border border-gray-100 bg-white shadow-sm dark:border-neutral-800 dark:bg-[#1C1C1E]"
  >
    <div class="p-[16px] space-y-3">
      <!-- 标题与状态徽标 -->
      <div class="flex items-center justify-between gap-2">
        <div class="flex items-center gap-2 min-w-0">
          <ShieldCheck class="h-4 w-4 shrink-0 text-emerald-500" />
          <span class="text-[13px] font-semibold text-gray-900 dark:text-gray-100 truncate">
            {{ t(locale, 'settings.ccswitch.title') }}
          </span>
        </div>
        <span
          class="shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium"
          :class="compat.running
            ? 'bg-amber-50 text-amber-600 dark:bg-amber-500/10 dark:text-amber-400'
            : 'bg-gray-50 text-gray-500 dark:bg-neutral-800 dark:text-gray-400'"
        >
          {{ t(locale, compat.running ? 'settings.ccswitch.running' : 'settings.ccswitch.notRunning') }}
        </span>
      </div>

      <p class="text-[11px] leading-snug text-gray-400 dark:text-gray-500">
        {{ t(locale, 'settings.ccswitch.desc') }}
      </p>

      <!-- 礼让状态 -->
      <div
        v-if="compat.yieldedTools.length > 0"
        class="rounded-lg bg-amber-50/70 px-2.5 py-2 space-y-1.5 dark:bg-amber-500/10"
      >
        <p class="text-[11px] font-semibold text-amber-700 dark:text-amber-400">
          {{ t(locale, 'settings.ccswitch.yieldedTitle') }}
        </p>
        <div
          v-for="tool in compat.yieldedTools"
          :key="tool"
          class="flex items-center justify-between gap-2"
        >
          <p class="min-w-0 flex-1 text-[10.5px] leading-snug text-amber-700/80 dark:text-amber-400/80">
            {{ t(locale, 'settings.ccswitch.yieldedDesc', { tool: formatToolDisplayName(tool, locale, store.settings.clientTools.profiles) }) }}
          </p>
          <button
            class="shrink-0 rounded-lg bg-amber-600/15 px-2 py-1 text-[10px] font-semibold text-amber-700 transition-colors hover:bg-amber-600/25 dark:text-amber-400"
            @click="forceReclaim(tool)"
          >
            {{ t(locale, 'settings.ccswitch.forceReclaim') }}
          </button>
        </div>
        <p v-if="reclaimErrorKey" class="text-[10.5px] leading-snug text-red-500">
          {{ t(locale, reclaimErrorKey) }}
        </p>
      </div>

      <!-- 清理状态与操作 -->
      <div class="flex items-center justify-between gap-2">
        <div class="min-w-0 flex-1 space-y-0.5">
          <p class="text-[11px] text-gray-500 dark:text-gray-400 truncate">{{ lastCleanText }}</p>
          <p v-if="lastReportText" class="text-[10.5px] text-gray-400 dark:text-gray-500 truncate">
            {{ lastReportText }}
          </p>
          <p v-if="compat.pendingClean" class="text-[10.5px] text-amber-500 truncate">
            {{ t(locale, 'settings.ccswitch.pendingClean') }}
          </p>
          <p v-if="cleanErrorKey" class="text-[10.5px] text-red-500 truncate">
            {{ t(locale, cleanErrorKey) }}
          </p>
          <p v-if="autoCleanErrorKey" class="text-[10.5px] text-red-500 truncate">
            {{ t(locale, autoCleanErrorKey) }}
          </p>
        </div>
        <button
          class="flex shrink-0 items-center gap-1 rounded-lg px-2.5 py-1.5 text-[11px] font-semibold transition-colors"
          :class="compat.running || compat.cleaning
            ? 'cursor-not-allowed bg-gray-50 text-gray-300 dark:bg-neutral-800 dark:text-gray-600'
            : 'bg-emerald-50 text-emerald-600 hover:bg-emerald-100 dark:bg-emerald-500/10 dark:text-emerald-400 dark:hover:bg-emerald-500/20'"
          :disabled="compat.running || compat.cleaning"
          :title="compat.running ? t(locale, 'settings.ccswitch.cleanDisabledRunning') : undefined"
          @click="compat.runClean()"
        >
          <Eraser class="h-3 w-3" />
          {{ t(locale, compat.cleaning ? 'settings.ccswitch.cleaning' : 'settings.ccswitch.cleanNow') }}
        </button>
      </div>

      <p
        v-if="compat.lastReport?.backupPath"
        class="text-[10px] leading-snug text-gray-300 dark:text-gray-600 break-all"
      >
        {{ t(locale, 'settings.ccswitch.backupHint', { path: compat.lastReport.backupPath }) }}
      </p>
    </div>
  </div>
</template>
