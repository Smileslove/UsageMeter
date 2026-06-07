// Shareable color themes. Each drives the poster's accent (TOKEN label, brand mark,
// trend bars, footer) while the body stays on the light "治愈系" base.
export interface ShareTheme {
  id: string
  labelKey: string
  swatch: string
  accent: string
  accentDeep: string
  grad1: string
  grad2: string
  orbA: string
  orbB: string
}

export const SHARE_THEMES: ShareTheme[] = [
  { id: 'emerald', labelKey: 'statistics.shareThemeEmerald', swatch: '#10b981', accent: '#10b981', accentDeep: '#0f766e', grad1: '#34d399', grad2: '#10b981', orbA: 'rgba(20, 184, 166, 0.45)', orbB: 'rgba(45, 212, 191, 0.22)' },
  { id: 'violet', labelKey: 'statistics.shareThemeViolet', swatch: '#7c3aed', accent: '#7c3aed', accentDeep: '#5b21b6', grad1: '#a78bfa', grad2: '#7c3aed', orbA: 'rgba(124, 58, 237, 0.4)', orbB: 'rgba(167, 139, 250, 0.22)' },
  { id: 'sky', labelKey: 'statistics.shareThemeSky', swatch: '#0ea5e9', accent: '#0284c7', accentDeep: '#075985', grad1: '#38bdf8', grad2: '#0284c7', orbA: 'rgba(14, 165, 233, 0.4)', orbB: 'rgba(56, 189, 248, 0.22)' },
  { id: 'amber', labelKey: 'statistics.shareThemeAmber', swatch: '#f59e0b', accent: '#d97706', accentDeep: '#b45309', grad1: '#fbbf24', grad2: '#f59e0b', orbA: 'rgba(245, 158, 11, 0.4)', orbB: 'rgba(251, 191, 36, 0.22)' },
  { id: 'rose', labelKey: 'statistics.shareThemeRose', swatch: '#f43f5e', accent: '#e11d48', accentDeep: '#9f1239', grad1: '#fb7185', grad2: '#e11d48', orbA: 'rgba(244, 63, 94, 0.38)', orbB: 'rgba(251, 113, 133, 0.2)' }
]

export function shareThemeById(id: string): ShareTheme {
  return SHARE_THEMES.find(theme => theme.id === id) ?? SHARE_THEMES[0]
}
