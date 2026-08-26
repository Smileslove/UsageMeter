<script setup lang="ts">
/**
 * 右栏事件检查器（设计 9.7 / 12.5）。
 * - payload 分页查看器（tabs + nextCursor 续读 append）+ 复制 ID；
 * - 内容全部来自后端脱敏 payload；
 * - 显隐/折叠由父组件三栏布局控制：props.event 变化时自重置 payload 状态。
 */
import { computed, ref, watch } from 'vue'
import { Clock, Copy, Eye, EyeOff, FolderOpen, Link2, Loader2, Wrench, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t, backendErrorLabel } from '../../../i18n'
import { getSessionEventPayload } from '../../../api/activityApi'
import { useClipboard } from '../../composables/useClipboard'
import {
  KIND_META, statusMeta, linkMeta, contentStateLabel,
  formatAbsTime, formatDuration, formatBytes
} from './eventMeta'
import type { RedactedPayloadPage, SessionEventListItem } from '../../../types'

const props = defineProps<{
  /** 选中事件（null 显示空态）。 */
  event: SessionEventListItem | null
  /** 宽屏三栏模式：X 折叠而非关闭。 */
  wide: boolean
}>()

const emit = defineEmits<{
  close: []
  /** 宽屏模式折叠检查器（父组件置 inspectorCollapsed）。 */
  collapse: []
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

const payloadSection = ref<'summary' | 'input' | 'output'>('summary')
const PAYLOAD_SECTIONS: readonly ('summary' | 'input' | 'output')[] = [
  'summary',
  'input',
  'output'
]
const payloadSections = computed<readonly ('summary' | 'input' | 'output')[]>(() =>
  props.event?.tool ? PAYLOAD_SECTIONS : (['summary'] as const)
)
const payload = ref<RedactedPayloadPage | null>(null)
const payloadLoading = ref(false)
const payloadError = ref('')
const { copiedValue: copiedFlash, copyText: copyEventIdText } = useClipboard({ duration: 1400 })

const loadPayload = async () => {
  const event = props.event
  if (!event) return
  payloadLoading.value = true
  payloadError.value = ''
  payload.value = null
  try {
    payload.value = await getSessionEventPayload(event.eventKey, payloadSection.value)
  } catch (e) {
    payloadError.value = e instanceof Error ? e.message : String(e)
  } finally {
    payloadLoading.value = false
  }
}

/** M3 payload 分页（设计 12.5：nextCursor 非空时续读，append 到内容区）。 */
const loadMorePayload = async () => {
  const event = props.event
  const page = payload.value
  if (!event || !page || !page.nextCursor || payloadLoading.value) return
  payloadLoading.value = true
  payloadError.value = ''
  try {
    const next = await getSessionEventPayload(event.eventKey, payloadSection.value, undefined, page.nextCursor)
    payload.value = { ...next, content: page.content + next.content }
  } catch (e) {
    payloadError.value = e instanceof Error ? e.message : String(e)
  } finally {
    payloadLoading.value = false
  }
}

watch(() => props.event, event => {
  payload.value = null
  payloadError.value = ''
  if (event) {
    payloadSection.value = event.tool ? 'input' : 'summary'
    void loadPayload()
  }
}, { immediate: true })

const setPayloadSection = (section: 'summary' | 'input' | 'output') => {
  payloadSection.value = section
  void loadPayload()
}

const copyEventId = async () => {
  const event = props.event
  if (!event) return
  await copyEventIdText(event.eventKey, 'id')
}
</script>

<template>
  <!-- 右栏：事件检查器（360px；宽屏可折叠，窄屏 overlay drawer） -->
  <aside
    class="theme-surface flex shrink-0 flex-col overflow-hidden rounded-xl border"
    :class="wide ? '' : 'absolute inset-y-0 right-0 z-40 shadow-2xl'"
    style="width: min(360px, 86vw)"
    :aria-label="t(locale, 'desktop.activity.inspectorTitle')"
  >
    <!-- 检查器头部 -->
    <div class="flex shrink-0 items-center justify-between border-b border-[var(--theme-border-subtle)] px-3 py-2">
      <span class="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
        <Eye class="h-3.5 w-3.5" aria-hidden="true" />
        {{ t(locale, 'desktop.activity.inspectorTitle') }}
      </span>
      <button
        type="button"
        class="rounded p-1 text-[var(--theme-text-quaternary)] hover:bg-[var(--theme-bg-hover)]"
        :aria-label="t(locale, 'desktop.activity.inspectorClose')"
        :title="t(locale, 'desktop.activity.inspectorClose')"
        @click="wide ? emit('collapse') : emit('close')"
      >
        <X class="h-3.5 w-3.5" aria-hidden="true" />
      </button>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto p-3">
      <!-- 未选中 -->
      <div v-if="!event" class="flex flex-col items-center justify-center px-4 py-16 text-center">
        <EyeOff class="h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <p class="mt-2 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.inspectorEmpty') }}</p>
      </div>

      <div v-else class="space-y-3">
        <!-- 类型 / 时间 / 状态 -->
        <div class="flex items-start gap-2.5">
          <span class="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg" :class="KIND_META[event.kind].cls">
            <component :is="KIND_META[event.kind].icon" class="h-4 w-4" aria-hidden="true" />
          </span>
          <div class="min-w-0 flex-1">
            <div class="text-[12.5px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, KIND_META[event.kind].labelKey) }}</div>
            <div class="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-[var(--theme-text-tertiary)]">
              <span class="inline-flex items-center gap-1" :title="event.timestampMs != null ? formatAbsTime(event.timestampMs, locale) : undefined">
                <Clock class="h-3 w-3" aria-hidden="true" />
                {{ event.timestampMs != null ? formatAbsTime(event.timestampMs, locale) : t(locale, 'desktop.activity.timeUnknown') }}
              </span>
              <span
                v-if="statusMeta(event.status)"
                class="inline-flex items-center rounded-full px-1.5 py-px text-xs font-bold leading-none"
                :class="statusMeta(event.status)!.cls"
              >
                {{ t(locale, statusMeta(event.status)!.labelKey) }}
              </span>
            </div>
          </div>
        </div>

        <!-- 来源文件（脱敏路径，直接显示） -->
        <div class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
          <div class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inspectorSourceFile') }}</div>
          <div class="mt-1 flex items-center gap-1.5 break-all font-mono text-xs leading-relaxed text-[var(--theme-text-secondary)]">
            <FolderOpen class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            {{ event.sourceRef.sourceFilePath || '—' }}
          </div>
        </div>

        <!-- 摘要全文（不截断） -->
        <div v-if="event.summary" class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
          <div class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inspectorSummary') }}</div>
          <p class="mt-1 break-words text-xs leading-relaxed text-[var(--theme-text-secondary)]">{{ event.summary }}</p>
        </div>

        <!-- 请求关联（设计 9.9） -->
        <div v-if="event.requestLinks.length > 0" class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
          <div class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inspectorRequestLinks') }}</div>
          <ul class="mt-1 space-y-1">
            <li v-for="link in event.requestLinks" :key="link.requestKey" class="flex items-center gap-1.5">
              <Link2 class="h-3 w-3 shrink-0 text-[var(--theme-text-quaternary)]" :class="{ 'opacity-40': linkMeta(link.strength).dashed }" aria-hidden="true" />
              <span class="min-w-0 flex-1 truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="link.requestKey">{{ link.requestKey }}</span>
              <span
                class="shrink-0 rounded px-1.5 py-px text-xs font-bold leading-none"
                :class="linkMeta(link.strength).dashed
                  ? 'bg-amber-500/10 text-amber-600 dark:text-amber-300'
                  : 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'"
              >
                {{ t(locale, linkMeta(link.strength).labelKey) }}
              </span>
            </li>
          </ul>
        </div>

        <!-- 工具详情 -->
        <div v-if="event.tool" class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
          <div class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inspectorTool') }}</div>
          <div class="mt-1.5 space-y-1 text-xs text-[var(--theme-text-secondary)]">
            <div class="flex items-center gap-1.5">
              <Wrench class="h-3 w-3 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
              <span class="truncate font-mono font-semibold">{{ event.tool.normalizedName || event.tool.rawName }}</span>
              <span v-if="event.tool.family" class="rounded bg-cyan-500/10 px-1.5 py-px text-xs font-bold text-cyan-600 dark:text-cyan-300">{{ event.tool.family }}</span>
            </div>
            <div class="flex flex-wrap gap-x-3 gap-y-0.5">
              <span class="inline-flex items-center gap-1"><Clock class="h-3 w-3 text-[var(--theme-text-quaternary)]" aria-hidden="true" />{{ t(locale, 'desktop.activity.durationLabel') }}: {{ formatDuration(event.tool.durationMs) }}</span>
              <span v-if="event.tool.inputBytes != null">{{ t(locale, 'desktop.activity.inspectorInput') }}: {{ formatBytes(event.tool.inputBytes) }}</span>
              <span v-if="event.tool.outputBytes != null">{{ t(locale, 'desktop.activity.inspectorOutput') }}: {{ formatBytes(event.tool.outputBytes) }}</span>
            </div>
            <div v-if="event.tool.inputKeys.length > 0" class="flex flex-wrap items-center gap-1">
              <span class="text-xs text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.inputKeysLabel') }}:</span>
              <span v-for="key in event.tool.inputKeys" :key="key" class="max-w-32 truncate rounded bg-[var(--theme-border-subtle)] px-1.5 py-px font-mono text-xs text-[var(--theme-text-tertiary)]" :title="key">{{ key }}</span>
            </div>
            <div v-if="event.tool.resultKind" class="break-all font-mono text-xs text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.activity.resultKind', { kind: event.tool.resultKind }) }}</div>
          </div>
        </div>

        <!-- payload（设计 12.5：内容全部来自后端脱敏 payload） -->
        <div class="rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
          <div class="flex flex-wrap items-center gap-1">
            <button
              v-for="section in payloadSections"
              :key="section"
              type="button"
              class="rounded-md px-2 py-1 text-xs font-semibold capitalize transition-colors"
              :class="payloadSection === section
                ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]'
                : 'text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]'"
              @click="setPayloadSection(section)"
            >
              {{ t(locale, section === 'summary' ? 'desktop.activity.inspectorSummary' : section === 'input' ? 'desktop.activity.inspectorInput' : 'desktop.activity.inspectorOutput') }}
            </button>
            <span
              v-if="payload"
              class="ml-auto inline-flex items-center rounded px-1.5 py-px text-xs font-bold leading-none"
              :class="contentStateLabel(payload.contentState).cls"
            >
              {{ t(locale, contentStateLabel(payload.contentState).key) }}
            </span>
          </div>
          <div class="mt-2">
            <div v-if="payloadLoading" class="flex items-center justify-center gap-1.5 py-6 text-xs text-[var(--theme-text-tertiary)]">
              <Loader2 class="h-3 w-3 animate-spin" aria-hidden="true" />
              {{ t(locale, 'desktop.activity.timelineLoading') }}
            </div>
            <div v-else-if="payloadError" class="break-all py-2 text-xs text-rose-500">{{ backendErrorLabel(locale, payloadError) }}</div>
            <template v-else-if="payload">
              <p v-if="payload.truncated" class="mb-1.5 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.payloadTruncated') }}</p>
              <pre
                v-if="payload.content"
                class="max-h-72 overflow-y-auto whitespace-pre-wrap break-all rounded-lg bg-[var(--theme-bg-workspace)] p-2.5 font-mono text-xs leading-relaxed text-[var(--theme-text-secondary)]"
              >{{ payload.content }}</pre>
              <p v-else class="py-4 text-center text-xs text-[var(--theme-text-quaternary)]">
                {{ payload.contentState === 'unavailable' ? t(locale, 'desktop.activity.payloadUnavailable') : t(locale, 'desktop.activity.payloadEmpty') }}
              </p>
              <!-- M3 payload 分页（设计 12.5：nextCursor 续读 append） -->
              <button
                v-if="payload.nextCursor"
                type="button"
                class="mt-2 inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-[var(--theme-border-subtle)] px-2.5 py-1.5 text-xs font-semibold text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:opacity-60"
                :disabled="payloadLoading"
                @click="loadMorePayload"
              >
                <Loader2 v-if="payloadLoading" class="h-3 w-3 animate-spin" aria-hidden="true" />
                {{ t(locale, 'desktop.activity.payloadLoadMore') }}
              </button>
            </template>
          </div>
        </div>

        <!-- 动作：复制 ID（不复制敏感内容） -->
        <div class="flex items-center gap-2 pt-1">
          <button
            type="button"
            class="theme-button-secondary inline-flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-xs font-semibold"
            :aria-label="t(locale, 'desktop.activity.copyId')"
            :title="t(locale, 'desktop.activity.copyId')"
            @click="copyEventId"
          >
            <Copy class="h-3 w-3" aria-hidden="true" />
            {{ t(locale, 'desktop.activity.copyId') }}
          </button>
          <span v-if="copiedFlash" class="text-xs font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.activity.copied') }}</span>
          <span class="ml-auto max-w-36 truncate font-mono text-xs text-[var(--theme-text-quaternary)]" :title="event.eventKey">{{ event.eventKey }}</span>
        </div>
      </div>
    </div>
  </aside>
</template>
