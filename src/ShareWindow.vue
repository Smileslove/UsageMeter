<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { toBlob, toJpeg, toPng } from 'html-to-image'
import { Check, Copy, Download, Loader2, X } from 'lucide-vue-next'
import { sourceLabel, t } from './i18n'
import { useMonitorStore } from './stores/monitor'
import {
  DEEPSEEK_HARNESS_ACCOUNT_SOURCE_ID,
  OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID,
  OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID,
  OFFICIAL_OPENAI_OAUTH_SOURCE_ID,
  type AppLocale,
  type StatisticsBucket,
  type StatisticsRangePreset,
  type StatisticsSummary
} from './types'
import ShareUsageCard from './components/statistics/ShareUsageCard.vue'
import { SHARE_THEMES } from './components/statistics/shareThemes'
import { formatToolDisplayName } from './utils/toolDisplay'
import { getStatisticsSummary } from './api/usageApi'
import { applyResolvedTheme } from './theme'

type SharePreset = Exclude<StatisticsRangePreset, 'custom'> | '1y'

const store = useMonitorStore()

const preset = ref<SharePreset>('today')
const displayName = ref('')
const includeStats = ref(true)
const includeTrend = ref(true)
const includeModels = ref(true)
const exportFormat = ref<'png' | 'jpeg'>('png')
const themeId = ref<string>(SHARE_THEMES[0].id)
const summary = ref<StatisticsSummary | null>(null)
const loading = ref(false)
const exporting = ref(false)
const status = ref<'idle' | 'copied' | 'copyFailed' | 'saveFailed' | 'exportFailed' | 'loadFailed'>('idle')
const captureRef = ref<HTMLElement | null>(null)
const viewportWidth = ref(typeof window === 'undefined' ? 1180 : window.innerWidth)
const viewportHeight = ref(typeof window === 'undefined' ? 760 : window.innerHeight)
let mediaQuery: MediaQueryList | null = null
let fetchId = 0
let fetchTimer: ReturnType<typeof setTimeout> | null = null

const cardWidth = 1200
const cardHeight = 1760
const previewScale = computed(() => {
  const panelWidth = viewportWidth.value <= 1100 ? 300 : 340
  const availableWidth = Math.max(160, viewportWidth.value - panelWidth - 64)
  const availableHeight = Math.max(200, viewportHeight.value - 200)
  return Math.min(0.65, availableWidth / cardWidth, availableHeight / cardHeight)
})

const locale = computed<AppLocale>(() => store.settings.locale)
const dayBoundaryHour = computed(() => store.settings.dayBoundaryMode === 'night_owl' ? 4 : 0)
const activeTheme = computed(() => SHARE_THEMES.find(theme => theme.id === themeId.value) ?? SHARE_THEMES[0])
const themes = computed(() => SHARE_THEMES.filter(theme => theme.appearance === activeTheme.value.appearance))

function selectThemeAppearance(appearance: 'dark' | 'light') {
  if (activeTheme.value.appearance === appearance) return
  themeId.value = SHARE_THEMES.find(theme => theme.appearance === appearance)?.id ?? SHARE_THEMES[0].id
}

const panelThemeVars = computed(() => ({
  '--share-selection': activeTheme.value.appearance === 'light' ? activeTheme.value.foreground : activeTheme.value.background,
  '--share-selection-ink': activeTheme.value.appearance === 'light' ? activeTheme.value.background : activeTheme.value.ink
}))
const contentOptions = [
  { key: 'statistics.shareIncludeStats', value: includeStats },
  { key: 'statistics.shareIncludeTrend', value: includeTrend },
  { key: 'statistics.shareIncludeModels', value: includeModels }
]
const posterProps = computed(() => ({
  locale: locale.value,
  summary: summary.value,
  currency: store.settings.currency,
  rangeLabel: rangeLabel.value,
  scopeLabel: scopeLabel.value,
  generatedAtLabel: generatedAtLabel.value,
  displayName: displayName.value,
  theme: themeId.value,
  visual: visualMode.value,
  calendarBounds: calendarBounds.value,
  includeStats: includeStats.value,
  includeTrend: includeTrend.value,
  includeModels: includeModels.value
}))

