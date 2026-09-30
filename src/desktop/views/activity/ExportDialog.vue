<script setup lang="ts">
/**
 * 导出会话活动对话框（15.3 / 21.5：范围预览，默认不勾选正文与工具 payload；
 * 成功显示路径可复制）。5 个勾选项 + 格式单选 + 路径复制全部自包含；
 * 通过 props.open 控制显隐、emit('close') 请求关闭。
 */
import { computed, ref, toRef, watch } from 'vue'
import { CheckCircle2, Copy, Loader2, TriangleAlert, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t, backendErrorLabel } from '../../../i18n'
import { exportSessionActivity } from '../../../api/activityApi'
import { useClipboard } from '../../composables/useClipboard'
import { useFocusTrap } from '../../composables/useFocusTrap'
import type { ExportResult } from '../../../types'

const props = defineProps<{
  open: boolean
  sessionKey: string | null
}>()

const emit = defineEmits<{
  close: []
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

const exportFormat = ref<'json' | 'csv'>('json')
const exportIncludeSummaries = ref(true)
const exportIncludeToolSummaries = ref(true)
const exportIncludeRequestLinks = ref(true)
const exportIncludePayloads = ref(false)
const exportBusy = ref(false)
const exportError = ref('')
const exportResult = ref<ExportResult | null>(null)
const { copiedValue: exportCopiedFlash, copyText: copyExportPathText } = useClipboard({ duration: 1400 })

// 打开时重置上次结果/错误（等价原 openExportDialog 逻辑）。
watch(() => props.open, open => {
  if (open) {
    exportResult.value = null
    exportError.value = ''
  }
})

const closeExportDialog = () => {
  if (exportBusy.value) return
  emit('close')
}

// 焦点陷阱（17.1）：打开时聚焦首个可聚焦元素、Tab 循环、Esc 关闭、关闭后焦点还给触发器
const dialogRoot = ref<HTMLElement | null>(null)
useFocusTrap({
  open: toRef(props, 'open'),
  container: dialogRoot,
  onClose: closeExportDialog
})

const runExport = async () => {
  const key = props.sessionKey
  if (!key || exportBusy.value) return
  exportBusy.value = true
  exportError.value = ''
  exportResult.value = null
  try {
    exportResult.value = await exportSessionActivity(key, {
      format: exportFormat.value,
      includeSummaries: exportIncludeSummaries.value,
      includeToolSummaries: exportIncludeToolSummaries.value,
      includeRequestLinks: exportIncludeRequestLinks.value,
      includePayloads: exportIncludePayloads.value
    })
  } catch (e) {
    exportError.value = e instanceof Error ? e.message : String(e)
  } finally {
    exportBusy.value = false
  }
}

const copyExportPath = async () => {
  if (!exportResult.value) return
  await copyExportPathText(exportResult.value.filePath, 'export-path')
}
</script>

<template>
  <!-- 导出对话框（15.3 / 21.5：范围预览，默认不勾选正文与工具 payload；成功显示路径可复制） -->
  <div
    v-if="open"
    ref="dialogRoot"
    class="theme-modal-backdrop fixed inset-0 z-[90] flex items-center justify-center"
    role="dialog"
    aria-modal="true"
    :aria-label="t(locale, 'desktop.activity.exportDialogTitle')"
  >
    <div class="absolute inset-0" @click="closeExportDialog"></div>
    <div class="theme-modal-shell relative max-w-[420px]" @click.stop>
      <div class="theme-modal-header">
        <h3 class="text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.exportDialogTitle') }}</h3>
        <button
          type="button"
          class="theme-modal-close -mr-2 -mt-2"
          :aria-label="t(locale, 'common.close')"
          :title="t(locale, 'common.close')"
          @click="closeExportDialog"
        >
          <X class="h-4 w-4" aria-hidden="true" />
        </button>
      </div>

      <!-- 范围预览 -->
      <div class="theme-modal-body space-y-2">
        <div class="flex items-center gap-2">
          <span class="w-24 shrink-0 text-xs font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.exportFormat') }}</span>
          <label class="flex cursor-pointer items-center gap-1.5 text-xs text-[var(--theme-text-secondary)]">
            <input
              v-model="exportFormat"
              type="radio"
              value="json"
              class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]"
            />
            {{ t(locale, 'desktop.activity.exportFormatJson') }}
          </label>
          <label class="flex cursor-pointer items-center gap-1.5 text-xs text-[var(--theme-text-secondary)]">
            <input
              v-model="exportFormat"
              type="radio"
              value="csv"
              class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]"
            />
            {{ t(locale, 'desktop.activity.exportFormatCsv') }}
          </label>
        </div>

        <div class="space-y-1.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-3 py-2.5">
          <div class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.exportScopePreview') }}</div>
          <label class="flex cursor-pointer items-center gap-2">
            <input v-model="exportIncludeSummaries" type="checkbox" class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]" />
            <span class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.activity.exportIncludeSummaries') }}</span>
          </label>
          <label class="flex cursor-pointer items-center gap-2">
            <input v-model="exportIncludeToolSummaries" type="checkbox" class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]" />
            <span class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.activity.exportIncludeToolSummaries') }}</span>
          </label>
          <label class="flex cursor-pointer items-center gap-2">
            <input v-model="exportIncludeRequestLinks" type="checkbox" class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]" />
            <span class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.activity.exportIncludeRequestLinks') }}</span>
          </label>
          <label class="flex cursor-pointer items-center gap-2">
            <input v-model="exportIncludePayloads" type="checkbox" class="h-3.5 w-3.5 accent-[var(--theme-accent-primary)]" />
            <span class="text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'desktop.activity.exportIncludePayloads') }}</span>
          </label>
          <p
            v-if="exportIncludePayloads"
            class="flex items-start gap-1.5 rounded-lg border border-amber-500/20 bg-amber-500/5 px-2.5 py-2 text-xs leading-relaxed text-amber-600 dark:text-amber-300"
          >
            <TriangleAlert class="mt-0.5 h-3 w-3 shrink-0" aria-hidden="true" />
            {{ t(locale, 'desktop.activity.exportPayloadWarning') }}
          </p>
        </div>
      </div>

      <!-- 结果 / 错误 -->
      <div class="mt-3">
        <p v-if="exportError" class="break-all text-xs leading-relaxed text-rose-500">{{ backendErrorLabel(locale, exportError) }}</p>
        <div v-else-if="exportResult" class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
          <div class="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-[var(--theme-text-tertiary)]">
            <CheckCircle2 class="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-300" aria-hidden="true" />
            <span>{{ t(locale, 'desktop.activity.exportSuccess') }}: {{ t(locale, 'desktop.activity.exportRows', { count: exportResult.rowCount }) }}</span>
            <span v-if="exportResult.truncated" class="text-amber-600 dark:text-amber-300">{{ t(locale, 'desktop.activity.exportTruncated') }}</span>
          </div>
          <div class="mt-1.5 flex items-center gap-1.5">
            <span class="min-w-0 flex-1 truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="exportResult.filePath">{{ exportResult.filePath }}</span>
            <button
              type="button"
              class="rounded p-1 text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)]"
              :aria-label="t(locale, 'desktop.activity.exportCopyPath')"
              :title="t(locale, 'desktop.activity.exportCopyPath')"
              @click="copyExportPath"
            >
              <Copy class="h-3 w-3" aria-hidden="true" />
            </button>
            <span v-if="exportCopiedFlash" class="text-xs font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.activity.copied') }}</span>
          </div>
        </div>
      </div>

      <!-- 操作 -->
      <div class="theme-modal-actions justify-end">
        <button
          type="button"
          class="theme-button-secondary inline-flex h-8 items-center rounded-lg px-3 text-xs font-semibold disabled:opacity-60"
          :disabled="exportBusy"
          @click="closeExportDialog"
        >
          {{ t(locale, 'common.cancel') }}
        </button>
        <button
          type="button"
          class="inline-flex h-8 items-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3.5 text-xs font-semibold text-[var(--theme-accent-contrast)] transition-opacity hover:opacity-90 disabled:opacity-60"
          :disabled="exportBusy"
          @click="runExport"
        >
          <Loader2 v-if="exportBusy" class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
          {{ exportBusy ? t(locale, 'desktop.activity.exportBusy') : t(locale, 'desktop.activity.exportButton') }}
        </button>
      </div>
    </div>
  </div>
</template>
