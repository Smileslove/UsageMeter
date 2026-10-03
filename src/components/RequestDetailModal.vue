<script setup lang="ts">
import { computed, onMounted, onUnmounted } from 'vue'
import {
  Activity,
  AlertCircle,
  ArrowDownToLine,
  ArrowUpFromLine,
  BarChart3,
  CheckCircle2,
  CircleHelp,
  Clock3,
  Code2,
  Copy,
  Database,
  DollarSign,
  FileCheck2,
  FileText,
  Info,
  Link2,
  Timer,
  X,
  Zap,
} from 'lucide-vue-next'
import type { RequestRecord } from '../types'
import { t } from '../i18n'
import { useMonitorStore } from '../stores/monitor'
import { useSessionDisplay } from '../composables/useSessionDisplay'
import { useClipboard } from '../desktop/composables/useClipboard'

const props = defineProps<{ visible: boolean; request: RequestRecord | null }>()
const emit = defineEmits<{ close: [] }>()
const store = useMonitorStore()
const {
  formatTime,
  formatCost,
  displayRequestCost,
  displayRequestTokens,
  formatDuration,
  requestCoverageLabel,
  requestModelLabel,
  requestProjectLabel,
  requestReconciliationStatusLabel,
  requestAccountingRoleLabel,
  requestReconciliationConfidenceLabel,
  requestObservationSourcesLabel,
  requestObservationIdLabel,
  requestSourceLabel,
  requestStatusClasses,
  requestStatusLabel,
  requestToolLabel,
} = useSessionDisplay(store)
const { copiedValue, copyText } = useClipboard()

const cacheHitRate = computed(() => {
  const request = props.request
  if (!request || request.provenance?.usageComplete === false) return '—'
  const denominator = (request.inputTokens || 0)
    + (request.outputTokens || 0)
    + (request.cacheCreateTokens || 0)
    + (request.cacheReadTokens || 0)
  if (denominator <= 0 || (request.cacheReadTokens || 0) <= 0) return denominator > 0 ? '0.0%' : '—'
  return `${((request.cacheReadTokens / denominator) * 100).toFixed(1)}%`
})

const requestStatusIcon = computed(() => {
  const request = props.request
  if (!request || request.coverageOrigin === 'local_only' || !request.statusCode) return CircleHelp
  return request.statusCode < 400 ? CheckCircle2 : AlertCircle
})

const projectValue = computed(() => {
  const request = props.request
  if (!request || request.provenance?.usageComplete === false) return '—'
  return `${requestProjectLabel(request)} / ${requestToolLabel(request.tool)}`
})

function copyRequestValue(value: string | null | undefined, label: string) {
  if (value?.trim()) void copyText(value, label)
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && props.visible) emit('close')
}

onMounted(() => window.addEventListener('keydown', handleKeydown))
onUnmounted(() => window.removeEventListener('keydown', handleKeydown))
</script>