const rangeOptions: Array<{ value: SharePreset; key: string }> = [
  { value: '5h', key: 'statistics.range5h' },
  { value: 'today', key: 'statistics.rangeToday' },
  { value: '1d', key: 'statistics.range1d' },
  { value: '7d', key: 'statistics.range7d' },
  { value: '30d', key: 'statistics.range30d' },
  { value: 'current_month', key: 'statistics.rangeMonth' },
  { value: '1y', key: 'statistics.range1y' }
]

function startOfBusinessDay(date: Date): Date {
  const candidate = new Date(date.getFullYear(), date.getMonth(), date.getDate(), dayBoundaryHour.value, 0, 0, 0)
  if (date.getHours() < dayBoundaryHour.value) {
    candidate.setDate(candidate.getDate() - 1)
  }
  return candidate
}

function businessDayStartFromLabel(dateStr: string): Date {
  const [year, month, day] = dateStr.split('-').map(Number)
  return new Date(year, (month || 1) - 1, day || 1, dayBoundaryHour.value, 0, 0, 0)
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date)
  next.setDate(next.getDate() + days)
  return next
}

function presetRangeDates(value: SharePreset): { start: Date; end: Date } {
  const now = new Date()
  if (value === '5h') return { start: new Date(now.getTime() - 5 * 60 * 60 * 1000), end: now }
  if (value === 'today') return { start: startOfBusinessDay(now), end: now }
  if (value === '1d') return { start: new Date(now.getTime() - 24 * 60 * 60 * 1000), end: now }
  if (value === '7d') return { start: addDays(startOfBusinessDay(now), -6), end: now }
  if (value === '30d') return { start: addDays(startOfBusinessDay(now), -29), end: now }
  if (value === 'current_month') return { start: businessDayStartFromLabel(`${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-01`), end: now }
  if (value === '1y') return { start: addDays(startOfBusinessDay(now), -364), end: now }
  return { start: startOfBusinessDay(now), end: now }
}

const range = computed(() => {
  const { start, end } = presetRangeDates(preset.value)
  return { start: Math.floor(start.getTime() / 1000), end: Math.floor(end.getTime() / 1000) }
})

const bucket = computed<StatisticsBucket>(() => {
  const hours = (range.value.end - range.value.start) / 3600
  return hours <= 48 ? 'hour' : 'day'
})

// Month-scale ranges use a calendar heatmap, a full year uses the contribution grid,
// and shorter spans fall back to the line chart.
const visualMode = computed<'chart' | 'calendar' | 'year'>(() => {
  if (preset.value === '1y') return 'year'
  if (preset.value === '30d' || preset.value === 'current_month') return 'calendar'
  return 'chart'
})

// The calendar grid spans these days. "本月" fills the entire month (future days shown
// as empty cells); the rolling 30-day window just spans its own range.
const calendarBounds = computed<{ start: number; end: number } | null>(() => {
  if (visualMode.value !== 'calendar') return null
  const now = new Date()
  if (preset.value === 'current_month') {
    const start = businessDayStartFromLabel(`${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-01`)
    const end = addDays(businessDayStartFromLabel(`${now.getFullYear()}-${String(now.getMonth() + 2).padStart(2, '0')}-01`), -1)
    return { start: Math.floor(start.getTime() / 1000), end: Math.floor(end.getTime() / 1000) }
  }
  return { start: range.value.start, end: range.value.end }
})

const rangeLabel = computed(() => {
  const format = new Intl.DateTimeFormat(locale.value, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
  return `${format.format(new Date(range.value.start * 1000))} - ${format.format(new Date(range.value.end * 1000))}`
})

const generatedAtLabel = computed(() => {
  const epoch = summary.value?.generatedAtEpoch ?? Math.floor(Date.now() / 1000)
  return new Intl.DateTimeFormat(locale.value, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  }).format(new Date(epoch * 1000))
})

