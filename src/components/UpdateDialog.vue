<script setup lang="ts">
import { computed } from 'vue'
import { Download, RefreshCw, SkipForward, X } from 'lucide-vue-next'
import { useUpdaterStore } from '../stores/updater'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()

const locale = computed(() => store.settings.locale)
const isDownloading = computed(() => updaterStore.status === 'downloading')
const hasError = computed(() => updaterStore.status === 'error')

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

async function handleInstall() {
  await updaterStore.downloadAndInstall()
}

async function handleRetry() {
  await updaterStore.checkForUpdate()
}

async function handleSkip() {
  await updaterStore.skipVersion()
}
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition duration-180 ease-out"
      enter-from-class="opacity-0"
      enter-to-class="opacity-100"
      leave-active-class="transition duration-150 ease-in"
      leave-from-class="opacity-100"
      leave-to-class="opacity-0"
    >
      <div
        v-if="updaterStore.isDialogOpen && updaterStore.updateInfo"
        class="fixed inset-0 z-[70] flex items-center justify-center bg-slate-950/28 px-4 backdrop-blur-[3px]"
        @click.self="updaterStore.closeDialog()"
      >
        <div class="w-full max-w-[368px] overflow-hidden rounded-[26px] border border-white/75 bg-white/96 shadow-[0_24px_80px_rgba(15,23,42,0.22)] ring-1 ring-slate-200/70 dark:border-white/12 dark:bg-[#15171C]/96 dark:ring-white/8">
          <div class="relative overflow-hidden px-5 pb-4 pt-4">
            <div class="pointer-events-none absolute inset-x-0 top-0 h-24 bg-[radial-gradient(circle_at_top,rgba(59,130,246,0.14),transparent_68%)] dark:bg-[radial-gradient(circle_at_top,rgba(59,130,246,0.18),transparent_68%)]"></div>
            <button
              class="absolute right-4 top-4 flex h-7 w-7 items-center justify-center rounded-full text-slate-400 transition-colors hover:bg-slate-100 hover:text-slate-600 dark:text-slate-500 dark:hover:bg-white/8 dark:hover:text-slate-200"
              :title="t(locale, 'common.cancel')"
              @click="updaterStore.closeDialog()"
            >
              <X class="h-4 w-4" />
            </button>

            <div class="relative pr-9">
              <div class="mb-3 inline-flex items-center gap-2 rounded-full border border-blue-100 bg-blue-50 px-2.5 py-1 text-[11px] font-medium text-blue-600 dark:border-blue-400/18 dark:bg-blue-400/10 dark:text-blue-300">
                <span class="h-1.5 w-1.5 rounded-full bg-blue-500"></span>
                {{ t(locale, 'settings.update.available') }}
              </div>
              <h3 class="text-[17px] font-semibold tracking-tight text-slate-900 dark:text-slate-50">
                UsageMeter v{{ updaterStore.updateInfo.version }}
              </h3>
              <p class="mt-1 text-[12px] text-slate-500 dark:text-slate-400">
                {{ t(locale, 'settings.update.currentVersion') }} v{{ updaterStore.updateInfo.currentVersion }}
              </p>
              <p v-if="formattedDate" class="mt-1 text-[11px] text-slate-400 dark:text-slate-500">
                {{ t(locale, 'settings.update.releasedAt', { date: formattedDate }) }}
              </p>
            </div>

            <div v-if="isDownloading" class="relative mt-4 rounded-2xl border border-slate-100 bg-slate-50/85 p-3.5 dark:border-white/8 dark:bg-white/[0.04]">
              <p class="text-[12px] font-medium text-slate-700 dark:text-slate-200">
                {{ t(locale, 'settings.update.downloading', { version: updaterStore.updateInfo.version }) }}
              </p>
              <div class="mt-3 h-2 overflow-hidden rounded-full bg-slate-200/80 dark:bg-white/10">
                <div
                  class="h-full rounded-full bg-blue-500 transition-all duration-300"
                  :style="{ width: `${updaterStore.downloadProgress}%` }"
                ></div>
              </div>
              <p class="mt-2 text-right text-[11px] text-slate-400 dark:text-slate-500">
                {{ updaterStore.formattedDownloaded }}
                <span v-if="updaterStore.formattedTotal"> / {{ updaterStore.formattedTotal }}</span>
                <span class="ml-1">({{ updaterStore.downloadProgress }}%)</span>
              </p>
            </div>

            <div v-else-if="hasError" class="relative mt-4 rounded-2xl border border-red-100 bg-red-50/90 p-3.5 dark:border-red-400/18 dark:bg-red-400/10">
              <p class="text-[12px] font-medium text-red-600 dark:text-red-300">
                {{ t(locale, updaterStore.errorMessage === 'downloadFailed' ? 'settings.update.downloadFailed' : 'settings.update.checkFailed') }}
              </p>
              <button
                class="mt-3 inline-flex h-8 items-center justify-center gap-1.5 rounded-xl border border-red-200 bg-white px-3 text-[11px] font-medium text-red-500 transition-colors hover:bg-red-50 dark:border-red-400/20 dark:bg-transparent dark:text-red-300 dark:hover:bg-red-400/10"
                @click="handleRetry"
              >
                <RefreshCw class="h-3.5 w-3.5" />
                {{ t(locale, 'settings.update.checkNow') }}
              </button>
            </div>

            <div
              v-else-if="updaterStore.updateInfo.body"
              class="relative mt-4 rounded-2xl border border-slate-100 bg-slate-50/85 p-3.5 dark:border-white/8 dark:bg-white/[0.04]"
            >
              <p class="mb-2 text-[11px] font-medium uppercase tracking-[0.14em] text-slate-400 dark:text-slate-500">
                {{ t(locale, 'settings.update.releaseNotes') }}
              </p>
              <pre class="max-h-36 overflow-y-auto whitespace-pre-wrap font-sans text-[11px] leading-relaxed text-slate-600 dark:text-slate-300">{{ updaterStore.updateInfo.body }}</pre>
            </div>

            <div v-if="!isDownloading" class="relative mt-4 flex gap-2">
              <button
                class="flex-1 rounded-2xl border border-slate-200 bg-white px-3 py-2.5 text-[12px] font-medium text-slate-500 transition-colors hover:bg-slate-50 dark:border-white/10 dark:bg-white/[0.03] dark:text-slate-300 dark:hover:bg-white/[0.06]"
                @click="updaterStore.closeDialog()"
              >
                {{ t(locale, 'settings.update.remindLater') }}
              </button>
              <button
                class="flex items-center justify-center gap-1.5 rounded-2xl border border-slate-200 bg-white px-3 py-2.5 text-[12px] font-medium text-slate-500 transition-colors hover:bg-slate-50 dark:border-white/10 dark:bg-white/[0.03] dark:text-slate-300 dark:hover:bg-white/[0.06]"
                @click="handleSkip"
              >
                <SkipForward class="h-3.5 w-3.5" />
                {{ t(locale, 'settings.update.skipVersion') }}
              </button>
              <button
                class="flex-1 items-center justify-center gap-1.5 rounded-2xl bg-blue-500 px-3 py-2.5 text-[12px] font-semibold text-white transition-colors hover:bg-blue-600"
                @click="handleInstall"
              >
                <span class="inline-flex items-center gap-1.5">
                  <Download class="h-3.5 w-3.5" />
                  {{ t(locale, 'settings.update.installNow') }}
                </span>
              </button>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
