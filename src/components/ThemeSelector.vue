<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Check, Monitor, Moon, Sun } from 'lucide-vue-next'
import { useMonitorStore } from '../stores/monitor'
import { applyResolvedTheme, DARK_THEME_PALETTE_OPTIONS, THEME_PALETTE_OPTIONS } from '../theme'
import { t } from '../i18n'
import type { ThemeAppearance, ThemeDarkPalette, ThemeLightPalette, ThemeSettings } from '../types'

const store = useMonitorStore()

const isOpen = ref(false)
const dropdownRef = ref<HTMLElement | null>(null)

const appearanceOptions: Array<{ id: ThemeAppearance; icon: typeof Monitor }> = [
  { id: 'system', icon: Monitor },
  { id: 'light', icon: Sun },
  { id: 'dark', icon: Moon }
]

const lightGroups = computed(() => ({
  light: THEME_PALETTE_OPTIONS.filter(item => item.family === 'light'),
  nature: THEME_PALETTE_OPTIONS.filter(item => item.family === 'nature'),
  warm: THEME_PALETTE_OPTIONS.filter(item => item.family === 'warm')
}))

const currentAppearanceIcon = computed(() => {
  const match = appearanceOptions.find(item => item.id === store.settings.theme.appearance)
  return match?.icon ?? Monitor
})

const systemPaletteHint = computed(() => {
  const lightLabel = t(store.settings.locale, `settings.palette${store.settings.theme.lightPalette.charAt(0).toUpperCase() + store.settings.theme.lightPalette.slice(1)}`)
  const darkLabel = t(store.settings.locale, `settings.palette${store.settings.theme.darkPalette.charAt(0).toUpperCase() + store.settings.theme.darkPalette.slice(1)}`)
  return t(store.settings.locale, 'settings.themePickerSystemHint', { light: lightLabel, dark: darkLabel })
})

async function updateTheme(mutator: (theme: ThemeSettings) => void) {
  const previousTheme = { ...store.settings.theme }
  mutator(store.settings.theme)

  try {
    await store.saveSettings()
  } catch {
    store.settings.theme = previousTheme
    applyResolvedTheme(previousTheme)
  }
}

async function setAppearance(appearance: ThemeAppearance) {
  await updateTheme(theme => {
    theme.appearance = appearance
  })
}

async function setLightPalette(palette: ThemeLightPalette) {
  await updateTheme(theme => {
    theme.lightPalette = palette
  })
}

async function setDarkPalette(palette: ThemeDarkPalette) {
  await updateTheme(theme => {
    theme.darkPalette = palette
  })
}

function toggleDropdown() {
  isOpen.value = !isOpen.value
}

function closeDropdown() {
  isOpen.value = false
}

function handleClickOutside(event: MouseEvent) {
  if (dropdownRef.value && !dropdownRef.value.contains(event.target as Node)) {
    closeDropdown()
  }
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside)
})

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside)
})
</script>