function sourceDisplayName(sourceId: string): string | null {
  if (sourceId === '__unknown__') return t(locale.value, 'sources.unknown')
  if (sourceId === DEEPSEEK_HARNESS_ACCOUNT_SOURCE_ID) return sourceLabel(locale.value, sourceId)
  if (sourceId === OFFICIAL_OPENAI_OAUTH_SOURCE_ID) return sourceLabel(locale.value, sourceId)
  if (sourceId === OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID) return sourceLabel(locale.value, sourceId)
  if (sourceId === OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID) return sourceLabel(locale.value, sourceId)
  const source = store.settings.sourceAware.sources.find(item => item.id === sourceId)
  if (!source) return null
  if (source.displayName) return source.displayName
  if (source.baseUrl) {
    try {
      return new URL(source.baseUrl).hostname
    } catch {
      return source.baseUrl
    }
  }
  return null
}

function toolDisplayName(tool: string): string {
  return formatToolDisplayName(tool, locale.value, store.settings.clientTools.profiles)
}

const scopeLabel = computed(() => {
  const labels: string[] = []
  const sourceFilter = store.settings.sourceAware.activeSourceFilter
  const toolFilter = store.settings.clientTools.activeToolFilter
  if (sourceFilter) {
    const sourceName = sourceDisplayName(sourceFilter)
    if (sourceName) labels.push(sourceName)
  }
  if (toolFilter) labels.push(toolDisplayName(toolFilter))
  return labels.length ? labels.join(' / ') : t(locale.value, 'statistics.shareScopeAll')
})

const statusLabel = computed(() => {
  if (status.value === 'loadFailed') return t(locale.value, 'statistics.shareLoadFailed')
  if (status.value === 'copied') return t(locale.value, 'statistics.shareCopied')
  if (status.value === 'copyFailed') return t(locale.value, 'statistics.shareCopyFailed')
  if (status.value === 'saveFailed') return t(locale.value, 'statistics.shareSaveFailed')
  if (status.value === 'exportFailed') return t(locale.value, 'statistics.shareExportFailed')
  return ''
})

async function fetchSummary() {
  const id = ++fetchId
  loading.value = true
  status.value = 'idle'
  try {
    const result = await getStatisticsSummary({
      startEpoch: range.value.start,
      endEpoch: range.value.end,
      timezone: store.settings.timezone,
      bucket: bucket.value,
      metric: 'tokens'
    }, store.settings)
    if (id === fetchId) summary.value = result
  } catch {
    if (id === fetchId) {
      summary.value = null
      status.value = 'loadFailed'
    }
  } finally {
    if (id === fetchId) loading.value = false
  }
}

function scheduleFetchSummary() {
  fetchId += 1
  loading.value = true
  status.value = 'idle'
  if (fetchTimer) clearTimeout(fetchTimer)
  fetchTimer = setTimeout(() => {
    fetchTimer = null
    void fetchSummary()
  }, 120)
}

function updateViewportSize() {
  viewportWidth.value = window.innerWidth
  viewportHeight.value = window.innerHeight
}

async function waitForPaint() {
  if (document.fonts?.ready) {
    try {
      await document.fonts.ready
    } catch {
      // Continue with fallback fonts.
    }
  }
  await new Promise(resolve => requestAnimationFrame(() => resolve(null)))
}

async function renderBlob(format: 'png' | 'jpeg' = 'png'): Promise<Blob> {
  await nextTick()
  await waitForPaint()
  const node = captureRef.value?.querySelector('.share-card') as HTMLElement | null
  if (!node) throw new Error('missing-share-card')

  const options = {
    pixelRatio: 2,
    cacheBust: true,
    backgroundColor: activeTheme.value.background,
    quality: 0.95,
    width: cardWidth,
    height: cardHeight,
    style: {
      width: `${cardWidth}px`,
      height: `${cardHeight}px`
    }
  }

  if (format === 'jpeg') {
    const response = await fetch(await toJpeg(node, options))
    return response.blob()
  }

  try {
    const blob = await toBlob(node, options)
    if (blob) return blob
  } catch {
    // Fall back to data URL path.
  }

  const dataUrl = await toPng(node, options)
  const response = await fetch(dataUrl)
  return response.blob()
}

