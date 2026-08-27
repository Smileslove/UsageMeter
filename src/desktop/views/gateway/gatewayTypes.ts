import type { GatewayCredentialRecovery, GatewayDispatchStrategy, GatewayLocalKey, GatewayProtocol, GatewayUpstreamKey, GatewayUpstreamModel } from '../../../types'

export interface DraftProfile {
  id?: string
  name: string
  protocol: GatewayProtocol
  baseUrl: string
  enabled: boolean
  clientLabel: string
  dispatchStrategy: GatewayDispatchStrategy
  upstreamKeys: GatewayUpstreamKey[]
  localKeys: GatewayLocalKey[]
  upstreamModels: GatewayUpstreamModel[]
  credentialRecovery: GatewayCredentialRecovery
}

export interface GatewayDraftInput {
  name: string
  protocol: GatewayProtocol
  baseUrl: string
  enabled: boolean
  clientLabel: string
  dispatchStrategy: GatewayDispatchStrategy
  upstreamSecret: string
}

export function emptyCredentialRecovery(): GatewayCredentialRecovery {
  return { upstreamKeyRequired: false, localKeyRotationRecommended: false }
}

const ERROR_KEY_MAP: Record<string, string> = {
  ERR_GATEWAY_LOCAL_KEY_ALREADY_EXISTS: 'gateway.singleLocalKeyHint',
  ERR_GATEWAY_UPSTREAM_KEY_ALREADY_EXISTS: 'gateway.singleUpstreamKeyHint',
  ERR_GATEWAY_UPSTREAM_KEY_MIGRATION_REQUIRED: 'gateway.upstreamKeyMigrationRequired',
  ERR_GATEWAY_LOCAL_KEY_REGENERATE_REQUIRED: 'gateway.localKeyRegenerateRequired',
}

export function gatewayErrorKey(code: string): string {
  if (code.startsWith('gateway.')) return code
  if (ERROR_KEY_MAP[code]) return ERROR_KEY_MAP[code]
  if (code.startsWith('ERR_GATEWAY_PROFILE_NAME')) return 'gateway.validationName'
  if (code.startsWith('ERR_GATEWAY_BASE_URL')) return 'gateway.validationUrl'
  return 'gateway.operationError'
}

export function isDisallowedUpstreamHost(hostname: string): boolean {
  const host = hostname.replace(/^\[|\]$/g, '').replace(/\.$/, '').toLowerCase()
  if (host === 'localhost') return true

  const ipv4 = host.split('.').map(Number)
  if (ipv4.length === 4 && ipv4.every(part => Number.isInteger(part) && part >= 0 && part <= 255)) {
    const [first, second] = ipv4
    return first === 0 ||
      first === 10 ||
      first === 127 ||
      first >= 224 ||
      (first === 100 && second >= 64 && second <= 127) ||
      (first === 169 && second === 254) ||
      (first === 172 && second >= 16 && second <= 31) ||
      (first === 192 && second === 168)
  }

  if (host === '::' || host === '::1' || host.startsWith('fc') || host.startsWith('fd')) return true
  if (/^fe[89ab]/.test(host) || host.startsWith('ff')) return true
  const mappedIpv4 = host.match(/^::ffff:(\d+\.\d+\.\d+\.\d+)$/)?.[1]
  return mappedIpv4 ? isDisallowedUpstreamHost(mappedIpv4) : false
}

export function validateGatewayDraft(draft: DraftProfile, input: GatewayDraftInput): string | null {
  if (!input.name.trim()) return 'gateway.validationName'
  if (!draft.id && !input.upstreamSecret.trim()) return 'gateway.validationUpstreamKey'
  try {
    const parsed = new URL(input.baseUrl.trim())
    if (
      parsed.protocol !== 'https:' ||
      !parsed.hostname ||
      parsed.username ||
      parsed.password ||
      parsed.search ||
      parsed.hash ||
      isDisallowedUpstreamHost(parsed.hostname)
    ) {
      return 'gateway.validationUrl'
    }
  } catch {
    return 'gateway.validationUrl'
  }
  return null
}
