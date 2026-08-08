import { invoke } from '@tauri-apps/api/core'
import type {
  GatewayDispatchStrategy,
  GatewayProfile,
  GatewayProtocol,
  GatewayStatus
} from '../types'

export interface GatewayProfileInput {
  name: string
  protocol: GatewayProtocol
  baseUrl: string
  enabled: boolean
  clientLabel: string
  dispatchStrategy: GatewayDispatchStrategy
  upstreamSecret?: string
}

export interface GatewayUpstreamKeyInput {
  remark: string
  secret: string
  enabled: boolean
  weight: number
  priority: number
}

export interface GatewayUpstreamKeyUpdateInput {
  enabled: boolean
  weight: number
  priority: number
}

export function listGatewayProfiles(): Promise<GatewayProfile[]> {
  return invoke('list_gateway_profiles')
}

export function getGatewayStatus(): Promise<GatewayStatus> {
  return invoke('get_gateway_status')
}

export function createGatewayProfile(input: GatewayProfileInput): Promise<GatewayProfile> {
  return invoke('create_gateway_profile', { input })
}

export function updateGatewayProfile(id: string, input: GatewayProfileInput): Promise<GatewayProfile> {
  return invoke('update_gateway_profile', { id, input })
}

export function deleteGatewayProfile(id: string): Promise<void> {
  return invoke('delete_gateway_profile', { id })
}

export function createGatewayUpstreamKey(
  profileId: string,
  input: GatewayUpstreamKeyInput
): Promise<GatewayProfile> {
  return invoke('create_gateway_upstream_key', { profileId, input })
}

export function deleteGatewayUpstreamKey(profileId: string, keyId: string): Promise<GatewayProfile> {
  return invoke('delete_gateway_upstream_key', { profileId, keyId })
}

export function updateGatewayUpstreamKey(
  profileId: string,
  keyId: string,
  input: GatewayUpstreamKeyUpdateInput
): Promise<GatewayProfile> {
  return invoke('update_gateway_upstream_key', { profileId, keyId, input })
}

export function createGatewayLocalKey(
  profileId: string,
  input: { remark: string }
): Promise<{ key: string }> {
  return invoke('create_gateway_local_key', { profileId, input })
}

export function revokeGatewayLocalKey(profileId: string, keyId: string): Promise<GatewayProfile> {
  return invoke('revoke_gateway_local_key', { profileId, keyId })
}

export function revealGatewayLocalKey(profileId: string, keyId: string): Promise<string> {
  return invoke('reveal_gateway_local_key', { profileId, keyId })
}