function downloadFileName(format: 'png' | 'jpeg'): string {
  const date = new Date().toISOString().slice(0, 10)
  return `usagemeter-share-${date}.${format === 'jpeg' ? 'jpg' : 'png'}`
}

async function copyImage() {
  exporting.value = true
  status.value = 'idle'
  try {
    const blob = await renderBlob()
    const ClipboardItemCtor = (window as typeof window & {
      ClipboardItem?: new (items: Record<string, Blob>) => ClipboardItem
    }).ClipboardItem
    if (!navigator.clipboard?.write || !ClipboardItemCtor) throw new Error('clipboard-image-not-supported')
    await navigator.clipboard.write([new ClipboardItemCtor({ 'image/png': blob })])
    status.value = 'copied'
  } catch {
    status.value = 'copyFailed'
  } finally {
    exporting.value = false
  }
}

async function saveImage() {
  exporting.value = true
  status.value = 'idle'
  try {
    const format = exportFormat.value
    const blob = await renderBlob(format)
    const url = URL.createObjectURL(blob)
    try {
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = downloadFileName(format)
      anchor.click()
    } finally {
      URL.revokeObjectURL(url)
    }
  } catch {
    status.value = 'saveFailed'
  } finally {
    exporting.value = false
  }
}

async function closeWindow() {
  await getCurrentWindow().close()
}

watch([preset], scheduleFetchSummary)
watch(() => store.settings.theme, applyResolvedTheme, { deep: true })

function handleSystemThemeChange() {
  if (store.settings.theme.appearance === 'system') applyResolvedTheme(store.settings.theme)
}

onMounted(async () => {
  updateViewportSize()
  window.addEventListener('resize', updateViewportSize)
  mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  mediaQuery.addEventListener('change', handleSystemThemeChange)
  await store.loadSettings()
  applyResolvedTheme(store.settings.theme)
  await fetchSummary()
})

onUnmounted(() => {
  window.removeEventListener('resize', updateViewportSize)
  mediaQuery?.removeEventListener('change', handleSystemThemeChange)
  fetchId += 1
  if (fetchTimer) clearTimeout(fetchTimer)
})
</script>

