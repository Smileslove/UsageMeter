import type { AppLocale, ClientToolProfile } from '../types'
import { t } from '../i18n'
import { formatToolDisplayName } from './toolDisplay'

const QODER_FALLBACK_MODELS = new Set(['unknown', 'custom_model'])
const QODER_MODEL_DISPLAY_NAMES: Record<string, string> = {
  auto: 'Auto',
  dfmodel: 'DeepSeek-V4-Flash',
  dmodel: 'DeepSeek-V4-Pro',
  gm51model: 'GLM-5.2',
  kmodel: 'Kimi-K2.7-Code',
  mmodel: 'MiniMax-M2.7',
  q36fmodel: 'Qwen3.6-Flash',
  qmodel: 'Qwen3.7-Plus',
  qmodel_latest: 'Qwen3.7-Max',
  'qwork-advanced': 'Advanced',
  'qwork-auto': 'Standard',
  'qwork-ultimate': 'Premium'
}

export function isOpaqueModelId(model: string | null | undefined): boolean {
  const value = model?.trim().toLowerCase()
  return !value || QODER_FALLBACK_MODELS.has(value)
}

export function formatModelDisplayName(
  model: string | null | undefined,
  tool: string | null | undefined,
  locale: AppLocale | undefined,
  profiles: ClientToolProfile[]
): string {
  if (!isOpaqueModelId(model)) {
    const value = model!.trim()
    return tool?.startsWith('qoder_')
      ? QODER_MODEL_DISPLAY_NAMES[value.toLowerCase()] ?? value
      : value
  }

  if (tool?.startsWith('qoder_')) {
    return formatToolDisplayName(tool, locale, profiles)
  }

  return t(locale, 'common.unknown')
}
