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
        class="fixed inset-0 z-[70] flex items-center justify-center px-4 backdrop-blur-[3px]"
        style="background: color-mix(in srgb, var(--theme-text-primary) 22%, transparent);"
        @click.self="updaterStore.closeDialog()"
      >
        <div class="w-full max-w-[368px] overflow-hidden rounded-[26px] border theme-border theme-surface-elevated" style="box-shadow: var(--theme-shadow-card);">
          <div class="relative overflow-hidden px-5 pb-4 pt-4">
            <div
              class="pointer-events-none absolute inset-x-0 top-0 h-24"
              style="background: radial-gradient(circle at top, color-mix(in srgb, var(--theme-accent-soft) 90%, transparent) 0%, transparent 68%);"
            ></div>
            <button
              class="absolute right-4 top-4 flex h-7 w-7 items-center justify-center rounded-full theme-text-tertiary transition-colors hover:bg-gray-50 hover:text-gray-800"
              :title="t(locale, 'common.cancel')"
              @click="updaterStore.closeDialog()"
            >
              <X class="h-4 w-4" />
            </button>

            <div class="relative pr-9">
              <div class="theme-status-info mb-3 inline-flex items-center gap-2 rounded-full border px-2.5 py-1 text-[11px] font-medium">
                <span class="h-1.5 w-1.5 rounded-full bg-[var(--theme-accent-primary)]"></span>
                {{ t(locale, 'settings.update.available') }}
              </div>
              <h3 class="text-[17px] font-semibold tracking-tight text-[var(--theme-text-primary)]">
                UsageMeter v{{ updaterStore.updateInfo.version }}
              </h3>
              <p class="mt-1 text-[12px] text-[var(--theme-text-secondary)]">
                {{ t(locale, 'settings.update.currentVersion') }} v{{ updaterStore.updateInfo.currentVersion }}
              </p>
              <p v-if="formattedDate" class="mt-1 text-[11px] text-[var(--theme-text-tertiary)]">
                {{ t(locale, 'settings.update.releasedAt', { date: formattedDate }) }}
              </p>
            </div>

            <div v-if="isDownloading" class="theme-surface-muted relative mt-4 rounded-2xl border p-3.5">
              <p class="text-[12px] font-medium text-[var(--theme-text-primary)]">
                {{ t(locale, 'settings.update.downloading', { version: updaterStore.updateInfo.version }) }}
              </p>
              <div class="mt-3 h-2 overflow-hidden rounded-full" :style="{ backgroundColor: 'var(--theme-border-default)' }">
                <div
                  class="h-full rounded-full transition-all duration-300"
                  style="background: var(--theme-accent-primary);"
                  :style="{ width: `${updaterStore.downloadProgress}%` }"
                ></div>
              </div>
              <p class="mt-2 text-right text-[11px] text-[var(--theme-text-tertiary)]">
                {{ updaterStore.formattedDownloaded }}
                <span v-if="updaterStore.formattedTotal"> / {{ updaterStore.formattedTotal }}</span>
                <span class="ml-1">({{ updaterStore.downloadProgress }}%)</span>
              </p>
            </div>

            <div v-else-if="hasError" class="theme-status-danger relative mt-4 rounded-2xl border p-3.5">
              <p class="text-[12px] font-medium">
                {{ t(locale, updaterStore.errorMessage === 'downloadFailed' ? 'settings.update.downloadFailed' : 'settings.update.checkFailed') }}
              </p>
              <button
                class="theme-status-danger mt-3 inline-flex h-8 items-center justify-center gap-1.5 rounded-xl border px-3 text-[11px] font-medium transition-colors"
                @click="handleRetry"
              >
                <RefreshCw class="h-3.5 w-3.5" />
                {{ t(locale, 'settings.update.checkNow') }}
              </button>
            </div>

            <div
              v-else-if="updaterStore.updateInfo.body"
              class="theme-surface-muted relative mt-4 rounded-2xl border p-3.5"
            >
              <p class="mb-2 text-[11px] font-medium uppercase tracking-[0.14em] text-[var(--theme-text-tertiary)]">
                {{ t(locale, 'settings.update.releaseNotes') }}
              </p>
              <pre class="max-h-36 overflow-y-auto whitespace-pre-wrap font-sans text-[11px] leading-relaxed text-[var(--theme-text-secondary)]">{{ updaterStore.updateInfo.body }}</pre>
            </div>

            <div v-if="!isDownloading" class="relative mt-4 flex gap-2">
              <button
                class="theme-surface flex-1 rounded-2xl border px-3 py-2.5 text-[12px] font-medium text-[var(--theme-text-secondary)] transition-colors hover:bg-gray-50"
                @click="updaterStore.closeDialog()"
              >
                {{ t(locale, 'settings.update.remindLater') }}
              </button>
              <button
                class="theme-surface flex items-center justify-center gap-1.5 rounded-2xl border px-3 py-2.5 text-[12px] font-medium text-[var(--theme-text-secondary)] transition-colors hover:bg-gray-50"
                @click="handleSkip"
              >
                <SkipForward class="h-3.5 w-3.5" />
                {{ t(locale, 'settings.update.skipVersion') }}
              </button>
              <button
                class="theme-button-accent flex-1 items-center justify-center gap-1.5 rounded-2xl px-3 py-2.5 text-[12px] font-semibold transition-colors"
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
