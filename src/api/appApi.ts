import { invoke } from '@tauri-apps/api/core'

export function enableAutoStart(): Promise<void> {
  return invoke('enable_autostart')
}

export function disableAutoStart(): Promise<void> {
  return invoke('disable_autostart')
}

export function isAutoStartEnabled(): Promise<boolean> {
  return invoke('is_autostart_enabled')
}

export function listWslDistros(): Promise<string[]> {
  return invoke('list_wsl_distros')
}

export function openShareWindow(): Promise<void> {
  return invoke('open_share_window')
}
