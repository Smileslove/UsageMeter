/**
 * 活动页展示元数据常量与纯函数（从 DesktopActivity.vue 拆出，保持行为逐点一致）。
 *
 * - KIND_META：事件类型图标/配色/i18n key；
 * - statusMeta / linkMeta / contentStateLabel：状态、请求关联强度、payload 内容状态的
 *   徽标元数据（只返回 i18n key + class，由组件调用 t 渲染）；
 * - agentStatusKey / agentStatusMeta / shortAgentId：代理树展示辅助；
 * - formatAbsTime / formatRelTime / formatDuration / formatBytes：时间/字节格式化；
 * - ALL_KINDS / KIND_GROUPS：事件种类全集与过滤分组（与 useActivityData 共享）。
 */
import type { Component } from 'vue'
import {
  CircleHelp, FileOutput, GitBranch, Info, MessagesSquare, Shrink,
  TriangleAlert, User, Wrench
} from 'lucide-vue-next'
import { t } from '../../../i18n'
import { formatDurationMs } from '../../../utils/format'
import type {
  AgentNodeDto, RedactedPayloadPage, SessionEventKind, SessionEventListItem
} from '../../../types'

/** 事件种类全集（过滤与空态判断共用）。 */
export const ALL_KINDS: SessionEventKind[] = [
  'userMessage', 'assistantMessage', 'toolInvocation', 'toolResult',
  'agentStarted', 'agentFinished', 'systemEvent', 'compaction', 'error', 'unknown'
]

export interface KindMeta { icon: Component; cls: string; labelKey: string }

