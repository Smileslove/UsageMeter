/**
 * 网关协议选项与地址辅助（桌面网关页共享）。
 * 由 DesktopGateway.vue 及其子组件（列表 / 编辑表单）共用，保持单一来源。
 */
import { t } from '../../../i18n'
import type { GatewayProtocol } from '../../../types'

export interface GatewayProtocolOption {
  value: GatewayProtocol
  labelKey: string
  compactLabelKey: string
  basePath: string
}

export const gatewayProtocolOptions: GatewayProtocolOption[] = [
  { value: 'open_ai_chat_completions', labelKey: 'gateway.protocolOpenAiChat', compactLabelKey: 'gateway.protocolOpenAiChatCompact', basePath: '/v1' },
  { value: 'open_ai_responses', labelKey: 'gateway.protocolOpenAiResponses', compactLabelKey: 'gateway.protocolOpenAiResponses', basePath: '/v1' },
  { value: 'anthropic_messages', labelKey: 'gateway.protocolAnthropic', compactLabelKey: 'gateway.protocolAnthropic', basePath: '/v1' },
  { value: 'gemini_generate_content', labelKey: 'gateway.protocolGemini', compactLabelKey: 'gateway.protocolGemini', basePath: '/v1beta' }
]

export function compactProtocolLabel(locale: string | undefined, protocol: GatewayProtocol): string {
  const option = gatewayProtocolOptions.find(item => item.value === protocol)
  return option ? t(locale, option.compactLabelKey) : protocol
}

export function gatewayProfileAddress(listenerAddress: string, profile: { id: string; protocol: GatewayProtocol }): string {
  const option = gatewayProtocolOptions.find(item => item.value === profile.protocol)
  return `${listenerAddress}/gateway/${profile.id}${option?.basePath ?? '/v1'}`
}
