<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { toBlob, toPng } from 'html-to-image'
import { Check, Copy, Download, Loader2, Sparkles, X } from 'lucide-vue-next'
import { t } from './i18n'
import { useMonitorStore } from './stores/monitor'
import type { AppLocale, StatisticsBucket, StatisticsRangePreset, StatisticsSummary } from './types'
import ShareUsageCard from './components/statistics/ShareUsageCard.vue'
import { SHARE_THEMES } from './components/statistics/shareThemes'

type SharePreset = Exclude<StatisticsRangePreset, 'custom'> | '1y'

const store = useMonitorStore()

const preset = ref<SharePreset>('today')
const displayName = ref('')
const themeId = ref<string>(SHARE_THEMES[0].id)
const summary = ref<StatisticsSummary | null>(null)
const loading = ref(false)
const exporting = ref(false)
const status = ref<'idle' | 'copied' | 'copyFailed' | 'saveFailed' | 'exportFailed'>('idle')
const captureRef = ref<HTMLElement | null>(null)
const viewportWidth = ref(typeof window === 'undefined' ? 1180 : window.innerWidth)
const viewportHeight = ref(typeof window === 'undefined' ? 760 : window.innerHeight)
let fetchTimer: ReturnType<typeof setTimeout> | null = null

const cardWidth = 1200
const cardHeight = 1760
const previewScale = computed(() => {
  const panelWidth = 350
  const shellPadding = 24
  const previewPadding = 52
  const previewHeaderHeight = 74
  const availableWidth = Math.max(280, viewportWidth.value - panelWidth - shellPadding - previewPadding)
  const availableHeight = Math.max(360, viewportHeight.value - shellPadding - previewPadding - previewHeaderHeight)
  return Math.max(0.23, Math.min(0.34, availableWidth / cardWidth, availableHeight / cardHeight))
})

const locale = computed<AppLocale>(() => store.settings.locale)
const themes = SHARE_THEMES
const activeTheme = computed(() => SHARE_THEMES.find(theme => theme.id === themeId.value) ?? SHARE_THEMES[0])
const panelThemeVars = computed(() => ({
  '--share-grad-1': activeTheme.value.grad1,
  '--share-grad-2': activeTheme.value.grad2,
  '--share-accent': activeTheme.value.accent
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

function startOfLocalDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate(), 0, 0, 0, 0)
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date)
  next.setDate(next.getDate() + days)
  return next
}

function presetRangeDates(value: SharePreset): { start: Date; end: Date } {
  const now = new Date()
  if (value === '5h') return { start: new Date(now.getTime() - 5 * 60 * 60 * 1000), end: now }
  if (value === 'today') return { start: startOfLocalDay(now), end: now }
  if (value === '1d') return { start: new Date(now.getTime() - 24 * 60 * 60 * 1000), end: now }
  if (value === '7d') return { start: addDays(startOfLocalDay(now), -6), end: now }
  if (value === '30d') return { start: addDays(startOfLocalDay(now), -29), end: now }
  if (value === 'current_month') return { start: new Date(now.getFullYear(), now.getMonth(), 1), end: now }
  if (value === '1y') return { start: addDays(startOfLocalDay(now), -364), end: now }
  return { start: startOfLocalDay(now), end: now }
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
    const start = new Date(now.getFullYear(), now.getMonth(), 1)
    const end = new Date(now.getFullYear(), now.getMonth() + 1, 0) // last day of month
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
  const profile = store.settings.clientTools.profiles.find(item => item.tool === tool)
  return profile?.displayName || tool
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
  if (status.value === 'copied') return t(locale.value, 'statistics.shareCopied')
  if (status.value === 'copyFailed') return t(locale.value, 'statistics.shareCopyFailed')
  if (status.value === 'saveFailed') return t(locale.value, 'statistics.shareSaveFailed')
  if (status.value === 'exportFailed') return t(locale.value, 'statistics.shareExportFailed')
  return ''
})

async function fetchSummary() {
  loading.value = true
  try {
    summary.value = await invoke<StatisticsSummary>('get_statistics_summary', {
      query: {
        startEpoch: range.value.start,
        endEpoch: range.value.end,
        timezone: store.settings.timezone,
        bucket: bucket.value,
        metric: 'tokens'
      },
      settings: store.settings
    })
  } finally {
    loading.value = false
  }
}

