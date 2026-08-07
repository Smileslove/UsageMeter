import { invoke } from '@tauri-apps/api/core'
import type { AppSettings, ProjectStats, RequestRecord, SessionStats } from '../types'

function withToolFilter(settings: AppSettings, toolFilter: string | null): AppSettings {
  return {
    ...settings,
    clientTools: {
      ...settings.clientTools,
      activeToolFilter: toolFilter
    }
  }
}

export function querySessions(
  settings: AppSettings,
  limit: number,
  offset: number,
  toolFilter?: string | null
): Promise<SessionStats[]> {
  return invoke('get_sessions', {
    limit,
    offset,
    settings: toolFilter === undefined ? settings : withToolFilter(settings, toolFilter)
  })
}

export function querySessionDetail(settings: AppSettings, sessionId: string): Promise<SessionStats | null> {
  return invoke('get_session_detail', { sessionId, settings })
}

export function queryRecentRequestRecords(
  settings: AppSettings,
  toolFilter: string | null,
  limit: number,
  offset: number
): Promise<RequestRecord[]> {
  return invoke('get_recent_request_records', {
    query: { limit, offset },
    settings: withToolFilter(settings, toolFilter)
  })
}

export function queryProjectStats(
  settings: AppSettings,
  toolFilter?: string | null
): Promise<ProjectStats[]> {
  return invoke('get_project_stats', {
    settings: toolFilter === undefined ? settings : withToolFilter(settings, toolFilter)
  })
}