<template>
  <Teleport to="#app">
    <div
      v-if="visible && request"
      class="request-detail-modal detail-modal-backdrop theme-modal-backdrop fixed inset-0 z-[90] flex items-center justify-center"
      style="-webkit-app-region: no-drag; app-region: no-drag"
      @click.self="emit('close')"
    >
      <div class="request-detail-modal__surface detail-modal-shell flex flex-col overflow-hidden" style="-webkit-app-region: no-drag; app-region: no-drag">
        <header class="request-detail-modal__header detail-modal-header">
          <div class="min-w-0 flex-1">
            <div class="detail-modal-meta flex flex-wrap items-center gap-2.5">
              <span class="request-detail-status" :class="requestStatusClasses(request)">
                <component :is="requestStatusIcon" class="h-4 w-4" :stroke-width="2.4" aria-hidden="true" />
                {{ requestStatusLabel(request) }}
              </span>
              <span class="detail-modal-meta-separator" aria-hidden="true">•</span>
              <span class="detail-modal-meta-time">{{ formatTime(request.timestampSec) }}</span>
            </div>
            <h2 class="detail-modal-title truncate">{{ requestModelLabel(request) }}</h2>
            <p class="detail-modal-subtitle truncate">
              {{ requestProjectLabel(request) }} <span class="mx-1 text-[var(--theme-border-strong)]">/</span> {{ requestToolLabel(request.tool) }} <span class="mx-1 text-[var(--theme-border-strong)]">/</span> {{ requestSourceLabel(request) }}
            </p>
          </div>
          <button type="button" class="request-detail-close detail-modal-close shrink-0" :aria-label="t(store.settings.locale, 'common.close')" :title="t(store.settings.locale, 'common.close')" @click="emit('close')">
            <X class="h-5 w-5" :stroke-width="2" aria-hidden="true" />
          </button>
        </header>

        <div class="request-detail-modal__body detail-modal-body min-h-0 flex-1 overflow-y-auto">
          <div v-if="request.tool === 'cursor'" class="mb-3 space-y-1 text-xs leading-relaxed text-[var(--theme-text-secondary)]">
            <p>{{ t(store.settings.locale, 'cursor.scopeNote') }}</p>
            <p v-if="request.provenance?.chargedAmountUsd != null">{{ t(store.settings.locale, 'cursor.chargedValue', { value: formatCost(request.provenance.chargedAmountUsd) }) }}</p>
          </div>
          <div class="grid grid-cols-4 gap-2 sm:gap-3">
            <div class="request-detail-metric"><div class="request-detail-metric__icon request-detail-metric__icon--blue"><FileText class="h-5 w-5" aria-hidden="true" /></div><span>{{ t(store.settings.locale, 'sessions.requestTotalTokens') }}</span><strong>{{ displayRequestTokens(request, request.totalTokens) }}</strong></div>
            <div class="request-detail-metric"><div class="request-detail-metric__icon request-detail-metric__icon--green"><DollarSign class="h-5 w-5" aria-hidden="true" /></div><span>{{ t(store.settings.locale, 'sessions.cost') }}</span><strong>{{ displayRequestCost(request) }}</strong></div>
            <div class="request-detail-metric"><div class="request-detail-metric__icon request-detail-metric__icon--blue"><Clock3 class="h-5 w-5" aria-hidden="true" /></div><span>{{ t(store.settings.locale, 'sessions.duration') }}</span><strong>{{ formatDuration(request.durationMs) }}</strong></div>
            <div class="request-detail-metric"><div class="request-detail-metric__icon request-detail-metric__icon--green"><Database class="h-5 w-5" aria-hidden="true" /></div><span>{{ t(store.settings.locale, 'statistics.cacheHitRate') }}</span><strong>{{ cacheHitRate }}</strong></div>
          </div>

          <section class="request-detail-section mt-4">
            <div class="request-detail-section__title"><div class="request-detail-section__title-icon request-detail-section__title-icon--blue"><FileText class="h-5 w-5" aria-hidden="true" /></div><h3>{{ t(store.settings.locale, 'sessions.requestTokenBreakdown') }}</h3></div>
            <div class="request-detail-table mt-4">
              <div class="request-detail-table__head"><span>{{ t(store.settings.locale, 'common.type') }}</span><span>{{ t(store.settings.locale, 'common.quantity') }}</span></div>
              <div class="request-detail-table__row"><div class="request-detail-table__label"><span class="request-detail-token-icon request-detail-token-icon--blue"><ArrowDownToLine class="h-4 w-4" aria-hidden="true" /></span><strong>{{ t(store.settings.locale, 'sessions.input') }}</strong></div><span class="request-detail-table__value">{{ displayRequestTokens(request, request.inputTokens) }} <small>{{ t(store.settings.locale, 'common.token') }}</small></span></div>
              <div class="request-detail-table__row"><div class="request-detail-table__label"><span class="request-detail-token-icon request-detail-token-icon--green"><ArrowUpFromLine class="h-4 w-4" aria-hidden="true" /></span><strong>{{ t(store.settings.locale, 'sessions.output') }}</strong></div><span class="request-detail-table__value">{{ displayRequestTokens(request, request.outputTokens) }} <small>{{ t(store.settings.locale, 'common.token') }}</small></span></div>
              <div class="request-detail-table__row"><div class="request-detail-table__label"><span class="request-detail-token-icon request-detail-token-icon--slate"><Database class="h-4 w-4" aria-hidden="true" /></span><strong>{{ t(store.settings.locale, 'statistics.cacheCreate') }}</strong></div><span class="request-detail-table__value">{{ displayRequestTokens(request, request.cacheCreateTokens) }} <small>{{ t(store.settings.locale, 'common.token') }}</small></span></div>
              <div class="request-detail-table__row"><div class="request-detail-table__label"><span class="request-detail-token-icon request-detail-token-icon--violet"><Database class="h-4 w-4" aria-hidden="true" /></span><strong>{{ t(store.settings.locale, 'statistics.cacheRead') }}</strong></div><span class="request-detail-table__value">{{ displayRequestTokens(request, request.cacheReadTokens) }} <small>{{ t(store.settings.locale, 'common.token') }}</small></span></div>
            </div>
            <p class="request-detail-note"><Info class="h-4 w-4 shrink-0" aria-hidden="true" />{{ t(store.settings.locale, 'sessions.requestCacheHitHint') }}</p>
          </section>

          <section class="request-detail-section mt-4">
            <div class="request-detail-section__title"><div class="request-detail-section__title-icon request-detail-section__title-icon--blue"><BarChart3 class="h-5 w-5" aria-hidden="true" /></div><h3>{{ t(store.settings.locale, 'sessions.requestDetailsTitle') }}</h3></div>
            <div class="request-detail-detail-grid mt-4">
              <div class="request-detail-detail-column"><div class="request-detail-detail-row"><span><Timer class="h-4 w-4" aria-hidden="true" />{{ t(store.settings.locale, 'sessions.ttft') }}</span><strong>{{ formatDuration(request.ttftMs) }}</strong></div><div class="request-detail-detail-row"><span><Zap class="h-4 w-4" aria-hidden="true" />{{ t(store.settings.locale, 'metrics.tokensPerSecond') }}</span><strong>{{ request.outputTokensPerSecond ? request.outputTokensPerSecond.toFixed(1) : '—' }}</strong></div></div>
              <div class="request-detail-detail-column"><div class="request-detail-detail-row"><span><Code2 class="h-4 w-4" aria-hidden="true" />{{ t(store.settings.locale, 'statistics.status') }}</span><strong>{{ request.statusCode || '—' }}</strong></div><div class="request-detail-detail-row"><span><FileCheck2 class="h-4 w-4" aria-hidden="true" />{{ t(store.settings.locale, 'sessions.requestCoverage') }}</span><strong>{{ requestCoverageLabel(request.coverageOrigin) }}</strong></div></div>
            </div>
          </section>

          <section class="request-detail-section mt-4">
            <div class="request-detail-section__title"><div class="request-detail-section__title-icon request-detail-section__title-icon--blue"><Link2 class="h-5 w-5" aria-hidden="true" /></div><h3>{{ t(store.settings.locale, 'sessions.requestSourceTitle') }}</h3></div>
            <div class="request-detail-source-list mt-4">
              <div class="request-detail-source-row"><span>{{ t(store.settings.locale, 'sessions.requestApiKey') }}</span><div class="request-detail-source-value"><strong class="truncate">{{ request.apiKeyPrefix || '—' }}</strong><button v-if="request.apiKeyPrefix" type="button" class="request-detail-copy" :aria-label="t(store.settings.locale, 'sessions.requestCopyValue')" :title="copiedValue === request.apiKeyPrefix ? t(store.settings.locale, 'sessions.copied') : t(store.settings.locale, 'sessions.requestCopyValue')" @click="copyRequestValue(request.apiKeyPrefix, request.apiKeyPrefix)"><CheckCircle2 v-if="copiedValue === request.apiKeyPrefix" class="h-4 w-4 text-emerald-500" aria-hidden="true" /><Copy v-else class="h-4 w-4" aria-hidden="true" /></button></div></div>
              <div class="request-detail-source-row"><span>{{ t(store.settings.locale, 'sessions.requestProject') }}</span><div class="request-detail-source-value"><strong class="truncate">{{ projectValue }}</strong><button type="button" class="request-detail-copy" :aria-label="t(store.settings.locale, 'sessions.requestCopyValue')" :title="copiedValue === projectValue ? t(store.settings.locale, 'sessions.copied') : t(store.settings.locale, 'sessions.requestCopyValue')" @click="copyRequestValue(projectValue, projectValue)"><CheckCircle2 v-if="copiedValue === projectValue" class="h-4 w-4 text-emerald-500" aria-hidden="true" /><Copy v-else class="h-4 w-4" aria-hidden="true" /></button></div></div>
            </div>
          </section>

          <details class="request-detail-advanced mt-4">
            <summary class="request-detail-advanced__title"><Activity class="h-4 w-4" aria-hidden="true" />{{ t(store.settings.locale, 'sessions.reconciliation') }}</summary>
            <div class="request-detail-advanced__grid"><div><span>{{ t(store.settings.locale, 'sessions.reconciliationStatus') }}</span><strong>{{ requestReconciliationStatusLabel(request) }}</strong></div><div><span>{{ t(store.settings.locale, 'sessions.accountingRole') }}</span><strong>{{ requestAccountingRoleLabel(request) }}</strong></div><div><span>{{ t(store.settings.locale, 'sessions.reconciliationConfidence') }}</span><strong>{{ requestReconciliationConfidenceLabel(request) }}</strong></div><div><span>{{ t(store.settings.locale, 'sessions.observationSources') }}</span><strong>{{ requestObservationSourcesLabel(request) }}</strong></div><div><span>{{ t(store.settings.locale, 'sessions.sessionId') }}</span><strong :title="request.sessionId">{{ requestObservationIdLabel(request.sessionId) }}</strong></div><div><span>{{ t(store.settings.locale, 'sessions.requestKey') }}</span><strong :title="request.requestKey">{{ requestObservationIdLabel(request.requestKey) }}</strong></div><div v-if="request.reconciliationMethod"><span>{{ t(store.settings.locale, 'sessions.reconciliationMethod') }}</span><strong :title="request.reconciliationMethod">{{ request.reconciliationMethod }}</strong></div><div v-if="request.localObservationKey"><span>{{ t(store.settings.locale, 'sessions.localObservationKey') }}</span><strong :title="request.localObservationKey">{{ requestObservationIdLabel(request.localObservationKey) }}</strong></div><div v-if="request.proxyObservationId"><span>{{ t(store.settings.locale, 'sessions.proxyObservationId') }}</span><strong :title="request.proxyObservationId">{{ requestObservationIdLabel(request.proxyObservationId) }}</strong></div></div>
          </details>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.request-detail-modal__surface { background: var(--theme-bg-overlay); border: 1px solid var(--theme-border-default); color: var(--theme-text-primary); }
