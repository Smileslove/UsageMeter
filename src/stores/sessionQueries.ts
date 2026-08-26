import { invoke } from '@tauri-apps/api/core'
import type { AppSettings, ProjectStats, RequestQueryParams, RequestRecordsPage, RequestRecord, SessionStats } from '../types'

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

export function queryRequestRecordsPage(
  settings: AppSettings,
  toolFilter: string | null,
  params: RequestQueryParams
): Promise<RequestRecordsPage> {
  return invoke('get_request_records_page', {
    query: {
      limit: params.limit,
      offset: params.offset,
      search: params.search ?? null,
      status: params.status ?? null,
      coverage: params.coverage ?? null,
      performance: params.performance ?? null,
      sortField: params.sortField ?? null,
      sortDir: params.sortDir ?? null,
    },
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
