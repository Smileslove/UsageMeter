<script setup lang="ts">
import type { RequestRecord } from '../types'
import { X } from 'lucide-vue-next'
import { t } from '../i18n'
import { useMonitorStore } from '../stores/monitor'
import { useSessionDisplay } from '../composables/useSessionDisplay'

defineProps<{
  visible: boolean
  request: RequestRecord | null
}>()

const emit = defineEmits<{
  close: []
}>()

const store = useMonitorStore()
const {
  formatTime,
  formatTokens,
  formatCost,
  formatDuration,
  requestCoverageLabel,
  requestModelLabel,
  requestProjectLabel,
  requestAttributionLabel,
  requestSourceLabel,
  requestStatusClasses,
  requestStatusLabel,
  requestToolLabel,
} = useSessionDisplay(store)
</script>

<template>
  <Teleport to="#app">
    <div
      v-if="visible && request"
      class="fixed inset-0 z-[80] flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm"
      style="-webkit-app-region: no-drag; app-region: no-drag"
      @click.self="emit('close')"
    >
      <div class="w-full max-w-md overflow-hidden rounded-2xl bg-white shadow-2xl dark:bg-[#1C1C1E]">
        <div class="flex items-start justify-between border-b border-gray-100 p-4 dark:border-neutral-800">
          <div class="min-w-0 pr-3">
            <div class="mb-1 flex items-center gap-1.5">
              <span class="request-card__status" :class="requestStatusClasses(request)">
                {{ requestStatusLabel(request) }}
              </span>
              <span class="text-[10px] text-gray-400">{{ formatTime(request.timestampSec) }}</span>
            </div>
            <h3 class="truncate text-base font-semibold text-gray-800 dark:text-gray-100">
              {{ requestModelLabel(request) }}
            </h3>
            <p class="mt-0.5 truncate text-[10px] text-gray-400">
              {{ requestProjectLabel(request) }} / {{ requestToolLabel(request.tool) }} / {{ requestSourceLabel(request) }}
            </p>
          </div>
          <button
            type="button"
            class="shrink-0 rounded-lg p-1.5 transition-colors hover:bg-gray-100 dark:hover:bg-neutral-800"
            :aria-label="t(store.settings.locale, 'common.close')"
            :title="t(store.settings.locale, 'common.close')"
            @click="emit('close')"
          >
            <X class="h-4 w-4 text-gray-400" :stroke-width="2" aria-hidden="true" />
          </button>
        </div>

        <div class="max-h-[calc(80vh-64px)] space-y-3 overflow-y-auto p-4">
          <div class="grid grid-cols-3 gap-2">
            <div class="request-detail-stat">
              <span>{{ t(store.settings.locale, 'common.totalTokens') }}</span>
              <strong>{{ formatTokens(request.totalTokens) }}</strong>
            </div>
            <div class="request-detail-stat">
              <span>{{ t(store.settings.locale, 'sessions.cost') }}</span>
              <strong class="text-[var(--theme-chart-cost)]">{{ formatCost(request.estimatedCost) }}</strong>
            </div>
            <div class="request-detail-stat">
              <span>{{ t(store.settings.locale, 'sessions.duration') }}</span>
              <strong>{{ formatDuration(request.durationMs) }}</strong>
            </div>
          </div>

          <div class="request-detail-section">
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'sessions.input') }}</span>
              <strong>{{ formatTokens(request.inputTokens) }}</strong>
            </div>
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'sessions.output') }}</span>
              <strong>{{ formatTokens(request.outputTokens) }}</strong>
            </div>
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'statistics.cacheCreate') }}</span>
              <strong>{{ formatTokens(request.cacheCreateTokens) }}</strong>
            </div>
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'statistics.cacheRead') }}</span>
              <strong>{{ formatTokens(request.cacheReadTokens) }}</strong>
            </div>
          </div>

          <div class="request-detail-section">
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'sessions.ttft') }}</span>
              <strong>{{ formatDuration(request.ttftMs) }}</strong>
            </div>
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'metrics.tokensPerSecond') }}</span>
              <strong>{{ request.outputTokensPerSecond ? request.outputTokensPerSecond.toFixed(1) : '—' }}</strong>
            </div>
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'statistics.status') }}</span>
              <strong>{{ request.statusCode || '—' }}</strong>
            </div>
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'sessions.requestCoverage') }}</span>
              <strong>{{ requestCoverageLabel(request.coverageOrigin) }}</strong>
            </div>
          </div>

          <div class="request-detail-section">
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'common.source') }}</span>
              <strong class="truncate text-right">{{ requestSourceLabel(request) }}</strong>
            </div>
            <div v-if="requestAttributionLabel(request)" class="request-detail-row">
              <span>{{ t(store.settings.locale, 'sessions.requestAttribution') }}</span>
              <strong>{{ requestAttributionLabel(request) }}</strong>
            </div>
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'sessions.sessionId') }}</span>
              <strong class="truncate text-right">{{ request.sessionId || '—' }}</strong>
            </div>
            <div class="request-detail-row">
              <span>{{ t(store.settings.locale, 'sessions.requestKey') }}</span>
              <strong class="truncate text-right">{{ request.requestKey }}</strong>
            </div>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.request-card__status {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  border-width: 1px;
  border-radius: 9999px;
  padding: 2px 6px;
  font-size: 9px;
  font-weight: 800;
  line-height: 1;
}

.request-detail-stat {
  display: flex;
  flex-direction: column;
  gap: 3px;
  border-radius: 14px;
  background: var(--theme-bg-surface);
  padding: 9px 8px;
  text-align: center;
}

.request-detail-stat span,
.request-detail-row span {
  font-size: 10px;
  color: var(--theme-text-tertiary);
}

.request-detail-stat strong {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 13px;
  color: var(--theme-text-primary);
}

.request-detail-section {
  border: 1px solid var(--theme-border-subtle);
  border-radius: 14px;
  background: var(--theme-bg-elevated);
  padding: 8px 10px;
}

.request-detail-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 4px 0;
}

.request-detail-row strong {
  min-width: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  color: var(--theme-text-primary);
}
</style>
