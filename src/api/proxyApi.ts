import { invoke } from '@tauri-apps/api/core'
import type { NetworkProxyConfig, ToolTakeoverStatus } from '../types'

export type TakeoverConflictAction = 'force_reclaim' | 'pause' | 'disable_takeover'

export interface NetworkProxyTestResult {
  ok: boolean
  reachable: boolean
  latencyMs?: number
  status?: number
  errorKind?: string
  errorDetail?: string
}

export function getTakeoverStatuses(): Promise<ToolTakeoverStatus[]> {
  return invoke('get_takeover_statuses')
}

export function setTakeoverForApp(app: string, enabled: boolean): Promise<void> {
  return invoke('set_takeover_for_app', { app, enabled })
}

export function resolveTakeoverConflict(
  tool: string,
  action: TakeoverConflictAction
): Promise<void> {
  return invoke('resolve_takeover_conflict', { tool, action })
}

export function testNetworkProxy(
  config: NetworkProxyConfig,
  target: string
): Promise<NetworkProxyTestResult> {
  return invoke('test_network_proxy', { config, target })
}
