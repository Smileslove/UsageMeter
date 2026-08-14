import { invoke } from '@tauri-apps/api/core'
import type {
  AgentNodeDto,
  AppSettings,
  EventsPage,
  ExportOptions,
  ExportResult,
  GlobalSearchPage,
  RebuildResult,
  RedactedPayloadPage,
  SessionActivityCapability,
  SessionActivitySummary,
  SessionEventFilter,
  ToolSummaryRow
} from '../types'

/**
 * M2/M3 深度会话活动 IPC 封装（与 src-tauri/src/commands/activity.rs 对齐）。
 * 除 get_activity_capabilities 外，其余命令均带 settings 参数（后端读取
 * deep_index_level 做隐私门控：off 时查询返回空/None、rebuild 返回空结果）。
 * 分页默认 100 事件一页（后端 clamp 1..=200）；payload 默认上限 256KB。
 * M3：会话内搜索（search_session_activity）与跨会话搜索（search_activity_global）
 * 仅在 deep_index_level='fulltext' 时返回命中，其余档位返回空页——前端按
 * settings.deepIndexLevel 自行提示并禁用搜索执行。
 */

export function getActivityCapabilities(): Promise<SessionActivityCapability[]> {
  return invoke('get_activity_capabilities')
}

export function getSessionActivitySummary(
  settings: AppSettings,
  sessionKey: string
): Promise<SessionActivitySummary | null> {
  return invoke('get_session_activity_summary', { sessionKey, settings })
}

export function getSessionEvents(
  settings: AppSettings,
  sessionKey: string,
  filter: SessionEventFilter | null,
  offset: number,
  limit: number
): Promise<EventsPage> {
  return invoke('get_session_events', { sessionKey, filter, offset, limit, settings })
}

export function getSessionAgents(sessionKey: string): Promise<AgentNodeDto[]> {
  return invoke('get_session_agents', { sessionKey })
}

export function getSessionToolSummary(sessionKey: string): Promise<ToolSummaryRow[]> {
  return invoke('get_session_tool_summary', { sessionKey })
}

export function getSessionEventPayload(
  eventKey: string,
  section: string,
  maxBytes?: number,
  cursor?: string | null
): Promise<RedactedPayloadPage> {
  return invoke('get_session_event_payload', { eventKey, section, maxBytes, cursor })
}

/** M3：会话内全文搜索（仅 deep_index_level='fulltext' 返回命中，其余档位返回空页）。 */
export function searchSessionActivity(
  settings: AppSettings,
  sessionKey: string,
  query: string,
  offset: number,
  limit: number
): Promise<EventsPage> {
  return invoke('search_session_activity', { sessionKey, query, offset, limit, settings })
}

/** M3：跨会话全文搜索（同上，仅 fulltext 档返回命中）。 */
export function searchActivityGlobal(
  settings: AppSettings,
  query: string,
  offset: number,
  limit: number
): Promise<GlobalSearchPage> {
  return invoke('search_activity_global', { query, offset, limit, settings })
}

/** M3：导出会话活动（范围预览由前端对话框确认；payload 默认不包含，后端 serde(default) 双保险）。 */
export function exportSessionActivity(
  sessionKey: string,
  options: ExportOptions
): Promise<ExportResult> {
  return invoke('export_session_activity', { sessionKey, options })
}

export function rebuildSessionActivityIndex(scope: string): Promise<RebuildResult> {
  return invoke('rebuild_session_activity_index', { scope })
}

export function purgeSessionActivityContent(
  settings: AppSettings,
  scope: string
): Promise<number> {
  return invoke('purge_session_activity_content', { scope, settings })
}