<template>
  <div ref="dropdownRef" class="relative">
    <button
      @click="toggleDropdown"
      class="theme-icon-button p-1.5 rounded-full transition-all select-none cursor-pointer"
      :title="t(store.settings.locale, 'settings.themePicker')"
    >
      <component :is="currentAppearanceIcon" class="h-3.5 w-3.5 text-[var(--theme-accent-primary)]" />
    </button>

    <Transition
      enter-active-class="transition ease-out duration-120"
      enter-from-class="transform opacity-0 scale-95"
      enter-to-class="transform opacity-100 scale-100"
      leave-active-class="transition ease-in duration-90"
      leave-from-class="transform opacity-100 scale-100"
      leave-to-class="transform opacity-0 scale-95"
    >
      <div
        v-if="isOpen"
        class="theme-popover scrollbar-hide absolute top-full right-0 z-50 mt-2 max-h-[calc(100vh-88px)] w-[248px] overflow-y-auto overscroll-contain rounded-[20px] border p-2 shadow-[0_16px_34px_rgba(15,23,42,0.14)] backdrop-blur-2xl"
      >
        <div class="theme-popover__section">
          <div class="theme-popover__section-title">
            <span class="theme-popover__title-dot"></span>
            {{ t(store.settings.locale, 'settings.appearance') }}
          </div>
          <div class="theme-mode-row grid grid-cols-3 gap-1.5">
            <button
              v-for="item in appearanceOptions"
              :key="item.id"
              class="theme-mode-pill rounded-[15px] px-2 py-1.5 transition-all"
              :class="store.settings.theme.appearance === item.id ? 'theme-mode-pill--active' : 'theme-mode-pill--idle'"
              @click="setAppearance(item.id)"
            >
              <component :is="item.icon" class="h-[13px] w-[13px] shrink-0" />
              <div class="theme-mode-pill__label text-[11px] font-semibold leading-none">
                {{ t(store.settings.locale, `settings.appearance${item.id.charAt(0).toUpperCase() + item.id.slice(1)}`) }}
              </div>
            </button>
          </div>
          <p v-if="store.settings.theme.appearance === 'system'" class="mt-1.5 px-1 text-[9.5px] leading-4 text-[var(--theme-text-tertiary)]">
            {{ systemPaletteHint }}
          </p>
        </div>

        <div class="theme-popover__divider"></div>

        <div class="theme-popover__section">
          <div class="theme-popover__section-title">
            <span class="theme-popover__title-dot"></span>
            {{ t(store.settings.locale, 'settings.themePickerLightPalette') }}
          </div>

          <div class="theme-popover__group-label">{{ t(store.settings.locale, 'settings.themePickerLight') }}</div>
          <button
            v-for="palette in lightGroups.light"
            :key="palette.id"
            class="theme-palette-option"
            :class="store.settings.theme.lightPalette === palette.id ? 'theme-palette-option--active' : 'theme-palette-option--idle'"
            @click="setLightPalette(palette.id as ThemeLightPalette)"
          >
            <div class="theme-palette-option__swatches">
              <span
                v-for="(swatch, index) in palette.preview"
                :key="`${palette.id}-${index}`"
                class="theme-palette-option__swatch"
                :style="{ backgroundColor: swatch }"
              ></span>
            </div>
            <div class="min-w-0 flex-1">
              <div class="theme-palette-option__title">{{ t(store.settings.locale, palette.key) }}</div>
            </div>
            <span class="theme-palette-option__tail">
              <Check v-if="store.settings.theme.lightPalette === palette.id" class="h-4 w-4 text-[var(--theme-accent-primary)]" />
            </span>
          </button>

          <div class="theme-popover__group-label mt-1.5">{{ t(store.settings.locale, 'settings.themePickerNature') }}</div>
          <button
            v-for="palette in lightGroups.nature"
            :key="palette.id"
            class="theme-palette-option"
            :class="store.settings.theme.lightPalette === palette.id ? 'theme-palette-option--active' : 'theme-palette-option--idle'"
            @click="setLightPalette(palette.id as ThemeLightPalette)"
          >
            <div class="theme-palette-option__swatches">
              <span
                v-for="(swatch, index) in palette.preview"
                :key="`${palette.id}-${index}`"
                class="theme-palette-option__swatch"
                :style="{ backgroundColor: swatch }"
              ></span>
            </div>
            <div class="min-w-0 flex-1">
              <div class="theme-palette-option__title">{{ t(store.settings.locale, palette.key) }}</div>
            </div>
            <span class="theme-palette-option__tail">
              <Check v-if="store.settings.theme.lightPalette === palette.id" class="h-4 w-4 text-[var(--theme-accent-primary)]" />
            </span>
          </button>

          <div class="theme-popover__group-label mt-1.5">{{ t(store.settings.locale, 'settings.themePickerWarm') }}</div>
          <button
            v-for="palette in lightGroups.warm"
            :key="palette.id"
            class="theme-palette-option"
            :class="store.settings.theme.lightPalette === palette.id ? 'theme-palette-option--active' : 'theme-palette-option--idle'"
            @click="setLightPalette(palette.id as ThemeLightPalette)"
          >
            <div class="theme-palette-option__swatches">
              <span
                v-for="(swatch, index) in palette.preview"
                :key="`${palette.id}-${index}`"
                class="theme-palette-option__swatch"
                :style="{ backgroundColor: swatch }"
              ></span>
            </div>
            <div class="min-w-0 flex-1">
              <div class="theme-palette-option__title">{{ t(store.settings.locale, palette.key) }}</div>
            </div>
            <span class="theme-palette-option__tail">
              <Check v-if="store.settings.theme.lightPalette === palette.id" class="h-4 w-4 text-[var(--theme-accent-primary)]" />
            </span>
          </button>
        </div>

        <div class="theme-popover__divider"></div>

        <div class="theme-popover__section">
          <div class="theme-popover__section-title">
            <span class="theme-popover__title-dot"></span>
            {{ t(store.settings.locale, 'settings.themePickerDarkPalette') }}
          </div>
          <button
            v-for="palette in DARK_THEME_PALETTE_OPTIONS"
            :key="palette.id"
            class="theme-palette-option"
            :class="store.settings.theme.darkPalette === palette.id ? 'theme-palette-option--active' : 'theme-palette-option--idle'"
            @click="setDarkPalette(palette.id as ThemeDarkPalette)"
          >
            <div class="theme-palette-option__swatches">
              <span
                v-for="(swatch, index) in palette.preview"
                :key="`${palette.id}-${index}`"
                class="theme-palette-option__swatch"
                :style="{ backgroundColor: swatch }"
              ></span>
            </div>
            <div class="min-w-0 flex-1">
              <div class="theme-palette-option__title">{{ t(store.settings.locale, palette.key) }}</div>
            </div>
            <span class="theme-palette-option__tail">
              <Check v-if="store.settings.theme.darkPalette === palette.id" class="h-4 w-4 text-[var(--theme-accent-primary)]" />
            </span>
          </button>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.theme-popover {
  background: var(--theme-overlay-gradient);
  border-color: var(--theme-border-default);
}

