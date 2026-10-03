import { invoke } from '@tauri-apps/api/core'

export interface CursorSyncState {
  accountKey: string
  status: string
  errorCode: string | null
  lastAttemptMs: number | null
  lastSuccessMs: number | null
  nextRetryMs: number | null
  rangeStartMs: number | null
  rangeEndMs: number | null
  eventCount: number | null
}
export interface CursorStatus {
  metadataStatus: string
  accountSyncAvailable: boolean
  usageEventCount: number
  unknownCostEvents: number
  incompleteUsageEvents: number
  localAvailable: boolean
  authStatus: string
  accountKey: string | null
  accountSyncEnabled: boolean
  sync: CursorSyncState | null
}
export interface CursorCsvPreview {
  contentHash: string
  eventCount: number
  firstTimestampMs: number | null
  lastTimestampMs: number | null
  totalTokens: number
  usageCostUsd: number | null
  unknownCostEvents: number
  cacheWriteBasis: 'independent' | 'cumulative'
}
export interface CursorBatch {
  batchId: string
  accountKey: string
  source: 'csv' | 'api'
  rangeStartMs: number
  rangeEndMs: number
  eventCount: number
  importedAtMs: number
}
export const getCursorStatus = () => invoke<CursorStatus>('get_cursor_status')
export const syncCursorUsage = (startMs?: number) => invoke<CursorSyncState>('sync_cursor_usage', { startMs: startMs ?? null })
export const selectCursorCsv = () => invoke<string | null>('select_cursor_csv')
export const previewCursorCsvImport = (path: string) => invoke<CursorCsvPreview>('preview_cursor_csv_import', { path })
export const importCursorCsv = (path: string, contentHash: string, namespace: string, accountKey: string | null) =>
  invoke<CursorBatch>('import_cursor_usage_csv', { path, contentHash, namespace, accountKey })
export const listCursorImportBatches = () => invoke<CursorBatch[]>('list_cursor_import_batches')
export const revokeCursorImportBatch = (batchId: string) => invoke<void>('revoke_cursor_import_batch', { batchId })
