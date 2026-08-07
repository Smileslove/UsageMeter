import { invoke } from '@tauri-apps/api/core'
import type {
  ConfiguredSourceQuotaQueryResult,
  CopilotAuthStatus,
  GitHubAccount,
  SourceQuotaBindingConfig,
  SourceQuotaBindingRuntimeState,
  SourceQuotaBindingTestResult,
  SourceQuotaProfileDescriptor,
  SubscriptionQueryResult
} from '../types'

export function queryBoolean(command: string): Promise<boolean> {
  return invoke(command)
}

export function querySubscriptionQuota(
  provider: 'gpt' | 'claude' | 'gemini' | 'copilot',
  refresh = false
): Promise<SubscriptionQueryResult> {
  return invoke(refresh ? 'refresh_subscription_quota' : 'get_subscription_quota', { provider })
}

export function queryCopilotAuthStatus(): Promise<CopilotAuthStatus> {
  return invoke('copilot_get_auth_status')
}

export function queryCopilotAccounts(): Promise<GitHubAccount[]> {
  return invoke('copilot_list_accounts')
}

export function querySourceQuotaProfiles(): Promise<SourceQuotaProfileDescriptor[]> {
  return invoke('get_source_quota_profiles')
}

export function querySourceQuotaBindingStates(sourceId?: string): Promise<SourceQuotaBindingRuntimeState[]> {
  return invoke('get_source_quota_binding_states', { sourceId: sourceId ?? null })
}

export function probeSourceQuotaBinding(
  sourceId: string,
  binding: SourceQuotaBindingConfig | null
): Promise<SourceQuotaBindingRuntimeState> {
  return invoke('probe_source_quota_query', { sourceId, binding })
}

export function testSourceQuotaBinding(
  sourceId: string,
  binding: SourceQuotaBindingConfig
): Promise<SourceQuotaBindingTestResult> {
  return invoke('test_source_quota_query', { sourceId, binding })
}

export function queryConfiguredSourceQuotas(): Promise<ConfiguredSourceQuotaQueryResult> {
  return invoke('get_configured_source_quotas')
}
