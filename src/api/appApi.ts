import { invoke } from '@tauri-apps/api/core'
import type { DesktopNavigationTarget } from '../types'

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

/** 打开（或聚焦）桌面主窗口；可携带深链导航目标（target 为空时仅打开窗口）。 */
export function openDesktopWindow(target?: DesktopNavigationTarget): Promise<void> {
  return invoke('open_desktop_window', target ? { target } : {})
}

/** 取回主窗口加载期间暂存的待处理导航目标；无暂存时返回 null。 */
export function takePendingDesktopNavigation(): Promise<DesktopNavigationTarget | null> {
  return invoke('take_pending_desktop_navigation')
}