.request-detail-modal__header { background: var(--theme-surface-gradient); border-bottom: 1px solid var(--theme-border-subtle); }
.request-detail-status { display: inline-flex; align-items: center; gap: 6px; border-radius: 999px; border-width: 1px; padding: 7px 12px; font-size: 13px; font-weight: 700; }
.request-detail-metric { min-width: 0; border: 1px solid var(--theme-border-subtle); border-radius: 18px; background: var(--theme-bg-surface); padding: 16px 14px 15px; box-shadow: var(--theme-shadow-inline); }
.request-detail-metric__icon, .request-detail-section__title-icon { display: inline-flex; align-items: center; justify-content: center; border-radius: 12px; }
.request-detail-metric__icon { width: 38px; height: 38px; margin-bottom: 12px; }
.request-detail-metric__icon--blue, .request-detail-section__title-icon--blue { color: #1769e0; background: rgba(59,130,246,.11); }
.request-detail-metric__icon--green { color: #079669; background: rgba(16,185,129,.11); }
.request-detail-metric > span { display: block; color: var(--theme-text-secondary); font-size: 12px; font-weight: 600; }
.request-detail-metric strong { display: block; margin-top: 6px; overflow: hidden; color: var(--theme-text-primary); font-family: var(--font-mono); font-size: clamp(1.15rem,3vw,1.7rem); font-variant-numeric: tabular-nums; font-weight: 750; line-height: 1.1; text-overflow: ellipsis; white-space: nowrap; }
.request-detail-metric small, .request-detail-table__value small { color: var(--theme-text-tertiary); font-size: 11px; }
.request-detail-metric small { display: block; margin-top: 4px; }
.request-detail-section { border: 1px solid var(--theme-border-subtle); border-radius: 20px; background: var(--theme-bg-surface); padding: 18px; box-shadow: var(--theme-shadow-inline); }
.request-detail-section__title { display: flex; align-items: center; gap: 10px; }
.request-detail-section__title-icon { width: 34px; height: 34px; }
.request-detail-section__title h3 { color: var(--theme-text-primary); font-size: 18px; font-weight: 750; letter-spacing: -0.02em; }
.request-detail-table { overflow: hidden; border-radius: 14px; background: var(--theme-bg-surface-muted); }
.request-detail-table__head, .request-detail-table__row { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 11px 14px; }
.request-detail-table__head { color: var(--theme-text-secondary); background: var(--theme-bg-app); font-size: 12px; font-weight: 600; }
.request-detail-table__row + .request-detail-table__row { border-top: 1px solid var(--theme-border-default); }
.request-detail-table__label, .request-detail-detail-row > span { display: flex; min-width: 0; align-items: center; gap: 10px; }
.request-detail-table__label strong { color: var(--theme-text-primary); font-size: 14px; }
.request-detail-token-icon { display: inline-flex; align-items: center; justify-content: center; width: 30px; height: 30px; border-radius: 999px; }
.request-detail-token-icon--blue { color: #1769e0; background: rgba(59,130,246,.1); }
.request-detail-token-icon--green { color: #07885f; background: rgba(16,185,129,.1); }
.request-detail-token-icon--slate { color: #64748b; background: rgba(100,116,139,.1); }
.request-detail-token-icon--violet { color: #7c3aed; background: rgba(124,58,237,.1); }
.request-detail-table__value { flex-shrink: 0; color: var(--theme-text-primary); font-family: var(--font-mono); font-size: 14px; font-variant-numeric: tabular-nums; font-weight: 650; }
.request-detail-note { display: flex; align-items: flex-start; gap: 9px; margin: 14px 2px 0; color: var(--theme-text-secondary); font-size: 12px; line-height: 1.55; }
.request-detail-detail-grid { display: grid; grid-template-columns: repeat(2,minmax(0,1fr)); }
.request-detail-detail-column + .request-detail-detail-column { border-left: 1px solid var(--theme-border-default); padding-left: 24px; }
.request-detail-detail-column:first-child { padding-right: 24px; }
.request-detail-detail-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 10px 0; }
.request-detail-detail-row + .request-detail-detail-row { border-top: 1px solid var(--theme-border-default); }
.request-detail-detail-row > span { color: var(--theme-text-secondary); font-size: 14px; }
.request-detail-detail-row > span svg { color: var(--theme-accent-primary); }
.request-detail-detail-row strong { color: var(--theme-text-primary); font-family: var(--font-mono); font-size: 14px; font-variant-numeric: tabular-nums; font-weight: 650; text-align: right; }
.request-detail-source-list { overflow: hidden; border-top: 1px solid var(--theme-border-default); }
.request-detail-source-row { display: flex; align-items: center; justify-content: space-between; gap: 20px; min-height: 45px; border-bottom: 1px solid var(--theme-border-default); color: var(--theme-text-secondary); font-size: 14px; }
.request-detail-source-value { display: flex; min-width: 0; align-items: center; gap: 8px; }
.request-detail-source-row strong { min-width: 0; color: var(--theme-text-primary); font-family: var(--font-mono); font-size: 14px; font-weight: 650; text-align: right; }
.request-detail-copy { display: inline-flex; flex-shrink: 0; align-items: center; justify-content: center; width: 32px; height: 32px; border-radius: 10px; color: var(--theme-text-secondary); background: var(--theme-bg-app); transition: background-color 180ms ease,color 180ms ease; }
.request-detail-copy:hover { color: var(--theme-accent-primary); background: var(--theme-accent-soft); }
.request-detail-copy:focus-visible { outline: none; box-shadow: 0 0 0 4px var(--theme-ring-focus); }
.request-detail-advanced { border: 1px solid var(--theme-border-default); border-radius: 16px; background: var(--theme-bg-surface-muted); padding: 14px 16px; }
.request-detail-advanced__title { display: flex; align-items: center; gap: 8px; color: var(--theme-text-secondary); font-size: 12px; font-weight: 700; }
.request-detail-advanced__grid { display: grid; grid-template-columns: repeat(2,minmax(0,1fr)); gap: 10px 20px; margin-top: 12px; }
.request-detail-advanced__grid > div { display: flex; min-width: 0; align-items: center; justify-content: space-between; gap: 12px; }
.request-detail-advanced__grid span { flex-shrink: 0; color: var(--theme-text-tertiary); font-size: 11px; }
.request-detail-advanced__grid strong { min-width: 0; overflow: hidden; color: var(--theme-text-secondary); font-family: var(--font-mono); font-size: 11px; font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
.request-detail-advanced summary { list-style: none; cursor: pointer; }
.request-detail-advanced summary::-webkit-details-marker { display: none; }
.request-detail-modal__body { scrollbar-width: thin; scrollbar-color: color-mix(in srgb, var(--theme-text-tertiary) 45%, transparent) transparent; }
.request-detail-modal__body::-webkit-scrollbar { width: 6px; }
.request-detail-modal__body::-webkit-scrollbar-track { background: transparent; }
.request-detail-modal__body::-webkit-scrollbar-thumb { border-radius: 999px; background: color-mix(in srgb, var(--theme-text-tertiary) 45%, transparent); }
@media (max-width: 639px) {
  .request-detail-modal { padding: 14px; }
  .request-detail-modal__surface { max-height: calc(100vh - 28px); border-radius: 20px; }
  .request-detail-modal__header { gap: 6px; padding: 12px 14px 11px; }
  .request-detail-modal__header h2 { margin-top: 6px; font-size: 22px; }
  .request-detail-modal__header p { margin-top: 3px; font-size: 11px; }
  .request-detail-status { gap: 4px; padding: 4px 8px; font-size: 10px; }
  .request-detail-status svg { width: 13px; height: 13px; }
  .request-detail-close { padding: 0; }
  .request-detail-close { min-width: 44px; min-height: 44px; }
  .request-detail-close svg { width: 18px; height: 18px; }
  .request-detail-modal__body { padding: 10px 14px 14px; }
  .request-detail-metric { border-radius: 12px; padding: 7px 5px 8px; box-shadow: none; }
  .request-detail-metric__icon { width: 24px; height: 24px; margin-bottom: 5px; border-radius: 8px; }
  .request-detail-metric__icon svg { width: 15px; height: 15px; }
  .request-detail-metric > span { font-size: 9px; line-height: 1.15; white-space: nowrap; }
  .request-detail-metric strong { margin-top: 3px; font-size: 14px; }
  .request-detail-metric small { margin-top: 1px; font-size: 8px; }
  .request-detail-modal__body .request-detail-section { margin-top: 8px; padding: 9px; border-radius: 14px; box-shadow: none; }
  .request-detail-section__title { gap: 6px; }
  .request-detail-section__title-icon { width: 24px; height: 24px; border-radius: 8px; }
  .request-detail-section__title-icon svg { width: 14px; height: 14px; }
  .request-detail-section__title h3 { font-size: 14px; }
  .request-detail-table { margin-top: 7px; border-radius: 10px; }
  .request-detail-table__head, .request-detail-table__row { gap: 6px; padding: 5px 7px; }
  .request-detail-table__head { font-size: 9px; }
  .request-detail-table__label { gap: 6px; }
  .request-detail-table__label strong { font-size: 11px; }
  .request-detail-token-icon { width: 22px; height: 22px; }
  .request-detail-token-icon svg { width: 12px; height: 12px; }
  .request-detail-table__value { font-size: 10px; }
  .request-detail-table__value small { font-size: 8px; }
  .request-detail-note { gap: 5px; margin-top: 7px; font-size: 9px; line-height: 1.3; }
  .request-detail-note svg { width: 12px; height: 12px; }
  .request-detail-detail-grid { margin-top: 6px; }
  .request-detail-detail-column:first-child { padding-right: 8px; }
  .request-detail-detail-column + .request-detail-detail-column { padding-left: 8px; }
  .request-detail-detail-row { gap: 5px; padding: 5px 0; }
  .request-detail-detail-row > span { gap: 5px; font-size: 10px; }
  .request-detail-detail-row > span svg { width: 13px; height: 13px; }
  .request-detail-detail-row strong { font-size: 10px; }
  .request-detail-source-list { margin-top: 6px; }
  .request-detail-source-row { min-height: 30px; gap: 6px; font-size: 10px; }
  .request-detail-source-row strong { font-size: 10px; }
  .request-detail-copy { width: 24px; height: 24px; border-radius: 7px; }
  .request-detail-copy svg { width: 12px; height: 12px; }
  .request-detail-advanced { margin-top: 8px; padding: 8px 10px; border-radius: 12px; }
  .request-detail-advanced__title { font-size: 9px; }
  .request-detail-advanced__title svg { width: 12px; height: 12px; }
  .request-detail-advanced__grid { gap: 6px; margin-top: 7px; }
  .request-detail-advanced__grid span, .request-detail-advanced__grid strong { font-size: 9px; }
}
@media (prefers-reduced-motion: reduce) { .request-detail-copy { transition: none; } }
</style>
