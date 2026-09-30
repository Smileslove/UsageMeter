<script setup lang="ts">
import { computed, onMounted, onUnmounted } from 'vue'
import {
  Activity,
  AlertCircle,
  BarChart3,
  CheckCircle2,
  Clock3,
  DollarSign,
  FileText,
  Folder,
  Info,
  Timer,
  X,
  Zap,
} from 'lucide-vue-next'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import type { SessionStats } from '../types'
import { formatCost as formatCostUtil, formatTokenValue } from '../utils/format'
import { formatToolDisplayName } from '../utils/toolDisplay'

const props = defineProps<{ visible: boolean; session: SessionStats | null }>()
const emit = defineEmits<{ close: [] }>()
const store = useMonitorStore()
const uuidLikePattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

const formatTime = (epoch: number) => {
  if (!epoch) return '-'
  return new Date(epoch * 1000).toLocaleString(store.settings.locale.replace('_', '-'), {
    year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit'
  })
}

const formatDuration = (ms: number) => {
  if (!ms) return '-'
  if (ms < 1000) return `${ms}ms`
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`
  return `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s`
}

const formatTokens = (tokens: number) => (!tokens ? '0' : formatTokenValue(tokens))
const formatCost = (cost: number | undefined) => (
  cost === undefined || cost === null ? '-' : formatCostUtil(cost, store.settings.currency, 4)
)

const coveredRequests = computed(() => props.session?.coveredRequests || 0)
const uncoveredRequests = computed(() => props.session?.uncoveredRequests || 0)
const localRecordCount = computed(() => coveredRequests.value + uncoveredRequests.value)
const hasCoverageData = computed(() => (
  coveredRequests.value > 0 || uncoveredRequests.value > 0 || props.session?.usageFullyCovered === false
))
const sessionUsageVisible = computed(() => (
  props.session?.tool !== 'reasonix' || hasCoverageData.value
))
const sessionHasPartialCoverage = computed(() => props.session?.tool === 'reasonix' && uncoveredRequests.value > 0)
const totalTokens = computed(() => {
  const session = props.session
  if (!session) return 0
  return session.totalInputTokens + session.totalOutputTokens + session.totalCacheCreateTokens + session.totalCacheReadTokens
})
const displayTokens = (tokens: number) => (sessionUsageVisible.value ? formatTokens(tokens) : '—')
const displayCost = (cost: number | undefined) => (sessionUsageVisible.value ? formatCost(cost) : '—')
const displayRate = computed(() => {
  const session = props.session
  if (!sessionUsageVisible.value || !session || session.avgOutputTokensPerSecond <= 0) return '—'
  return session.avgOutputTokensPerSecond.toFixed(1)
})
const displayDuration = computed(() => (
  sessionUsageVisible.value && props.session ? formatDuration(props.session.totalDurationMs) : '—'
))
const displayProxyTokenValue = computed(() => {
  const session = props.session
  if (!session || coveredRequests.value <= 0) return '—'
  return formatTokens(session.totalInputTokens + session.totalOutputTokens + session.totalCacheCreateTokens + session.totalCacheReadTokens)
})
const displayProxyRate = computed(() => {
  const session = props.session
  if (!session || coveredRequests.value <= 0 || session.avgOutputTokensPerSecond <= 0) return '—'
  return `${session.avgOutputTokensPerSecond.toFixed(1)}t/s`
})
const displaySessionTitle = computed(() => {
  const session = props.session
  if (!session) return t(store.settings.locale, 'sessions.untitled')
  const sessionName = session.sessionName?.trim()
  if (session.topic?.trim()) return session.topic
  if (sessionName && !uuidLikePattern.test(sessionName)) return sessionName
  if (session.lastPrompt?.trim()) return session.lastPrompt
  if (session.projectName?.trim()) return session.projectName
  return t(store.settings.locale, 'sessions.untitled')
})
const displayToolName = computed(() => (
  props.session?.tool
    ? formatToolDisplayName(props.session.tool, store.settings.locale, store.settings.clientTools.profiles)
    : ''
))
const displayProjectBadge = computed(() => {
  const session = props.session
  if (!session) return ''
  if (session.projectName?.trim()) return session.projectName
  if (session.projectIdentity === 'global') return t(store.settings.locale, 'common.global')
  if (session.projectIdentity === 'unknown') return t(store.settings.locale, 'common.unknownProject')
  return ''
})
const projectBadgeClasses = computed(() => {
  if (props.session?.projectIdentity === 'global') return 'session-detail-project--global'
  if (props.session?.projectIdentity === 'unknown') return 'session-detail-project--unknown'
  return 'session-detail-project--named'
})
const projectHint = computed(() => {
  if (props.session?.projectIdentity === 'global') return t(store.settings.locale, 'sessions.globalSessionHint')
  if (props.session?.projectIdentity === 'unknown') return t(store.settings.locale, 'sessions.unknownProjectHint')
  return ''
})
const inputOutputRatio = computed(() => {
  const session = props.session
  if (!session) return { input: 50, output: 50 }
  const total = session.totalInputTokens + session.totalOutputTokens
  if (total === 0) return { input: 50, output: 50 }
  return { input: (session.totalInputTokens / total) * 100, output: (session.totalOutputTokens / total) * 100 }
})

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && props.visible) emit('close')
}

onMounted(() => window.addEventListener('keydown', handleKeydown))
onUnmounted(() => window.removeEventListener('keydown', handleKeydown))
</script>

<template>
  <Teleport to="#app">
    <div
      v-if="visible && session"
      class="session-detail-modal detail-modal-backdrop theme-modal-backdrop fixed inset-0 z-[90] flex items-center justify-center"
      style="-webkit-app-region: no-drag; app-region: no-drag"
      @click.self="emit('close')"
    >
      <div class="session-detail-modal__surface detail-modal-shell flex flex-col overflow-hidden" style="-webkit-app-region: no-drag; app-region: no-drag">
        <header class="session-detail-modal__header detail-modal-header">
          <div class="min-w-0 flex-1">
            <div class="session-detail-meta">
              <div v-if="displayProjectBadge" class="session-detail-project" :class="projectBadgeClasses">
                <Folder class="h-3.5 w-3.5" aria-hidden="true" />
                <span class="truncate">{{ displayProjectBadge }}</span>
              </div>
              <span v-if="displayProjectBadge" class="session-detail-meta-separator" aria-hidden="true">•</span>
              <span class="session-detail-meta-time">{{ formatTime(session.lastRequestTime) }}</span>
            </div>
            <h2 class="detail-modal-title line-clamp-2">{{ displaySessionTitle }}</h2>
            <p class="detail-modal-subtitle truncate">
              {{ session.models.join(', ') }}<span v-if="displayToolName" class="mx-1.5 text-[var(--theme-border-strong)]">/</span>{{ displayToolName }}
            </p>
          </div>
          <button type="button" class="session-detail-close detail-modal-close shrink-0" :aria-label="t(store.settings.locale, 'common.close')" :title="t(store.settings.locale, 'common.close')" @click="emit('close')">
            <X class="h-5 w-5" aria-hidden="true" />
          </button>
        </header>

        <div class="session-detail-modal__body detail-modal-body min-h-0 flex-1 overflow-y-auto">
          <div v-if="sessionHasPartialCoverage || projectHint" class="session-detail-notice">
            <Info class="h-4 w-4 shrink-0" aria-hidden="true" />
            <div class="min-w-0">
              <p v-if="sessionHasPartialCoverage">{{ t(store.settings.locale, 'sessions.coverageOnlyHint') }}</p>
              <p v-else>{{ projectHint }}</p>
              <div v-if="sessionHasPartialCoverage" class="session-detail-notice__meta">
                <span>{{ t(store.settings.locale, 'sessions.coveredRequests', { count: coveredRequests }) }}</span>
                <span v-if="uncoveredRequests > 0">{{ t(store.settings.locale, 'sessions.uncoveredRequests', { count: uncoveredRequests }) }}</span>
              </div>
            </div>
          </div>

          <div class="session-detail-metrics" :class="{ 'session-detail-metrics--partial': sessionHasPartialCoverage }">
            <div class="session-detail-metric"><div class="session-detail-metric__icon session-detail-metric__icon--blue"><FileText class="h-4 w-4" aria-hidden="true" /></div><span>{{ sessionHasPartialCoverage ? t(store.settings.locale, 'sessions.localRecords') : t(store.settings.locale, 'sessions.totalTokens') }}</span><strong>{{ sessionHasPartialCoverage ? localRecordCount : displayTokens(totalTokens) }}</strong></div>
            <div class="session-detail-metric"><div class="session-detail-metric__icon session-detail-metric__icon--green"><DollarSign class="h-4 w-4" aria-hidden="true" /></div><span>{{ sessionHasPartialCoverage ? t(store.settings.locale, 'sessions.proxyRecords') : t(store.settings.locale, 'sessions.estimatedCost') }}</span><strong>{{ sessionHasPartialCoverage ? coveredRequests : displayCost(session.estimatedCost) }}</strong></div>
            <div class="session-detail-metric"><div class="session-detail-metric__icon session-detail-metric__icon--violet"><Zap class="h-4 w-4" aria-hidden="true" /></div><span>{{ sessionHasPartialCoverage ? t(store.settings.locale, 'sessions.proxyTokens') : t(store.settings.locale, 'sessions.avgRate') }}</span><strong>{{ sessionHasPartialCoverage ? displayProxyTokenValue : displayRate }}</strong><small v-if="!sessionHasPartialCoverage">t/s</small></div>
            <div class="session-detail-metric"><div class="session-detail-metric__icon session-detail-metric__icon--blue"><Clock3 class="h-4 w-4" aria-hidden="true" /></div><span>{{ sessionHasPartialCoverage ? t(store.settings.locale, 'sessions.proxyRate') : t(store.settings.locale, 'sessions.duration') }}</span><strong>{{ sessionHasPartialCoverage ? displayProxyRate : displayDuration }}</strong></div>
          </div>

          <section class="session-detail-section">
            <div class="session-detail-section__title"><div class="session-detail-section__icon session-detail-section__icon--blue"><BarChart3 class="h-4 w-4" aria-hidden="true" /></div><h3>{{ t(store.settings.locale, 'sessions.inputOutput') }}</h3></div>
            <div class="session-detail-ratio" :class="{ 'session-detail-ratio--muted': !sessionUsageVisible }">
              <div class="session-detail-ratio__bar"><span :style="{ width: `${sessionUsageVisible ? inputOutputRatio.input : 50}%` }"></span><span :style="{ width: `${sessionUsageVisible ? inputOutputRatio.output : 50}%` }"></span></div>
              <div class="session-detail-ratio__legend"><span><b class="session-detail-dot session-detail-dot--input"></b>{{ t(store.settings.locale, 'common.inputTokens') }} <strong>{{ displayTokens(session.totalInputTokens) }}</strong></span><span><b class="session-detail-dot session-detail-dot--output"></b>{{ t(store.settings.locale, 'common.outputTokens') }} <strong>{{ displayTokens(session.totalOutputTokens) }}</strong></span></div>
            </div>
          </section>

          <section class="session-detail-section">
            <div class="session-detail-section__title"><div class="session-detail-section__icon session-detail-section__icon--green"><Activity class="h-4 w-4" aria-hidden="true" /></div><h3>{{ t(store.settings.locale, 'sessions.sessionPerformance') }}</h3></div>
            <div class="session-detail-stat-grid">
              <div><span>{{ t(store.settings.locale, 'sessions.requests') }}</span><strong>{{ sessionHasPartialCoverage ? localRecordCount : session.totalRequests }}</strong></div>
              <div><span><Timer class="h-3.5 w-3.5" aria-hidden="true" />{{ t(store.settings.locale, 'sessions.ttft') }}</span><strong>{{ sessionUsageVisible && session.avgTtftMs ? `${session.avgTtftMs.toFixed(0)}ms` : '—' }}</strong></div>
              <div><span><CheckCircle2 class="h-3.5 w-3.5" aria-hidden="true" />{{ t(store.settings.locale, 'common.success') }}</span><strong class="session-detail-stat--success">{{ sessionUsageVisible ? (session.successRequests || 0) : '—' }}</strong></div>
              <div><span><AlertCircle class="h-3.5 w-3.5" aria-hidden="true" />{{ t(store.settings.locale, 'common.error') }}</span><strong class="session-detail-stat--error">{{ sessionUsageVisible ? (session.errorRequests || 0) : '—' }}</strong></div>
            </div>
          </section>

          <details v-if="session.cwd || session.lastPrompt || session.firstRequestTime || session.lastRequestTime" class="session-detail-context">
            <summary><Folder class="h-4 w-4" aria-hidden="true" /><span>{{ t(store.settings.locale, 'sessions.sessionContext') }}</span></summary>
            <div class="session-detail-context__grid">
              <div v-if="session.cwd"><span>{{ t(store.settings.locale, 'settings.cwd') }}</span><strong :title="session.cwd">{{ session.cwd }}</strong></div>
              <div v-if="session.firstRequestTime"><span>{{ t(store.settings.locale, 'sessions.startTime') }}</span><strong>{{ formatTime(session.firstRequestTime) }}</strong></div>
              <div v-if="session.lastRequestTime"><span>{{ t(store.settings.locale, 'sessions.endTime') }}</span><strong>{{ formatTime(session.lastRequestTime) }}</strong></div>
              <div v-if="session.lastPrompt" class="session-detail-context__prompt"><span>{{ t(store.settings.locale, 'sessions.lastPrompt') }}</span><strong>{{ session.lastPrompt }}</strong></div>
            </div>
          </details>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.session-detail-modal__surface { background: var(--theme-bg-overlay); border: 1px solid var(--theme-border-default); color: var(--theme-text-primary); }
.session-detail-modal__header { background: var(--theme-surface-gradient); border-bottom: 1px solid var(--theme-border-subtle); }
.session-detail-modal__body { scrollbar-width: thin; scrollbar-color: color-mix(in srgb, var(--theme-text-tertiary) 45%, transparent) transparent; }
.session-detail-modal__body::-webkit-scrollbar { width: 6px; }
.session-detail-modal__body::-webkit-scrollbar-track { background: transparent; }
.session-detail-modal__body::-webkit-scrollbar-thumb { border-radius: 999px; background: color-mix(in srgb, var(--theme-text-tertiary) 45%, transparent); }
.session-detail-project { display: inline-flex; max-width: 100%; align-items: center; gap: 5px; border-radius: 999px; padding: 4px 8px; font-size: 10px; font-weight: 700; }
.session-detail-meta { display: flex; min-width: 0; align-items: center; gap: 6px; }
.session-detail-meta-separator { color: var(--theme-border-strong); font-size: 13px; }
.session-detail-meta-time { color: var(--theme-text-secondary); font-size: 11px; font-weight: 600; }
.session-detail-project--named { color: #6450b8; background: rgba(124,58,237,.1); }
.session-detail-project--global { color: var(--theme-text-secondary); background: var(--theme-bg-surface-muted); }
.session-detail-project--unknown { color: var(--theme-status-warning-fg); background: var(--theme-status-warning-bg); }
.session-detail-close { display: inline-flex; min-width: 44px; min-height: 44px; align-items: center; justify-content: center; }
.session-detail-notice { display: flex; align-items: flex-start; gap: 8px; margin-bottom: 8px; border: 1px solid var(--theme-border-default); border-radius: 12px; background: var(--theme-bg-surface-muted); padding: 8px 10px; color: var(--theme-text-secondary); font-size: 11px; line-height: 1.4; }
.session-detail-notice > svg { color: var(--theme-accent-primary); }
.session-detail-notice__meta { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 3px; color: var(--theme-text-tertiary); font-size: 10px; }
.session-detail-metrics { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; }
.session-detail-metric { min-width: 0; border: 1px solid var(--theme-border-subtle); border-radius: 18px; background: var(--theme-bg-surface); padding: 16px 14px 15px; box-shadow: var(--theme-shadow-inline); }
.session-detail-metric__icon, .session-detail-section__icon { display: inline-flex; align-items: center; justify-content: center; border-radius: 9px; }
.session-detail-metric__icon { width: 38px; height: 38px; margin-bottom: 12px; }
.session-detail-metric__icon--blue, .session-detail-section__icon--blue { color: #1769e0; background: rgba(59,130,246,.11); }
.session-detail-metric__icon--green, .session-detail-section__icon--green { color: #07885f; background: rgba(16,185,129,.11); }
.session-detail-metric__icon--violet { color: #7c3aed; background: rgba(124,58,237,.1); }
.session-detail-metric > span { display: block; overflow: hidden; color: var(--theme-text-secondary); font-size: 12px; font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
.session-detail-metric strong { display: block; margin-top: 6px; overflow: hidden; color: var(--theme-text-primary); font-family: var(--font-mono); font-size: clamp(1.15rem,3vw,1.7rem); font-variant-numeric: tabular-nums; font-weight: 750; line-height: 1.1; text-overflow: ellipsis; white-space: nowrap; }
.session-detail-metric small { display: block; margin-top: 4px; color: var(--theme-text-tertiary); font-size: 11px; }
.session-detail-section { margin-top: 16px; border: 1px solid var(--theme-border-subtle); border-radius: 20px; background: var(--theme-bg-surface); padding: 18px; box-shadow: var(--theme-shadow-inline); }
.session-detail-section__title { display: flex; align-items: center; gap: 7px; }
.session-detail-section__icon { width: 34px; height: 34px; }
.session-detail-section__title h3 { color: var(--theme-text-primary); font-size: 18px; font-weight: 750; letter-spacing: -0.02em; }
.session-detail-ratio { margin-top: 9px; }
.session-detail-ratio__bar { display: flex; height: 7px; overflow: hidden; border-radius: 999px; background: var(--theme-bg-surface-muted); }
.session-detail-ratio__bar span:first-child { background: #22c7d6; }
.session-detail-ratio__bar span:last-child { background: #c084fc; }
.session-detail-ratio--muted { opacity: .45; }
.session-detail-ratio__legend { display: flex; justify-content: space-between; gap: 10px; margin-top: 7px; color: var(--theme-text-secondary); font-size: 10px; }
.session-detail-ratio__legend > span { display: flex; min-width: 0; align-items: center; gap: 4px; }
.session-detail-ratio__legend strong { color: var(--theme-text-primary); font-family: var(--font-mono); font-weight: 650; }
.session-detail-dot { display: inline-block; width: 7px; height: 7px; border-radius: 50%; }
.session-detail-dot--input { background: #22c7d6; }
.session-detail-dot--output { background: #c084fc; }
.session-detail-stat-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 0 18px; margin-top: 5px; }
.session-detail-stat-grid > div { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 7px 0; border-bottom: 1px solid var(--theme-border-default); }
.session-detail-stat-grid span { display: inline-flex; align-items: center; gap: 5px; color: var(--theme-text-secondary); font-size: 11px; }
.session-detail-stat-grid strong { color: var(--theme-text-primary); font-family: var(--font-mono); font-size: 11px; font-variant-numeric: tabular-nums; font-weight: 650; }
.session-detail-stat-grid svg { color: var(--theme-accent-primary); }
.session-detail-stat--success { color: var(--theme-status-success-fg) !important; }
.session-detail-stat--error { color: var(--theme-status-danger-fg) !important; }
.session-detail-context { margin-top: 8px; border: 1px solid var(--theme-border-default); border-radius: 13px; background: var(--theme-bg-surface-muted); padding: 9px 10px; }
.session-detail-context summary { display: flex; align-items: center; gap: 6px; cursor: pointer; list-style: none; color: var(--theme-text-secondary); font-size: 11px; font-weight: 700; }
.session-detail-context summary::-webkit-details-marker { display: none; }
.session-detail-context summary svg { color: var(--theme-accent-primary); }
.session-detail-context__grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 7px 16px; margin-top: 8px; }
.session-detail-context__grid > div { min-width: 0; }
.session-detail-context__grid span { display: block; color: var(--theme-text-tertiary); font-size: 9px; }
.session-detail-context__grid strong { display: block; overflow: hidden; margin-top: 2px; color: var(--theme-text-secondary); font-family: var(--font-mono); font-size: 10px; font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
.session-detail-context__prompt { grid-column: 1 / -1; }
.session-detail-context__prompt strong { font-family: var(--font-sans); line-height: 1.35; white-space: normal; display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; }
@media (max-width: 639px) {
  .session-detail-modal { padding: 14px; }
  .session-detail-modal__surface { max-height: calc(100vh - 28px); border-radius: 20px; }
  .session-detail-modal__header { gap: 6px; padding: 12px 14px 11px; }
  .session-detail-modal__header h2 { margin-top: 6px; font-size: 22px; }
  .session-detail-modal__header p { margin-top: 3px; font-size: 10px; }
  .session-detail-project { padding: 3px 7px; font-size: 9px; }
  .session-detail-project svg { width: 12px; height: 12px; }
  .session-detail-close { min-width: 44px; min-height: 44px; }
  .session-detail-close svg { width: 18px; height: 18px; }
  .session-detail-modal__body { padding: 10px 14px 14px; }
  .session-detail-notice { margin-bottom: 7px; padding: 7px 8px; font-size: 10px; }
  .session-detail-notice__meta { font-size: 9px; }
  .session-detail-metrics { gap: 8px; }
  .session-detail-metric { border-radius: 12px; padding: 7px 5px 8px; box-shadow: none; }
  .session-detail-metric__icon { width: 24px; height: 24px; margin-bottom: 5px; border-radius: 8px; }
  .session-detail-metric__icon svg { width: 14px; height: 14px; }
  .session-detail-metric > span { font-size: 9px; }
  .session-detail-metric strong { margin-top: 3px; font-size: 14px; }
  .session-detail-metric small { font-size: 8px; }
  .session-detail-section { margin-top: 8px; padding: 9px; border-radius: 14px; box-shadow: none; }
  .session-detail-section__icon { width: 24px; height: 24px; }
  .session-detail-section__icon svg { width: 14px; height: 14px; }
  .session-detail-section__title h3 { font-size: 14px; }
  .session-detail-ratio { margin-top: 7px; }
  .session-detail-ratio__bar { height: 6px; }
  .session-detail-ratio__legend { margin-top: 5px; font-size: 9px; }
  .session-detail-ratio__legend strong { font-size: 9px; }
  .session-detail-stat-grid { gap: 0 12px; margin-top: 3px; }
  .session-detail-stat-grid > div { padding: 5px 0; }
  .session-detail-stat-grid span, .session-detail-stat-grid strong { font-size: 10px; }
  .session-detail-context { margin-top: 7px; padding: 8px 9px; }
  .session-detail-context__grid { gap: 6px 12px; margin-top: 7px; }
}
@media (prefers-reduced-motion: reduce) { .session-detail-close, .session-detail-modal__body * { transition: none !important; } }
</style>
