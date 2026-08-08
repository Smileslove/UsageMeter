import { invoke } from '@tauri-apps/api/core'
import type { AppSettings, RemoteSyncDevice, SyncStatus } from '../types'

export interface SyncCredentials {
  password: string
  syncPassword: string
}

export interface RotateSyncPasswordPayload {
  currentSyncPassword: string
  newSyncPassword: string
}

export function getSyncStatus(settings: AppSettings): Promise<SyncStatus> {
  return invoke('get_sync_status', { settings })
}

export function listSyncDevices(): Promise<RemoteSyncDevice[]> {
  return invoke('list_sync_devices')
}

export function getActiveSyncDeviceId(): Promise<string | null> {
  return invoke('get_active_sync_device_id')
}

export function testWebDavConnection(settings: AppSettings, credentials: SyncCredentials): Promise<void> {
  return invoke('test_webdav_connection', { settings, credentials })
}

export function syncNow(settings: AppSettings, credentials: SyncCredentials): Promise<SyncStatus> {
  return invoke('sync_now', { settings, credentials })
}

export function removeSyncDevice(deviceId: string): Promise<void> {
  return invoke('remove_sync_device', { deviceId })
}

export function clearImportedSyncData(): Promise<void> {
  return invoke('clear_imported_sync_data')
}

export function rotateSyncPassword(
  settings: AppSettings,
  credentials: SyncCredentials,
  payload: RotateSyncPasswordPayload
): Promise<void> {
  return invoke('rotate_sync_password', { settings, credentials, payload })
}
