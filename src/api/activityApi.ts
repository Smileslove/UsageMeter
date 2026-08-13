import { invoke } from '@tauri-apps/api/core'
import type {
  AgentNodeDto,
  AppSettings,
  EventsPage,
  RebuildResult,
  RedactedPayloadPage,
  SessionActivityCapability,
  SessionActivitySummary,
  SessionEventFilter,
  ToolSummaryRow
} from '../types'

/**
 * M2 深度会话活动 IPC 封装（与 src-tauri/src/commands/activity.rs 对齐）。
 * 除 get_activity_capabilities 外，其余命令均带 settings 参数（后端读取
 * deep_index_level 做隐私门控：off 时查询返回空/None、rebuild 返回空结果）。
 * 分页默认 100 事件一页（后端 clamp 1..=200）；payload 默认上限 256KB。
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

export function getSessionAgents(
  settings: AppSettings,
  sessionKey: string
): Promise<AgentNodeDto[]> {
  return invoke('get_session_agents', { sessionKey, settings })
}

export function getSessionToolSummary(
  settings: AppSettings,
  sessionKey: string
): Promise<ToolSummaryRow[]> {
  return invoke('get_session_tool_summary', { sessionKey, settings })
}

export function getSessionEventPayload(
  settings: AppSettings,
  eventKey: string,
  section: string,
  maxBytes?: number
): Promise<RedactedPayloadPage> {
  return invoke('get_session_event_payload', { eventKey, section, maxBytes, settings })
}

export function rebuildSessionActivityIndex(
  settings: AppSettings,
  scope: string
): Promise<RebuildResult> {
  return invoke('rebuild_session_activity_index', { scope, settings })
}

export function purgeSessionActivityContent(
  settings: AppSettings,
  scope: string
): Promise<number> {
  return invoke('purge_session_activity_content', { scope, settings })
}