<template>
  <main class="share-window" :style="panelThemeVars">
    <header class="share-window__header" data-tauri-drag-region>
      <div data-tauri-drag-region>
        <h1 data-tauri-drag-region>{{ t(locale, 'statistics.shareTitle') }}</h1>
        <p data-tauri-drag-region>{{ t(locale, 'statistics.shareEditorHint') }}</p>
      </div>
      <button type="button" class="share-window__close" :aria-label="t(locale, 'common.close')" @click="closeWindow">
        <X class="h-4 w-4" />
      </button>
    </header>

    <div class="share-window__chrome">
      <div ref="captureRef" class="share-window__capture" aria-hidden="true">
        <ShareUsageCard v-bind="posterProps" />
      </div>

      <section class="share-window__preview" :aria-label="t(locale, 'statistics.sharePreview')" :aria-busy="loading">
        <div class="share-window__preview-head">
          <span>{{ t(locale, 'statistics.sharePreview') }}</span>
          <span class="share-window__scope" :title="scopeLabel">{{ scopeLabel }}</span>
        </div>
        <div
          class="share-window__preview-frame"
          :style="{ width: `${cardWidth * previewScale}px`, height: `${cardHeight * previewScale}px` }"
        >
          <div
            class="share-window__card-scale"
            :style="{ width: `${cardWidth}px`, height: `${cardHeight}px`, transform: `scale(${previewScale})` }"
          >
            <ShareUsageCard v-bind="posterProps" />
          </div>
          <div v-if="loading" class="share-window__loading">
            <Loader2 class="h-6 w-6 animate-spin" :aria-label="t(locale, 'statistics.sharePreparing')" />
          </div>
        </div>
        <span class="share-window__preview-scale">{{ t(locale, 'statistics.sharePreviewScale', { percent: Math.round(previewScale * 100) }) }}</span>
      </section>

      <aside class="share-window__panel">
        <div class="share-window__settings">
          <div class="share-window__title-block">
            <h2>{{ t(locale, 'statistics.shareSettings') }}</h2>
            <p>{{ t(locale, 'statistics.shareSettingsHint') }}</p>
          </div>

          <fieldset :disabled="exporting" class="share-window__fields">
            <section class="share-window__group">
              <label for="share-display-name">{{ t(locale, 'statistics.shareDisplayName') }}</label>
              <input id="share-display-name" v-model="displayName" type="text" maxlength="80" :placeholder="t(locale, 'statistics.shareDisplayNamePlaceholder')" />
            </section>

            <section class="share-window__group" role="group" :aria-label="t(locale, 'statistics.shareTimeRange')">
              <h3>{{ t(locale, 'statistics.shareTimeRange') }}</h3>
              <div class="share-window__range-grid">
                <button
                  v-for="item in rangeOptions"
                  :key="item.value"
                  type="button"
                  :aria-pressed="preset === item.value"
                  :class="{ 'share-window__choice--active': preset === item.value }"
                  @click="preset = item.value"
                >
                  {{ t(locale, item.key) }}
                </button>
              </div>
            </section>

            <section class="share-window__group" role="group" :aria-label="t(locale, 'statistics.shareTheme')">
              <div class="share-window__theme-head">
                <h3>{{ t(locale, 'statistics.shareTheme') }}</h3>
                <div class="share-window__theme-appearances">
                  <button
                    v-for="appearance in (['dark', 'light'] as const)"
                    :key="appearance"
                    type="button"
                    :aria-pressed="activeTheme.appearance === appearance"
                    :class="{ 'share-window__choice--active': activeTheme.appearance === appearance }"
                    @click="selectThemeAppearance(appearance)"
                  >{{ t(locale, appearance === 'dark' ? 'statistics.shareThemeDark' : 'statistics.shareThemeLight') }}</button>
                </div>
              </div>
              <div class="share-window__themes">
                <button
                  v-for="theme in themes"
                  :key="theme.id"
                  type="button"
                  class="share-window__swatch"
                  :class="{ 'share-window__swatch--active': themeId === theme.id }"
                  :style="{ '--swatch': theme.swatch }"
                  :title="t(locale, theme.labelKey)"
                  :aria-label="t(locale, theme.labelKey)"
                  :aria-pressed="themeId === theme.id"
                  @click="themeId = theme.id"
                >
                  <span></span>
                </button>
              </div>
            </section>

            <section class="share-window__group share-window__group--ruled">
              <h3>{{ t(locale, 'statistics.shareContent') }}</h3>
              <label v-for="item in contentOptions" :key="item.key" class="share-window__toggle-row">
                <span>{{ t(locale, item.key) }}</span>
                <input v-model="item.value.value" type="checkbox" role="switch" />
                <span class="share-window__toggle" aria-hidden="true"></span>
              </label>
            </section>

            <section class="share-window__group share-window__group--ruled" role="group" :aria-label="t(locale, 'statistics.shareExportFormat')">
              <h3>{{ t(locale, 'statistics.shareExportFormat') }}</h3>
              <div class="share-window__format-grid">
                <button
                  v-for="format in (['png', 'jpeg'] as const)"
                  :key="format"
                  type="button"
                  :aria-pressed="exportFormat === format"
                  :class="{ 'share-window__choice--active': exportFormat === format }"
                  @click="exportFormat = format"
                >{{ t(locale, format === 'png' ? 'statistics.shareFormatPng' : 'statistics.shareFormatJpeg') }}</button>
              </div>
            </section>
          </fieldset>
        </div>

        <section class="share-window__actions" :aria-label="t(locale, 'statistics.shareActions')">
          <span class="share-window__export-detail">{{ t(locale, 'statistics.shareExportQuality') }}</span>
          <button type="button" class="share-window__primary" :disabled="exporting || loading || !summary" @click="saveImage">
            <Loader2 v-if="exporting" class="h-4 w-4 animate-spin" />
            <Download v-else class="h-4 w-4" />
            <span>{{ t(locale, 'statistics.shareSaveImage') }}</span>
          </button>
          <button type="button" :disabled="exporting || loading || !summary" @click="copyImage">
            <Copy class="h-4 w-4" />
            <span>{{ t(locale, 'statistics.shareCopyImage') }}</span>
          </button>
          <p v-if="exporting || statusLabel" role="status" :class="{ 'share-window__status--ok': status === 'copied', 'share-window__status--neutral': exporting }">
            <Check v-if="status === 'copied'" class="h-4 w-4 shrink-0" />
            <span>{{ exporting ? t(locale, 'statistics.sharePreparing') : statusLabel }}</span>
            <button v-if="status === 'loadFailed'" type="button" class="share-window__retry" @click="fetchSummary">{{ t(locale, 'desktop.activity.retry') }}</button>
          </p>
        </section>
      </aside>
    </div>
  </main>