function scheduleFetchSummary() {
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

async function renderBlob(): Promise<Blob> {
  await nextTick()
  await waitForPaint()
  const node = captureRef.value?.querySelector('.share-card') as HTMLElement | null
  if (!node) throw new Error('missing-share-card')

  const options = {
    pixelRatio: 2,
    cacheBust: true,
    width: cardWidth,
    height: cardHeight,
    style: {
      width: `${cardWidth}px`,
      height: `${cardHeight}px`
    }
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

function downloadFileName(): string {
  const date = new Date().toISOString().slice(0, 10)
  return `usagemeter-share-${date}.png`
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
    const blob = await renderBlob()
    const url = URL.createObjectURL(blob)
    try {
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = downloadFileName()
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

onMounted(async () => {
  updateViewportSize()
  window.addEventListener('resize', updateViewportSize)
  await store.loadSettings()
  await fetchSummary()
})

onUnmounted(() => {
  window.removeEventListener('resize', updateViewportSize)
  if (fetchTimer) clearTimeout(fetchTimer)
})
</script>

<template>
  <main class="share-window">
    <div class="share-window__chrome">
      <div ref="captureRef" class="share-window__capture" aria-hidden="true">
        <ShareUsageCard
          :locale="locale"
          :summary="summary"
          :currency="store.settings.currency"
          :range-label="rangeLabel"
          :scope-label="scopeLabel"
          :generated-at-label="generatedAtLabel"
          :display-name="displayName"
          :theme="themeId"
          :visual="visualMode"
          :calendar-bounds="calendarBounds"
          :include-trend="true"
          :include-models="true"
        />
      </div>

      <section class="share-window__preview">
        <div class="share-window__preview-head">
          <div>
            <span>{{ t(locale, 'statistics.sharePreview') }}</span>
            <strong>{{ displayName || scopeLabel }}</strong>
          </div>
          <p>{{ t(locale, 'statistics.sharePosterHint') }}</p>
        </div>
        <div
          class="share-window__preview-frame"
          :style="{ width: `${cardWidth * previewScale}px`, height: `${cardHeight * previewScale}px` }"
        >
          <div
            class="share-window__card-scale"
            :style="{ width: `${cardWidth}px`, height: `${cardHeight}px`, transform: `scale(${previewScale})` }"
          >
            <ShareUsageCard
              :locale="locale"
              :summary="summary"
              :currency="store.settings.currency"
              :range-label="rangeLabel"
              :scope-label="scopeLabel"
              :generated-at-label="generatedAtLabel"
              :display-name="displayName"
              :theme="themeId"
              :visual="visualMode"
              :calendar-bounds="calendarBounds"
              :include-trend="true"
              :include-models="true"
            />
          </div>
          <div v-if="loading" class="share-window__loading">
            <Loader2 class="h-6 w-6 animate-spin" />
          </div>
        </div>
      </section>

      <aside class="share-window__panel" :style="panelThemeVars">
        <button type="button" class="share-window__close" @click="closeWindow">
          <X class="h-5 w-5" />
        </button>

        <div class="share-window__title-block">
          <span><Sparkles class="h-4 w-4" />{{ t(locale, 'app.name') }}</span>
          <h1>{{ t(locale, 'statistics.shareTitle') }}</h1>
          <p>{{ t(locale, 'statistics.shareSubtitle') }}</p>
        </div>

        <section class="share-window__group">
          <label>{{ t(locale, 'statistics.shareDisplayName') }}</label>
          <input v-model="displayName" type="text" :placeholder="t(locale, 'statistics.shareDisplayNamePlaceholder')" />
        </section>

        <section class="share-window__group">
          <label>{{ t(locale, 'statistics.shareTimeRange') }}</label>
          <div class="share-window__range-grid">
            <button
              v-for="item in rangeOptions"
              :key="item.value"
              type="button"
              :class="{ 'share-window__choice--active': preset === item.value }"
              @click="preset = item.value"
            >
              {{ t(locale, item.key) }}
            </button>
          </div>
        </section>

        <section class="share-window__group">
          <label>{{ t(locale, 'statistics.shareTheme') }}</label>
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
              @click="themeId = theme.id"
            >
              <Check v-if="themeId === theme.id" class="h-4 w-4" />
            </button>
          </div>
        </section>

        <section class="share-window__actions">
          <label>{{ t(locale, 'statistics.shareActions') }}</label>
          <button type="button" class="share-window__primary" :disabled="exporting || loading" @click="copyImage">
            <Copy class="h-5 w-5" />
            <span>{{ t(locale, 'statistics.shareCopyImage') }}</span>
          </button>
          <button type="button" :disabled="exporting || loading" @click="saveImage">
            <Download class="h-5 w-5" />
            <span>{{ t(locale, 'statistics.shareSaveImage') }}</span>
          </button>
          <p :class="{ 'share-window__status--ok': status === 'copied' }">
            <Check v-if="status === 'copied'" class="h-4 w-4" />
            <span>{{ exporting ? t(locale, 'statistics.sharePreparing') : statusLabel }}</span>
          </p>
        </section>
      </aside>
    </div>
  </main>
</template>

<style scoped>
.share-window {
  display: grid;
  grid-template-columns: 1fr;
  height: 100vh;
  padding: 12px;
  overflow: hidden;
  color: rgba(255, 255, 255, 0.9);
  background:
    radial-gradient(circle at 20% 16%, rgba(16, 185, 129, 0.16), transparent 26%),
    radial-gradient(circle at 74% 14%, rgba(99, 102, 241, 0.16), transparent 24%),
    linear-gradient(145deg, rgba(8, 12, 16, 0.95), rgba(16, 18, 22, 0.94));
}

.share-window__chrome {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 350px;
  height: calc(100vh - 24px);
  min-height: 0;
  overflow: hidden;
  border: 1px solid rgba(255, 255, 255, 0.13);
  border-radius: 24px;
  background:
    linear-gradient(135deg, rgba(255, 255, 255, 0.1), rgba(255, 255, 255, 0.035)),
    rgba(9, 10, 12, 0.8);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.16),
    inset 0 -1px 0 rgba(255, 255, 255, 0.05),
    0 24px 84px rgba(0, 0, 0, 0.42);
  backdrop-filter: blur(28px) saturate(1.15);
}

.share-window__preview {
  position: relative;
  display: grid;
  grid-template-rows: auto 1fr;
  gap: 12px;
  min-width: 0;
  min-height: 0;
  place-items: center;
  padding: 20px 26px;
  overflow: hidden;
  background:
    linear-gradient(rgba(255, 255, 255, 0.035) 1px, transparent 1px),
    linear-gradient(90deg, rgba(255, 255, 255, 0.03) 1px, transparent 1px),
    radial-gradient(circle at 44% 34%, rgba(16, 185, 129, 0.2), transparent 34%),
    radial-gradient(circle at 76% 70%, rgba(79, 70, 229, 0.18), transparent 30%),
    rgba(5, 7, 9, 0.74);
  background-size: 38px 38px, 38px 38px, auto, auto, auto;
}

.share-window__preview-head {
  display: flex;
  width: min(100%, 560px);
  align-items: flex-start;
  justify-content: space-between;
  gap: 22px;
  justify-self: center;
  align-self: end;
}

.share-window__preview-head span,
.share-window__actions > label {
  display: block;
  color: rgba(255, 255, 255, 0.46);
  font-size: 12px;
  font-weight: 760;
  letter-spacing: 0.14em;
  text-transform: uppercase;
}

.share-window__preview-head strong {
  display: block;
  overflow: hidden;
  max-width: 310px;
  margin-top: 6px;
  color: rgba(255, 255, 255, 0.92);
  font-size: 18px;
  line-height: 1.1;
  font-weight: 820;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-window__preview-head p {
  max-width: 210px;
  margin: 0;
  color: rgba(255, 255, 255, 0.48);
  font-size: 12px;
  line-height: 1.45;
  font-weight: 650;
  text-align: right;
}

.share-window__preview-frame {
  position: relative;
  align-self: start;
  overflow: hidden;
  border-radius: 20px;
  box-shadow:
    0 36px 100px rgba(0, 0, 0, 0.56),
    0 0 0 1px rgba(255, 255, 255, 0.1),
    0 0 0 10px rgba(255, 255, 255, 0.025);
}

.share-window__card-scale {
  transform-origin: top left;
}

.share-window__loading {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  color: rgba(255, 255, 255, 0.75);
  background: rgba(0, 0, 0, 0.28);
}

.share-window__panel {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 14px;
  min-width: 0;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 60px 24px 24px;
  border-left: 1px solid rgba(255, 255, 255, 0.1);
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.07), transparent 30%),
    rgba(12, 13, 15, 0.72);
  backdrop-filter: blur(18px);
}

.share-window__panel::-webkit-scrollbar {
  width: 0;
  height: 0;
}

.share-window__close {
  position: absolute;
  top: 20px;
  right: 22px;
  display: grid;
  width: 34px;
  height: 34px;
  place-items: center;
  border-radius: 999px;
  color: rgba(255, 255, 255, 0.52);
  background: rgba(255, 255, 255, 0.04);
  transition: background 0.16s ease, color 0.16s ease;
}

.share-window__close:hover {
  color: rgba(255, 255, 255, 0.9);
  background: rgba(255, 255, 255, 0.08);
}

.share-window__title-block {
  padding-bottom: 2px;
}

.share-window__title-block > span {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
  color: rgba(255, 255, 255, 0.38);
  font-size: 12px;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
}

.share-window h1 {
  margin: 0;
  color: rgba(255, 255, 255, 0.94);
  font-size: 31px;
  line-height: 1.1;
  font-weight: 820;
}

.share-window p {
  margin: 7px 0 0;
  color: rgba(255, 255, 255, 0.52);
  font-size: 13px;
  line-height: 1.5;
  font-weight: 600;
}

.share-window__group {
  display: grid;
  gap: 10px;
  padding: 13px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 18px;
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.06), rgba(255, 255, 255, 0.025)),
    rgba(255, 255, 255, 0.02);
}

.share-window__group > label {
  color: rgba(255, 255, 255, 0.54);
  font-size: 12px;
  font-weight: 760;
  letter-spacing: 0.02em;
}

.share-window input[type="text"] {
  height: 42px;
  min-width: 0;
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 14px;
  padding: 0 14px;
  color: rgba(255, 255, 255, 0.92);
  background: rgba(0, 0, 0, 0.16);
  font-size: 14px;
  font-weight: 650;
  outline: none;
}

.share-window input[type="text"]:focus {
  border-color: rgba(255, 255, 255, 0.32);
}

.share-window__range-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.share-window__range-grid button {
  position: relative;
  min-width: 0;
  height: 42px;
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 12px;
  color: rgba(255, 255, 255, 0.62);
  background: rgba(255, 255, 255, 0.035);
  font-size: 13px;
  font-weight: 720;
  letter-spacing: 0.01em;
  cursor: pointer;
  transition:
    background 0.2s cubic-bezier(0.4, 0, 0.2, 1),
    border-color 0.2s ease,
    color 0.2s ease,
    transform 0.12s ease;
}

.share-window__range-grid button:hover {
  color: rgba(255, 255, 255, 0.92);
  border-color: rgba(255, 255, 255, 0.16);
  background: rgba(255, 255, 255, 0.07);
}

.share-window__range-grid button:active {
  transform: scale(0.97);
}

.share-window__choice--active {
  border-color: transparent !important;
  color: #06231b !important;
  background: linear-gradient(135deg, var(--share-grad-1, #34d399), var(--share-grad-2, #10b981)) !important;
  box-shadow:
    0 6px 18px rgba(16, 185, 129, 0.28),
    inset 0 1px 0 rgba(255, 255, 255, 0.3);
}

.share-window__choice--active:hover {
  color: #06231b !important;
  background: linear-gradient(135deg, var(--share-grad-1, #34d399), var(--share-grad-2, #10b981)) !important;
}

/* Theme color picker: round swatches with a soft active ring. */
.share-window__themes {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  padding: 2px 0;
}

.share-window__swatch {
  position: relative;
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  border: none;
  border-radius: 999px;
  color: #ffffff;
  background: var(--swatch);
  cursor: pointer;
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.3),
    0 4px 12px rgba(0, 0, 0, 0.3);
  transition: transform 0.18s cubic-bezier(0.34, 1.56, 0.64, 1), box-shadow 0.18s ease;
}

.share-window__swatch::after {
  content: '';
  position: absolute;
  inset: -5px;
  border-radius: 999px;
  border: 2px solid var(--swatch);
  opacity: 0;
  transform: scale(0.85);
  transition: opacity 0.18s ease, transform 0.18s ease;
}

.share-window__swatch:hover {
  transform: translateY(-2px) scale(1.06);
}

.share-window__swatch--active {
  transform: scale(1.04);
}

.share-window__swatch--active::after {
  opacity: 0.7;
  transform: scale(1);
}

.share-window__actions {
  margin-top: 2px;
  padding-top: 18px;
  border-top: 1px solid rgba(255, 255, 255, 0.1);
  display: grid;
  gap: 9px;
}

.share-window__actions button {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-width: 0;
  height: 44px;
  padding: 0 16px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 13px;
  color: rgba(255, 255, 255, 0.82);
  background: rgba(255, 255, 255, 0.045);
  font-size: 13px;
  font-weight: 740;
  cursor: pointer;
  transition:
    background 0.2s ease,
    border-color 0.2s ease,
    color 0.2s ease,
    transform 0.12s ease;
}

.share-window__actions button:hover {
  color: rgba(255, 255, 255, 0.95);
  border-color: rgba(255, 255, 255, 0.2);
  background: rgba(255, 255, 255, 0.09);
}

.share-window__actions button:active {
  transform: scale(0.98);
}

.share-window__actions button:disabled {
  cursor: not-allowed;
  opacity: 0.52;
}

.share-window__actions .share-window__primary {
  color: #06231b;
  border-color: transparent;
  background: linear-gradient(135deg, var(--share-grad-1, #34d399), var(--share-grad-2, #10b981));
  box-shadow: 0 8px 22px rgba(16, 185, 129, 0.26);
}

.share-window__actions .share-window__primary:hover {
  color: #06231b;
  border-color: transparent;
  filter: brightness(1.05);
  background: linear-gradient(135deg, var(--share-grad-1, #34d399), var(--share-grad-2, #10b981));
}

.share-window__actions p {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 20px;
  color: #fb7185;
  font-size: 12px;
  font-weight: 700;
}

.share-window__status--ok {
  color: #34d399 !important;
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
</style>
