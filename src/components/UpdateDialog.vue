<script setup lang="ts">
import { computed } from 'vue'
import { CalendarDays, Download, RefreshCw, SkipForward, Sparkles, X } from 'lucide-vue-next'
import { useUpdaterStore } from '../stores/updater'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()

const locale = computed(() => store.settings.locale)
const isDownloading = computed(() => updaterStore.status === 'downloading')
const hasError = computed(() => updaterStore.status === 'error')

const formattedDate = computed(() => {
  const rawDate = updaterStore.updateInfo?.date?.trim()
  if (!rawDate) return ''

  const parsedDate = new Date(rawDate.length === 10 ? `${rawDate}T00:00:00` : rawDate)
  if (Number.isNaN(parsedDate.getTime())) {
    return ''
  }

  return parsedDate.toLocaleDateString(locale.value === 'en-US' ? 'en-US' : 'zh-CN', {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  })
})

const releaseNotesHtml = computed(() => renderReleaseNotes(updaterStore.updateInfo?.body ?? ''))

async function handleInstall() {
  await updaterStore.downloadAndInstall()
}

async function handleRetry() {
  await updaterStore.checkForUpdate()
}

async function handleSkip() {
  await updaterStore.skipVersion()
}

function renderReleaseNotes(markdown: string): string {
  const normalized = markdown.replace(/\r\n/g, '\n').trim()
  if (!normalized) return ''

  const blocks: string[] = []
  const paragraphLines: string[] = []
  const listItems: string[] = []

  const flushParagraph = () => {
    if (!paragraphLines.length) return
    blocks.push(`<p>${renderInlineMarkdown(paragraphLines.join(' '))}</p>`)
    paragraphLines.length = 0
  }

  const flushList = () => {
    if (!listItems.length) return
    blocks.push(`<ul>${listItems.map((item) => `<li>${renderInlineMarkdown(item)}</li>`).join('')}</ul>`)
    listItems.length = 0
  }

  for (const line of normalized.split('\n')) {
    const trimmed = line.trim()

    if (!trimmed) {
      flushParagraph()
      flushList()
      continue
    }

    if (/^-{3,}$/.test(trimmed)) {
      flushParagraph()
      flushList()
      blocks.push('<hr />')
      continue
    }

    const headingMatch = /^(#{1,6})\s+(.*)$/.exec(trimmed)
    if (headingMatch) {
      flushParagraph()
      flushList()
      const level = Math.min(headingMatch[1].length, 4)
      blocks.push(`<h${level}>${renderInlineMarkdown(headingMatch[2])}</h${level}>`)
      continue
    }

    const listMatch = /^[-*]\s+(.*)$/.exec(trimmed)
    if (listMatch) {
      flushParagraph()
      listItems.push(listMatch[1])
      continue
    }

    paragraphLines.push(trimmed)
  }

  flushParagraph()
  flushList()

  return blocks.join('')
}

function renderInlineMarkdown(text: string): string {
  return escapeHtml(text)
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>')
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
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
        <div class="theme-border theme-surface-elevated h-[min(88vh,596px)] w-full max-w-[392px] overflow-hidden rounded-[28px] border" style="box-shadow: var(--theme-shadow-card);">
          <div class="relative flex h-full flex-col overflow-hidden px-5 pb-4 pt-4">
            <div
              class="pointer-events-none absolute inset-x-0 top-0 h-28"
              style="background: radial-gradient(circle at top, color-mix(in srgb, var(--theme-accent-soft) 78%, transparent) 0%, transparent 72%);"
            ></div>
            <button
              class="absolute right-4 top-4 flex h-8 w-8 items-center justify-center rounded-full theme-text-tertiary transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
              :title="t(locale, 'common.cancel')"
              @click="updaterStore.closeDialog()"
            >
              <X class="h-4 w-4" />
            </button>

            <div class="relative shrink-0 pr-10">
              <div class="theme-status-info mb-3 inline-flex items-center gap-2 rounded-full border px-3 py-1 text-[11px] font-medium">
                <Sparkles class="h-3.5 w-3.5" />
                {{ t(locale, 'settings.update.available') }}
              </div>

              <div class="space-y-2">
                <h3 class="text-[21px] font-semibold tracking-[-0.03em] text-[var(--theme-text-primary)]">
                  UsageMeter v{{ updaterStore.updateInfo.version }}
                </h3>
                <div class="flex flex-wrap gap-2">
                  <div class="theme-surface-muted inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] text-[var(--theme-text-secondary)]">
                    {{ t(locale, 'settings.update.currentVersion') }} v{{ updaterStore.updateInfo.currentVersion }}
                  </div>
                  <div
                    v-if="formattedDate"
                    class="theme-surface-muted inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] text-[var(--theme-text-secondary)]"
                  >
                    <CalendarDays class="h-3.5 w-3.5 text-[var(--theme-text-tertiary)]" />
                    {{ t(locale, 'settings.update.releasedAt', { date: formattedDate }) }}
                  </div>
                </div>
              </div>

              <p class="mt-3 text-[12px] leading-relaxed text-[var(--theme-text-secondary)]">
                {{ t(locale, 'settings.update.releaseNotes') }}
              </p>
            </div>

            <div v-if="isDownloading" class="theme-surface-muted relative mt-4 flex shrink-0 flex-col rounded-[24px] border p-4">
              <p class="text-[13px] font-semibold text-[var(--theme-text-primary)]">
                {{ t(locale, 'settings.update.downloading', { version: updaterStore.updateInfo.version }) }}
              </p>
              <p class="mt-1 text-[11px] leading-relaxed text-[var(--theme-text-secondary)]">
                {{ updaterStore.formattedDownloaded }}
                <span v-if="updaterStore.formattedTotal"> / {{ updaterStore.formattedTotal }}</span>
                <span class="ml-1">({{ updaterStore.downloadProgress }}%)</span>
              </p>
              <div class="mt-4 h-2 overflow-hidden rounded-full" :style="{ backgroundColor: 'var(--theme-border-default)' }">
                <div
                  class="h-full rounded-full transition-all duration-300"
                  style="background: var(--theme-accent-primary);"
                  :style="{ width: `${updaterStore.downloadProgress}%` }"
                ></div>
              </div>
            </div>

            <div v-else-if="hasError" class="theme-status-danger relative mt-4 flex shrink-0 flex-col rounded-[24px] border p-4">
              <p class="text-[13px] font-semibold">
                {{ t(locale, updaterStore.errorMessage === 'downloadFailed' ? 'settings.update.downloadFailed' : 'settings.update.checkFailed') }}
              </p>
              <button
                class="theme-status-danger mt-3 inline-flex h-9 items-center justify-center gap-1.5 self-start rounded-xl border px-3 text-[11px] font-medium transition-colors"
                @click="handleRetry"
              >
                <RefreshCw class="h-3.5 w-3.5" />
                {{ t(locale, 'settings.update.checkNow') }}
              </button>
            </div>

            <div
              v-else-if="releaseNotesHtml"
              class="theme-surface-muted update-notes-card relative mt-4 flex min-h-0 flex-1 flex-col overflow-hidden rounded-[24px] border"
            >
              <div class="shrink-0 border-b px-4 py-3" style="border-color: color-mix(in srgb, var(--theme-border-default) 72%, transparent);">
                <p class="text-[12px] font-semibold tracking-[0.02em] text-[var(--theme-text-primary)]">
                  {{ t(locale, 'settings.update.releaseNotes') }}
                </p>
              </div>
              <div class="release-notes-scroll min-h-0 flex-1 overflow-y-auto px-4 pb-4 pt-3">
                <div class="update-release-notes" v-html="releaseNotesHtml"></div>
              </div>
            </div>

            <div v-if="!isDownloading" class="relative mt-4 shrink-0">
              <div class="grid grid-cols-2 gap-2">
                <button
                  class="theme-button-secondary rounded-2xl px-3 py-2.5 text-[12px] font-medium transition-colors"
                  @click="updaterStore.closeDialog()"
                >
                  {{ t(locale, 'settings.update.remindLater') }}
                </button>
                <button
                  class="theme-button-secondary flex items-center justify-center gap-1.5 rounded-2xl px-3 py-2.5 text-[12px] font-medium transition-colors"
                  @click="handleSkip"
                >
                  <SkipForward class="h-3.5 w-3.5" />
                  {{ t(locale, 'settings.update.skipVersion') }}
                </button>
              </div>

              <button
                class="theme-button-accent mt-2 flex w-full items-center justify-center gap-1.5 rounded-2xl px-3 py-3 text-[13px] font-semibold transition-colors"
                @click="handleInstall"
              >
                <Download class="h-4 w-4" />
                {{ t(locale, 'settings.update.installNow') }}
              </button>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.update-notes-card::after {
  content: '';
  pointer-events: none;
  position: absolute;
  inset: 0;
  border-radius: inherit;
  box-shadow: inset 0 1px 0 color-mix(in srgb, white 46%, transparent);
}

.release-notes-scroll {
  scrollbar-width: thin;
  scrollbar-color: color-mix(in srgb, var(--theme-text-tertiary) 42%, transparent) transparent;
}

.update-release-notes {
  color: var(--theme-text-secondary);
}

.update-release-notes:deep(h1),
.update-release-notes:deep(h2),
.update-release-notes:deep(h3),
.update-release-notes:deep(h4) {
  margin: 0 0 10px;
  color: var(--theme-text-primary);
  font-weight: 700;
  letter-spacing: -0.02em;
}

.update-release-notes:deep(h1),
.update-release-notes:deep(h2) {
  font-size: 16px;
}

.update-release-notes:deep(h3) {
  font-size: 14px;
}

.update-release-notes:deep(h4) {
  font-size: 13px;
}

.update-release-notes:deep(p) {
  margin: 0 0 10px;
  font-size: 12px;
  line-height: 1.72;
}

.update-release-notes:deep(ul) {
  margin: 0 0 12px;
  padding-left: 18px;
}

.update-release-notes:deep(li) {
  margin: 0 0 8px;
  font-size: 12px;
  line-height: 1.72;
}

.update-release-notes:deep(strong) {
  color: var(--theme-text-primary);
  font-weight: 700;
}

.update-release-notes:deep(code) {
  border: 1px solid color-mix(in srgb, var(--theme-border-default) 78%, transparent);
  border-radius: 8px;
  background: color-mix(in srgb, var(--theme-bg-overlay) 72%, transparent);
  padding: 1px 6px;
  font-size: 11px;
  color: var(--theme-text-primary);
}

.update-release-notes:deep(a) {
  color: var(--theme-accent-primary);
  text-decoration: none;
}

.update-release-notes:deep(a:hover) {
  text-decoration: underline;
}

.update-release-notes:deep(hr) {
  margin: 12px 0;
  border: 0;
  border-top: 1px solid color-mix(in srgb, var(--theme-border-default) 72%, transparent);
}
</style>