</template>

<style scoped>
.share-window {
  --share-workspace: #f3f1ec;
  --share-surface: #fbfaf7;
  --share-text: #292e2b;
  --share-muted: #70756e;
  --share-border: #dedfd8;
  --share-hover: #eeeee7;
  --share-selected-bg: #e5eae2;
  --share-selected-text: #24483e;
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
  color: var(--share-text);
  background: var(--share-workspace);
}

:global(.dark .share-window) {
  --share-workspace: #202522;
  --share-surface: #272d29;
  --share-text: #eeeee6;
  --share-muted: #b3baaf;
  --share-border: #414a43;
  --share-hover: #343d35;
  --share-selected-bg: #3d4b40;
  --share-selected-text: #e5eadc;
}

.share-window__header {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  height: 88px;
  padding: 26px 30px 12px;
  border-bottom: 1px solid var(--share-border);
  background: var(--share-surface);
}

.share-window h1,
.share-window h2,
.share-window h3,
.share-window p {
  margin: 0;
}

.share-window h1 {
  font-size: 22px;
  line-height: 1.2;
  font-weight: 500;
  letter-spacing: 0.04em;
}

.share-window__header p,
.share-window__title-block p {
  margin-top: 5px;
  color: var(--share-muted);
  font-size: 12px;
  line-height: 1.5;
}

.share-window__close {
  display: grid;
  flex: 0 0 auto;
  width: 32px;
  height: 32px;
  place-items: center;
  border-radius: 6px;
  color: var(--share-muted);
}

.share-window__close:hover {
  color: var(--share-text);
  background: var(--share-hover);
}

.share-window__chrome {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 340px;
  flex: 1;
  min-height: 0;
}

.share-window__preview {
  display: grid;
  grid-template-rows: 20px minmax(0, 1fr) 24px;
  place-items: center;
  gap: 14px;
  min-width: 0;
  min-height: 0;
  padding: 20px 32px;
}

.share-window__preview-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  width: 100%;
  color: var(--share-muted);
  font-size: 11px;
}

