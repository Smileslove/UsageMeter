import type { ThemeAppearance, ThemeDarkPalette, ThemeLightPalette, ThemePalette, ThemePaletteOption, ThemeSettings } from './types'

export interface ResolvedTheme {
  appearance: Exclude<ThemeAppearance, 'system'>
  palette: ThemePalette
  isDark: boolean
}

export const DEFAULT_THEME_SETTINGS: ThemeSettings = {
  appearance: 'system',
  lightPalette: 'cloud',
  darkPalette: 'midnight'
}

export const LIGHT_THEME_PALETTES: ThemeLightPalette[] = ['dawn', 'cloud', 'mist', 'moss', 'parchment', 'rose']
export const DARK_THEME_PALETTES: ThemeDarkPalette[] = ['midnight', 'graphite', 'forest']

export const THEME_PALETTE_OPTIONS: ThemePaletteOption[] = [
  { id: 'dawn', key: 'settings.paletteDawn', preview: ['#FFF6F4', '#FFD6CF', '#6F8DDD'], family: 'light' },
  { id: 'cloud', key: 'settings.paletteCloud', preview: ['#F3FAFF', '#BFE4FF', '#2D86D9'], family: 'light' },
  { id: 'mist', key: 'settings.paletteMist', preview: ['#F3F5F7', '#CDD6E2', '#7084A4'], family: 'light' },
  { id: 'moss', key: 'settings.paletteMoss', preview: ['#F2F6EF', '#C8DABF', '#4C7A4E'], family: 'nature' },
  { id: 'parchment', key: 'settings.paletteParchment', preview: ['#F6EBD8', '#F0C89C', '#B04A2D'], family: 'warm' },
  { id: 'rose', key: 'settings.paletteRose', preview: ['#FFF3F5', '#F2C8D3', '#C86479'], family: 'warm' }
]

export const DARK_THEME_PALETTE_OPTIONS: ThemePaletteOption[] = [
  { id: 'midnight', key: 'settings.paletteMidnight', preview: ['#0B1020', '#12244E', '#5DA0FF'], family: 'dark' },
  { id: 'graphite', key: 'settings.paletteGraphite', preview: ['#12161D', '#263141', '#8FAEE8'], family: 'dark' },
  { id: 'forest', key: 'settings.paletteForest', preview: ['#101713', '#1D3A31', '#7BC4A1'], family: 'dark' }
]

export function systemPrefersDark(): boolean {
  return window.matchMedia('(prefers-color-scheme: dark)').matches
}

export function resolveTheme(theme: ThemeSettings): ResolvedTheme {
  const prefersDark = theme.appearance === 'system' ? systemPrefersDark() : theme.appearance === 'dark'
  const appearance = prefersDark ? 'dark' : 'light'
  const palette = prefersDark ? theme.darkPalette : theme.lightPalette
  return {
    appearance,
    palette,
    isDark: prefersDark
  }
}

export function applyResolvedTheme(theme: ThemeSettings) {
  const resolved = resolveTheme(theme)
  const root = document.documentElement
  root.dataset.appearance = resolved.appearance
  root.dataset.palette = resolved.palette
  root.classList.toggle('dark', resolved.isDark)
  return resolved
}

export function nextAppearance(appearance: ThemeAppearance): ThemeAppearance {
  const appearances: ThemeAppearance[] = ['system', 'light', 'dark']
  const currentIndex = appearances.indexOf(appearance)
  return appearances[(currentIndex + 1) % appearances.length]
}

export function themeColorVar(name: string): string {
  return `var(${name})`
}
