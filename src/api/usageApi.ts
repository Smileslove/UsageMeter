import { invoke } from '@tauri-apps/api/core'
import type {
  AppSettings,
  StatisticsBucket,
  StatisticsMetric,
  StatisticsSummary
} from '../types'

export interface StatisticsSummaryQuery {
  startEpoch: number
  endEpoch: number
  timezone: string
  bucket: StatisticsBucket
  metric: StatisticsMetric
}

export interface LocalCacheStats {
  totalLocalFacts: number
  orphanLocalFacts: number
}

export interface OpenCodeSchemaStatus {
  dbFound: boolean
  dbPath: string | null
  schemaCompatible: boolean
  compatibilityMode?: 'full' | 'message_only' | 'incompatible'
  persistedCompatibilityMode?: 'full' | 'message_only' | 'incompatible' | 'unknown' | null
  incompatibilityReason: string | null
  messageIdConflict?: {
    hasConflict: boolean
    conflictCount: number
    sampleIds: string[]
  }
}

export function getStatisticsSummary(
  query: StatisticsSummaryQuery,
  settings: AppSettings
): Promise<StatisticsSummary> {
  return invoke('get_statistics_summary', { query, settings })
}

export function getLocalCacheStats(): Promise<LocalCacheStats> {
  return invoke('get_local_usage_maintenance_stats')
}

export function purgeOrphanLocalFacts(olderThanDays: number): Promise<number> {
  return invoke('purge_orphan_local_facts', { olderThanDays })
}

export function rebuildLocalUsageCache(): Promise<void> {
  return invoke('rebuild_local_usage_cache')
}

export function getOpenCodeSchemaStatus(): Promise<OpenCodeSchemaStatus> {
  return invoke('get_opencode_schema_status')
}