.share-window__scope {
  overflow: hidden;
  max-width: 65%;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-window__preview-frame {
  position: relative;
  overflow: hidden;
  border-radius: 4px;
  box-shadow: 0 12px 28px rgb(22 36 26 / 16%), 0 2px 6px rgb(22 36 26 / 8%);
}

.share-window__card-scale {
  transform-origin: top left;
}

.share-window__preview-scale {
  padding: 4px 12px;
  border: 1px solid var(--share-border);
  border-radius: 20px;
  color: var(--share-muted);
  background: var(--share-surface);
  font-size: 10px;
  line-height: 1.4;
  font-variant-numeric: tabular-nums;
}

.share-window__loading {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  color: #f3efdf;
  background: rgb(15 35 27 / 45%);
}

.share-window__panel {
  display: flex;
  flex-direction: column;
  min-height: 0;
  border-left: 1px solid var(--share-border);
  background: var(--share-surface);
}

.share-window__settings {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 20px 26px;
  scrollbar-width: thin;
}

.share-window__title-block {
  margin-bottom: 20px;
}

.share-window h2 {
  font-size: 17px;
  font-weight: 550;
  letter-spacing: 0.025em;
}

.share-window__fields {
  display: grid;
  gap: 20px;
  min-width: 0;
  margin: 0;
  padding: 0;
  border: 0;
}

.share-window__group {
  display: grid;
  gap: 10px;
  min-width: 0;
}

.share-window__group h3,
.share-window__group > label:not(.share-window__toggle-row) {
  font-size: 12px;
  font-weight: 500;
}

.share-window__group--ruled {
  padding-top: 16px;
  border-top: 1px solid var(--share-border);
}

.share-window input[type='text'] {
  width: 100%;
  height: 36px;
  padding: 0 12px;
  border: 1px solid var(--share-border);
  border-radius: 5px;
  color: var(--share-text);
  background: transparent;
  font-size: 12px;
}

.share-window input::placeholder {
  color: var(--share-muted);
}

.share-window__range-grid,
.share-window__format-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 6px;
}

.share-window__format-grid {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

.share-window__range-grid button,
.share-window__format-grid button {
  min-width: 0;
  min-height: 34px;
  padding: 6px 4px;
  border: 1px solid var(--share-border);
  border-radius: 5px;
  color: var(--share-text);
  background: transparent;
  font-size: 12px;
  line-height: 1.3;
}

.share-window__range-grid button:hover,
.share-window__format-grid button:hover {
  background: var(--share-hover);
}

.share-window button.share-window__choice--active {
  border-color: var(--share-muted);
  color: var(--share-selected-text);
  background: var(--share-selected-bg);
}

.share-window__theme-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.share-window__theme-appearances {
  display: flex;
  gap: 2px;
  padding: 2px;
  border: 1px solid var(--share-border);
  border-radius: 5px;
}

.share-window__theme-appearances button {
  padding: 2px 8px;
  border-radius: 3px;
  color: var(--share-muted);
  font-size: 10px;
  line-height: 1.4;
}

.share-window__themes {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-left: -4px;
}

.share-window__swatch {
  display: grid;
  width: 36px;
  height: 36px;
  place-items: center;
  border: 1px solid transparent;
  border-radius: 50%;
}

.share-window__swatch span {
  width: 26px;
  height: 26px;
  border-radius: 50%;
  background: var(--swatch);
  box-shadow: inset 0 0 0 1px rgb(0 0 0 / 8%);
}

.share-window__swatch--active {
  border-color: var(--share-text);
}

.share-window__swatch:hover {
  background: var(--share-hover);
}

.share-window__toggle-row {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  min-height: 24px;
  cursor: pointer;
  font-size: 12px;
}

.share-window__toggle-row input {
  position: absolute;
  right: 0;
  width: 32px;
  height: 28px;
  opacity: 0;
}

.share-window__toggle {
  display: flex;
  align-items: center;
  width: 32px;
  height: 18px;
  padding: 2px;
  border-radius: 20px;
  background: var(--share-border);
  transition: background 160ms ease;
}

.share-window__toggle::after {
  content: '';
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: #fbfaf7;
  box-shadow: 0 1px 3px rgb(0 0 0 / 12%);
  transition: transform 160ms ease;
}

.share-window__toggle-row input:checked + .share-window__toggle {
  background: var(--share-selection);
}

:global(.dark .share-window__toggle-row input:checked + .share-window__toggle) {
  background: var(--share-selection-ink);
}

:global(.dark .share-window__toggle-row input:checked + .share-window__toggle::after) {
  background: var(--share-surface);
}

.share-window__toggle-row input:checked + .share-window__toggle::after {
  transform: translateX(14px);
}

.share-window__toggle-row input:focus-visible + .share-window__toggle {
  outline: 2px solid var(--share-text);
  outline-offset: 3px;
}

.share-window__actions {
  display: grid;
  flex: 0 0 auto;
  gap: 8px;
  padding: 16px 26px 20px;
  border-top: 1px solid var(--share-border);
}

.share-window__export-detail {
  margin-bottom: 2px;
  color: var(--share-muted);
  font-size: 10px;
}

.share-window__actions > button {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  height: 36px;
  padding: 0 12px;
  border: 1px solid var(--share-border);
  border-radius: 5px;
  color: var(--share-text);
  background: transparent;
  font-size: 12px;
  font-weight: 500;
}

.share-window__actions > button:hover {
  background: var(--share-hover);
}

.share-window__actions > .share-window__primary {
  border-color: transparent;
  color: #f3efdf;
  background: var(--share-selection);
}

.share-window__actions > .share-window__primary:hover {
  filter: brightness(1.12);
  background: var(--share-selection);
}

.share-window__actions p {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--theme-status-danger-fg);
  font-size: 11px;
  line-height: 1.5;
}

