import { invoke } from '@tauri-apps/api/core'

export function renameSource(sourceId: string, name: string): Promise<void> {
  return invoke('rename_api_source', { sourceId, name })
}

export function deleteSource(sourceId: string, alsoDeleteRecords: boolean): Promise<void> {
  return invoke('delete_api_source', { sourceId, alsoDeleteRecords })
}

export function mergeSource(sourceIdFrom: string, sourceIdInto: string): Promise<void> {
  return invoke('merge_api_source', { sourceIdFrom, sourceIdInto })
}

export function addKeyPrefix(sourceId: string, keyPrefix: string): Promise<void> {
  return invoke('add_key_prefix_to_source', { sourceId, keyPrefix })
}

export function updateSourceKeyNote(sourceId: string, keyPrefix: string, note: string): Promise<void> {
  return invoke('update_api_source_key_note', { sourceId, keyPrefix, note })
}
