import type { AppLocale, ClientToolProfile } from '../types'
import { t } from '../i18n'
import { getFamilyForTool, getFamilyHead, getVariantLabel } from '../toolFamilies'

const FALLBACK_TOOL_NAMES: Record<string, string> = {
  copilot: 'GitHub Copilot CLI',
}

function translatedToolName(locale: AppLocale | undefined, tool: string): string | null {
  const key = `tools.${tool}`
  const translated = t(locale, key)
  return translated === key ? null : translated
}

export function getToolProfileByTool(profiles: ClientToolProfile[], tool: string): ClientToolProfile | undefined {
  const exact = profiles.find(profile => profile.tool === tool)
  if (exact) return exact

  const headId = getFamilyHead(tool)
  if (headId !== tool) {
    return profiles.find(profile => profile.tool === headId)
  }

  return undefined
}

export function formatToolDisplayName(
  tool: string | null | undefined,
  locale: AppLocale | undefined,
  profiles: ClientToolProfile[]
): string {
  if (!tool) return t(locale, 'common.unknown')

  const exactProfile = profiles.find(profile => profile.tool === tool)
  const familyProfile = getToolProfileByTool(profiles, tool)
  const headId = getFamilyHead(tool)
  const exactTranslated = translatedToolName(locale, tool)
  const translatedHead = headId !== tool ? translatedToolName(locale, headId) : null

  // If this exact tool already has a dedicated translated/display name, use it as-is.
  if (exactTranslated) return exactTranslated
  if (exactProfile?.displayName) return exactProfile.displayName

  const baseName = familyProfile?.displayName || translatedHead || FALLBACK_TOOL_NAMES[tool] || tool

  if (tool === headId) return baseName

  const variant = getVariantLabel(tool)
  return variant ? `${baseName} · ${variant}` : baseName
}

export function formatToolFilterDisplayName(
  tool: string | null | undefined,
  locale: AppLocale | undefined,
  profiles: ClientToolProfile[]
): string {
  if (!tool) return t(locale, 'common.unknown')

  const family = getFamilyForTool(tool)
  if (family?.head === tool && family.familyLabelKey) {
    const translated = t(locale, family.familyLabelKey)
    if (translated !== family.familyLabelKey) {
      return translated
    }
  }

  return formatToolDisplayName(tool, locale, profiles)
}