.share-window__actions p.share-window__status--ok {
  color: var(--theme-status-success-fg);
}

.share-window__actions p.share-window__status--neutral {
  color: var(--share-muted);
}

.share-window__retry {
  margin-left: auto;
  flex-shrink: 0;
  text-decoration: underline;
}

.share-window button {
  cursor: pointer;
  transition: background 160ms ease, color 160ms ease;
}

.share-window button:focus-visible,
.share-window input[type='text']:focus-visible {
  outline: 2px solid var(--share-text);
  outline-offset: 3px;
}

.share-window button:disabled,
.share-window__fields:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.share-window__capture {
  position: fixed;
  left: 0;
  top: 0;
  z-index: -1;
  width: 1200px;
  height: 1760px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  opacity: 0;
  pointer-events: none;
}

@media (max-width: 1100px) {
  .share-window__chrome { grid-template-columns: minmax(0, 1fr) 300px; }
  .share-window__settings { padding: 20px; }
  .share-window__fields { gap: 18px; }
  .share-window__title-block { margin-bottom: 20px; }
  .share-window__actions { padding-inline: 20px; }
}

@media (max-height: 780px) {
  .share-window__settings { padding: 12px 20px; }
  .share-window__title-block { margin-bottom: 12px; }
  .share-window__title-block p { margin-top: 2px; font-size: 11px; }
  .share-window__fields { gap: 8px; }
  .share-window__group { gap: 6px; }
  .share-window__title-block h2 { line-height: 1.2; }
  .share-window__title-block p { line-height: 1.3; }
  .share-window__group h3,
  .share-window__group > label:not(.share-window__toggle-row) { font-size: 11px; line-height: 1.3; }
  .share-window input[type='text'] { height: 32px; }
  .share-window__swatch { width: 32px; height: 32px; }
  .share-window__swatch span { width: 24px; height: 24px; }
  .share-window__group--ruled { padding-top: 8px; }
  .share-window__range-grid button,
  .share-window__format-grid button { min-height: 28px; padding-block: 4px; font-size: 11px; }
  .share-window__range-grid { gap: 4px; }
  .share-window__toggle-row { min-height: 24px; }
  .share-window__actions { padding: 12px 20px; gap: 6px; }
  .share-window__actions > button { height: 32px; }
}

@media (prefers-reduced-motion: reduce) {
  .share-window *, .share-window *::after { transition: none; animation: none; }
}
</style>