export const KIND_META: Record<SessionEventKind, KindMeta> = {
  userMessage: { icon: User, cls: 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-300', labelKey: 'desktop.activity.kindUserMessage' },
  assistantMessage: { icon: MessagesSquare, cls: 'bg-violet-500/10 text-violet-600 dark:text-violet-300', labelKey: 'desktop.activity.kindAssistantMessage' },
  toolInvocation: { icon: Wrench, cls: 'bg-cyan-500/10 text-cyan-600 dark:text-cyan-300', labelKey: 'desktop.activity.kindToolInvocation' },
  toolResult: { icon: FileOutput, cls: 'bg-teal-500/10 text-teal-600 dark:text-teal-300', labelKey: 'desktop.activity.kindToolResult' },
  agentStarted: { icon: GitBranch, cls: 'bg-fuchsia-500/10 text-fuchsia-600 dark:text-fuchsia-300', labelKey: 'desktop.activity.kindAgentStarted' },
  agentFinished: { icon: GitBranch, cls: 'bg-fuchsia-500/10 text-fuchsia-600 dark:text-fuchsia-300', labelKey: 'desktop.activity.kindAgentFinished' },
  systemEvent: { icon: Info, cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300', labelKey: 'desktop.activity.kindSystemEvent' },
  compaction: { icon: Shrink, cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300', labelKey: 'desktop.activity.kindCompaction' },
  error: { icon: TriangleAlert, cls: 'bg-rose-500/10 text-rose-600 dark:text-rose-300', labelKey: 'desktop.activity.kindError' },
  unknown: { icon: CircleHelp, cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300', labelKey: 'desktop.activity.kindUnknown' }
}

/** 事件类型过滤组（用户/模型/工具/子代理/错误/系统）。 */
export const KIND_GROUPS: Array<{ id: string; kinds: SessionEventKind[]; labelKey: string }> = [
  { id: 'user', kinds: ['userMessage'], labelKey: 'desktop.activity.filterUser' },
  { id: 'model', kinds: ['assistantMessage'], labelKey: 'desktop.activity.filterModel' },
  { id: 'tool', kinds: ['toolInvocation', 'toolResult'], labelKey: 'desktop.activity.filterTool' },
  { id: 'agent', kinds: ['agentStarted', 'agentFinished'], labelKey: 'desktop.activity.filterAgent' },
  { id: 'error', kinds: ['error'], labelKey: 'desktop.activity.filterError' },
  { id: 'system', kinds: ['systemEvent', 'compaction', 'unknown'], labelKey: 'desktop.activity.filterSystem' }
]

/** 事件状态徽标元数据（i18n key + class）；未知/空状态返回 null。 */
export const statusMeta = (status: SessionEventListItem['status']) => {
  switch (status) {
    case 'success':
      return { labelKey: 'desktop.activity.statusSuccess', cls: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300' }
    case 'error':
      return { labelKey: 'desktop.activity.statusError', cls: 'bg-rose-500/10 text-rose-600 dark:text-rose-300' }
    case 'running':
      return { labelKey: 'desktop.activity.statusRunning', cls: 'bg-sky-500/10 text-sky-600 dark:text-sky-300' }
    case 'pending':
      return { labelKey: 'desktop.activity.statusPending', cls: 'bg-amber-500/10 text-amber-600 dark:text-amber-300' }
    case 'cancelled':
      return { labelKey: 'desktop.activity.statusCancelled', cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300' }
    default:
      return null
  }
}

/** 请求关联强度元数据（9.9：exact/sourceExplicit 实线，timeWindow 虚线）。 */
export const linkMeta = (strength: string) =>
  strength === 'timeWindow'
    ? { labelKey: 'desktop.activity.linkInferred', dashed: true }
    : { labelKey: 'desktop.activity.linkExact', dashed: false }

/** payload 内容状态徽标元数据（i18n key + class）。 */
export const contentStateLabel = (state: RedactedPayloadPage['contentState']) => {
  switch (state) {
    case 'available': return { key: 'desktop.activity.contentStateAvailable', cls: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300' }
    case 'redacted': return { key: 'desktop.activity.contentStateRedacted', cls: 'bg-amber-500/10 text-amber-600 dark:text-amber-300' }
    case 'truncated': return { key: 'desktop.activity.contentStateTruncated', cls: 'bg-sky-500/10 text-sky-600 dark:text-sky-300' }
    case 'unavailable': return { key: 'desktop.activity.contentStateUnavailable', cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300' }
    default: return { key: 'desktop.activity.contentStateNone', cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300' }
  }
}

/** 跨会话搜索命中 kind 归一化（未知 → 'unknown'）。 */
export const hitKind = (kind: string): SessionEventKind =>
  kind in KIND_META ? (kind as SessionEventKind) : 'unknown'

export const agentStatusKey = (status: AgentNodeDto['status']) => {
  switch (status) {
    case 'success': return 'desktop.activity.statusSuccess'
    case 'error': return 'desktop.activity.statusError'
    case 'running': return 'desktop.activity.statusRunning'
    case 'pending': return 'desktop.activity.statusPending'
    case 'cancelled': return 'desktop.activity.statusCancelled'
    default: return 'desktop.activity.statusUnknown'
  }
}

export const agentStatusMeta = (status: AgentNodeDto['status']) => {
  switch (status) {
    case 'success': return 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300'
    case 'error': return 'bg-rose-500/10 text-rose-600 dark:text-rose-300'
    case 'running': return 'bg-sky-500/10 text-sky-600 dark:text-sky-300'
    case 'cancelled': return 'bg-slate-500/10 text-slate-500 dark:text-slate-300'
    default: return 'bg-slate-500/10 text-slate-500 dark:text-slate-300'
  }
}

export const shortAgentId = (key: string) => (key.length > 14 ? `${key.slice(0, 8)}…${key.slice(-4)}` : key)

/** 绝对时间（跟随 locale）。 */
export const formatAbsTime = (ms: number, locale: string) =>
  new Date(ms).toLocaleString(locale.replace('_', '-'), {
    year: 'numeric', month: 'short', day: 'numeric',
    hour: '2-digit', minute: '2-digit', second: '2-digit'
  })

/** 相对时间（分钟/小时前，超过 24h 回退绝对时间）。 */
export const formatRelTime = (ms: number, locale: string) => {
  const diff = Date.now() - ms
  const mins = Math.floor(diff / 60000)
  if (mins < 1) return t(locale, 'common.justNow')
  if (mins < 60) return t(locale, 'sessions.timeMinutesAgo', { count: mins })
  const hours = Math.floor(mins / 60)
  if (hours < 24) return t(locale, 'sessions.timeHoursAgo', { count: hours })
  return formatAbsTime(ms, locale)
}

export const formatDuration = (ms: number | null | undefined) =>
  ms == null ? '—' : formatDurationMs(ms)

export const formatBytes = (bytes: number | null | undefined) => {
  if (bytes == null) return '—'
  if (bytes < 1024) return `${bytes}B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`
  return `${(bytes / (1024 * 1024)).toFixed(2)}MB`
}