.theme-popover__section-title {
  display: flex;
  align-items: center;
  gap: 0.45rem;
  padding: 0 0.25rem;
  margin-bottom: 0.35rem;
  font-size: 10px;
  font-weight: 700;
  color: var(--theme-text-tertiary);
  letter-spacing: 0.06em;
}

.theme-popover__title-dot {
  height: 6px;
  width: 6px;
  flex: none;
  border-radius: 999px;
  background: var(--theme-accent-primary);
  box-shadow: 0 0 0 4px color-mix(in srgb, var(--theme-accent-soft) 38%, transparent 62%);
}

.theme-popover__divider {
  margin: 0.45rem 0 0.5rem;
  height: 1px;
  background: color-mix(in srgb, var(--theme-border-default) 46%, white 54%);
}

.theme-popover__group-label {
  padding: 0 0.25rem;
  margin-bottom: 0.2rem;
  font-size: 9px;
  font-weight: 700;
  color: var(--theme-text-quaternary);
  letter-spacing: 0.1em;
  text-transform: uppercase;
}

.theme-mode-pill {
  display: flex;
  min-height: 32px;
  align-items: center;
  justify-content: center;
  gap: 0.28rem;
  border: 1px solid var(--theme-border-default);
  text-align: center;
}

.theme-mode-pill__label {
  white-space: nowrap;
  letter-spacing: 0;
}

.theme-mode-pill--active {
  background:
    linear-gradient(145deg, color-mix(in srgb, var(--theme-accent-soft) 44%, white 56%), color-mix(in srgb, var(--theme-accent-soft) 14%, transparent 86%));
  color: var(--theme-accent-primary);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.36),
    inset 0 0 0 1px color-mix(in srgb, var(--theme-accent-primary) 8%, transparent 92%);
}

.theme-mode-pill--idle {
  background: var(--theme-surface-gradient);
  color: var(--theme-text-secondary);
}

.theme-mode-pill--idle:hover {
  background: var(--theme-surface-muted-gradient);
}

.theme-palette-option {
  display: flex;
  width: 100%;
  align-items: center;
  gap: 0.52rem;
  border-radius: 14px;
  padding: 0.38rem 0.5rem;
  transition: background-color 120ms ease, border-color 120ms ease;
}

.theme-palette-option--active {
  background:
    linear-gradient(145deg, color-mix(in srgb, var(--theme-accent-soft) 28%, white 72%), color-mix(in srgb, var(--theme-accent-soft) 10%, transparent 90%));
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.34),
    inset 0 0 0 1px color-mix(in srgb, var(--theme-accent-primary) 6%, transparent 94%);
}

.theme-palette-option--idle:hover {
  background: var(--theme-surface-muted-gradient);
}

.theme-palette-option__swatches {
  display: flex;
  min-width: 50px;
}

.theme-palette-option__swatch {
  margin-left: -8px;
  height: 18px;
  width: 18px;
  border-radius: 999px;
  border: 2px solid color-mix(in srgb, var(--theme-bg-overlay) 82%, white 18%);
  box-shadow: 0 1px 6px rgba(15, 23, 42, 0.05);
}

.theme-palette-option__swatch:first-child {
  margin-left: 0;
}

.theme-palette-option__title {
  font-size: 11.5px;
  font-weight: 700;
  color: var(--theme-text-primary);
  letter-spacing: 0.01em;
}

.theme-palette-option__tail {
  display: inline-flex;
  width: 14px;
  flex: none;
  align-items: center;
  justify-content: center;
}
</style>
