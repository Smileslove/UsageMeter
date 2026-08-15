<script setup lang="ts">
/**
 * 概览页"最近活跃会话"卡片（设计 6.8）。
 * 会话数据与展示辅助参数（locale/currency/profiles）由父页面传入；
 * 点击行打开会话、点击"查看全部"进入会话页，均以 emit 通知父页面。
 */
import type { PropType } from 'vue'
import { ArrowUpRight, Clock } from 'lucide-vue-next'
import { t } from '../../../i18n'
import { formatCost, formatRequestCount } from '../../../utils/format'
import { formatToolDisplayName } from '../../../utils/toolDisplay'
import type { AppLocale, ClientToolProfile, CurrencySettings, SessionStats } from '../../../types'

const props = defineProps({
  sessions: { type: Array as PropType<SessionStats[]>, required: true },
  /** store.sessions 总数（"查看全部"按钮显隐判断）。 */
  totalSessions: { type: Number, required: true },
  loading: { type: Boolean, required: true },
  locale: { type: String as PropType<AppLocale>, required: true },
  currency: { type: Object as PropType<CurrencySettings>, required: true },
  profiles: { type: Array as PropType<ClientToolProfile[]>, required: true }
})

const emit = defineEmits<{
  openSession: [sessionId: string]
  viewAll: []
}>()

function sessionTitle(session: { sessionName?: string; topic?: string }): string {
  return session.sessionName || session.topic || t(props.locale, 'sessions.untitled')
}

function formatRelativeTime(epoch: number): string {
  if (!epoch) return '--'
  const diffMs = Date.now() - epoch * 1000
  if (diffMs < 60_000) return t(props.locale, 'common.justNow')
  const minutes = Math.floor(diffMs / 60_000)
  if (minutes < 60) return `${minutes}${t(props.locale, 'common.minutesAgo')}`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}${t(props.locale, 'common.hoursAgo')}`
  return `${Math.floor(hours / 24)}${t(props.locale, 'common.daysAgo')}`
}
</script>

<template>
  <div class="rounded-lg border border-[var(--theme-border-default)] p-4 xl:col-span-2" style="background: var(--theme-surface-gradient)">
    <div class="mb-2 flex items-center justify-between gap-2">
      <h3 class="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
        <Clock :size="14" class="shrink-0" aria-hidden="true" />
        {{ t(locale, 'desktop.overview.sessionsTitle') }}
      </h3>
      <button
        v-if="sessions.length > 5 || totalSessions > 5"
        type="button"
        class="flex items-center gap-1 text-xs font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:underline"
        @click="emit('viewAll')"
      >
        {{ t(locale, 'desktop.overview.sessionsViewAll') }}
        <ArrowUpRight :size="13" aria-hidden="true" />
      </button>
    </div>

    <div v-if="loading && sessions.length === 0" class="flex flex-col gap-2">
      <div v-for="i in 3" :key="i" class="h-[52px] animate-pulse rounded-lg border border-[var(--theme-border-subtle)]"></div>
    </div>
    <div
      v-else-if="sessions.length === 0"
      class="grid place-items-center rounded-lg border border-dashed border-[var(--theme-border-strong)] px-4 py-8 text-center text-xs text-[var(--theme-text-tertiary)]"
    >
      {{ t(locale, 'desktop.overview.sessionsEmpty') }}
    </div>
    <ul v-else class="flex flex-col divide-y divide-[var(--theme-border-subtle)]">
      <li v-for="s in sessions" :key="s.sessionId">
        <button
          type="button"
          class="flex w-full items-center gap-3 rounded-md px-2 py-2 text-left transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
          :title="t(locale, 'desktop.overview.sessionsOpenHint')"
          @click="emit('openSession', s.sessionId)"
        >
          <span class="min-w-0 flex-1">
            <span class="block truncate text-xs font-semibold text-[var(--theme-text-primary)]">{{ sessionTitle(s) }}</span>
            <span class="mt-0.5 block truncate text-[11px] text-[var(--theme-text-tertiary)]">
              {{ s.projectName || t(locale, 'common.unknownProject') }}
              · {{ formatToolDisplayName(s.tool, locale, profiles) }}
              · {{ formatRelativeTime(s.lastRequestTime) }}
            </span>
          </span>
          <span class="flex shrink-0 items-center gap-3 font-mono text-[11px] text-[var(--theme-text-secondary)]">
            <span>{{ formatRequestCount(s.totalRequests) }}</span>
            <span>{{ formatCost(s.estimatedCost ?? 0, currency) }}</span>
            <ArrowUpRight :size="14" class="text-[var(--theme-text-quaternary)]" aria-hidden="true" />
          </span>
        </button>
      </li>
    </ul>
  </div>
</template>
