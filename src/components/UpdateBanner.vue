<script setup lang="ts">
import { computed } from 'vue'
import { ChevronDown, ChevronUp, Download, SkipForward, RefreshCw } from 'lucide-vue-next'
import { useUpdaterStore } from '../stores/updater'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()

const locale = computed(() => store.settings.locale)

const formattedDate = computed(() => {
  if (!updaterStore.updateInfo?.date) return ''
  try {
    const d = new Date(updaterStore.updateInfo.date)
    return d.toLocaleDateString(locale.value === 'en-US' ? 'en-US' : 'zh-CN', {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    })
  } catch {
    return updaterStore.updateInfo.date
  }
})

const isDownloading = computed(() => updaterStore.status === 'downloading')
const hasError = computed(() => updaterStore.status === 'error')

async function handleInstall() {
  await updaterStore.downloadAndInstall()
}

async function handleSkip() {
  await updaterStore.skipVersion()
}

async function handleRetry() {
  await updaterStore.checkForUpdate()
}
</script>

<template>
  <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-blue-100 dark:border-blue-900/50 overflow-hidden shadow-sm">
    <!-- 标题栏（始终显示） -->
    <div
      class="flex items-center gap-2.5 px-4 py-3 cursor-pointer select-none"
      @click="updaterStore.toggleExpanded()"
    >
      <div class="w-2 h-2 rounded-full bg-blue-500 shrink-0"></div>
      <span class="text-[13px] font-medium text-gray-800 dark:text-gray-100 flex-1 truncate">
        {{ t(locale, 'settings.update.available') }}
        <span class="ml-1.5 text-blue-500 font-semibold">v{{ updaterStore.updateInfo?.version }}</span>
      </span>
      <component
        :is="updaterStore.isExpanded ? ChevronUp : ChevronDown"
        class="w-4 h-4 text-gray-400 shrink-0"
      />
    </div>

    <!-- 展开内容 -->
    <div v-if="updaterStore.isExpanded" class="border-t border-gray-50 dark:border-neutral-800">

      <!-- 下载中状态 -->
      <div v-if="isDownloading" class="px-4 py-3 space-y-2">
        <p class="text-[12px] text-gray-600 dark:text-gray-400">
          {{ t(locale, 'settings.update.downloading', { version: updaterStore.updateInfo?.version ?? '' }) }}
        </p>
        <div class="w-full h-1.5 rounded-full bg-gray-100 dark:bg-neutral-700 overflow-hidden">
          <div
            class="h-full rounded-full bg-blue-500 transition-all duration-300"
            :style="{ width: `${updaterStore.downloadProgress}%` }"
          ></div>
        </div>
        <p class="text-[11px] text-gray-400 text-right">
          {{ updaterStore.formattedDownloaded }}
          <span v-if="updaterStore.formattedTotal"> / {{ updaterStore.formattedTotal }}</span>
          <span class="ml-1">({{ updaterStore.downloadProgress }}%)</span>
        </p>
      </div>

      <!-- 错误状态：提供重试入口 -->
      <div v-else-if="hasError" class="px-4 py-3 space-y-2">
        <p class="text-[12px] text-red-500">{{ t(locale, 'settings.update.downloadFailed') }}</p>
        <p v-if="updaterStore.errorMessage" class="text-[11px] text-gray-400 truncate">{{ updaterStore.errorMessage }}</p>
        <button
          @click="handleRetry"
          class="flex items-center gap-1.5 h-7 px-3 text-[11px] text-gray-600 dark:text-gray-400 rounded-lg border border-gray-200 dark:border-neutral-700 hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors"
        >
          <RefreshCw class="w-3 h-3" />
          {{ t(locale, 'settings.update.checkNow') }}
        </button>
      </div>

      <!-- 正常展开态 -->
      <template v-else>
        <!-- 发布日期 -->
        <div v-if="formattedDate" class="px-4 pt-2.5 pb-1">
          <span class="text-[11px] text-gray-400">
            {{ t(locale, 'settings.update.releasedAt', { date: formattedDate }) }}
          </span>
        </div>

        <!-- Release notes -->
        <div
          v-if="updaterStore.updateInfo?.body"
          class="mx-4 my-2 px-3 py-2.5 bg-gray-50 dark:bg-neutral-800/60 rounded-lg max-h-28 overflow-y-auto"
        >
          <pre class="text-[11px] text-gray-600 dark:text-gray-400 whitespace-pre-wrap font-sans leading-relaxed">{{ updaterStore.updateInfo.body }}</pre>
        </div>

        <!-- 操作按钮 -->
        <div class="flex gap-2 px-4 pb-3 pt-2">
          <button
            @click="handleInstall"
            class="flex-1 flex items-center justify-center gap-1.5 h-8 px-3 text-[12px] font-medium rounded-lg bg-blue-500 text-white hover:bg-blue-600 transition-colors"
          >
            <Download class="w-3.5 h-3.5" />
            {{ t(locale, 'settings.update.installNow') }}
          </button>
          <button
            @click="handleSkip"
            class="flex items-center justify-center gap-1.5 h-8 px-3 text-[12px] text-gray-500 dark:text-gray-400 rounded-lg border border-gray-200 dark:border-neutral-700 hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors"
          >
            <SkipForward class="w-3.5 h-3.5" />
            {{ t(locale, 'settings.update.skipVersion') }}
          </button>
        </div>
      </template>
    </div>
  </div>
</template>
