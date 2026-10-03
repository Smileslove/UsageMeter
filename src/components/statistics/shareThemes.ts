// Each poster palette defines its own surface, type, and chart colors.
export interface ShareTheme {
  id: string
  labelKey: string
  appearance: 'dark' | 'light'
  swatch: string
  background: string
  foreground: string
  muted: string
  ink: string
}

export const SHARE_THEMES: ShareTheme[] = [
  { id: 'emerald', labelKey: 'statistics.shareThemeEmerald', appearance: 'dark', swatch: '#24483e', background: '#183c34', foreground: '#f3efdf', muted: '#b9cebb', ink: '#b9cebb' },
  { id: 'violet', labelKey: 'statistics.shareThemeViolet', appearance: 'dark', swatch: '#8b80ad', background: '#383047', foreground: '#f3efdf', muted: '#c9bddb', ink: '#c9bddb' },
  { id: 'sky', labelKey: 'statistics.shareThemeSky', appearance: 'dark', swatch: '#718da2', background: '#263c4d', foreground: '#f3efdf', muted: '#b8cfdf', ink: '#b8cfdf' },
  { id: 'amber', labelKey: 'statistics.shareThemeAmber', appearance: 'dark', swatch: '#b69a5c', background: '#493d28', foreground: '#f3efdf', muted: '#dfcfa5', ink: '#dfcfa5' },
  { id: 'rose', labelKey: 'statistics.shareThemeRose', appearance: 'dark', swatch: '#b57481', background: '#4a2f38', foreground: '#f3efdf', muted: '#dfbfc7', ink: '#dfbfc7' },
  { id: 'ivory', labelKey: 'statistics.shareThemeIvory', appearance: 'light', swatch: '#eeeadb', background: '#f6f4eb', foreground: '#2e3d32', muted: '#5c6c5c', ink: '#70866a' },
  { id: 'lavender', labelKey: 'statistics.shareThemeLavender', appearance: 'light', swatch: '#e9e1f0', background: '#f3eff8', foreground: '#3b3148', muted: '#6c607b', ink: '#8a74a2' },
  { id: 'mist', labelKey: 'statistics.shareThemeMist', appearance: 'light', swatch: '#dce8f0', background: '#edf3f7', foreground: '#293e50', muted: '#5d7181', ink: '#678aa3' },
  { id: 'sand', labelKey: 'statistics.shareThemeSand', appearance: 'light', swatch: '#f2e3c8', background: '#faf3e6', foreground: '#493c29', muted: '#796447', ink: '#a88446' },
  { id: 'blush', labelKey: 'statistics.shareThemeBlush', appearance: 'light', swatch: '#f0dce1', background: '#f8eef0', foreground: '#4c333a', muted: '#815e69', ink: '#b17c8a' }
]

export function shareThemeById(id: string): ShareTheme {
  return SHARE_THEMES.find(theme => theme.id === id) ?? SHARE_THEMES[0]
}
