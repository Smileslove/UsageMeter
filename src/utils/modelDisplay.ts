import type { AppLocale, ClientToolProfile } from '../types'
import { t } from '../i18n'
import { formatToolDisplayName } from './toolDisplay'

const QODER_FALLBACK_MODELS = new Set(['unknown', 'custom_model'])

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
    return model!.trim()
  }

  if (tool?.startsWith('qoder_')) {
    return formatToolDisplayName(tool, locale, profiles)
  }

  return t(locale, 'common.unknown')
}

