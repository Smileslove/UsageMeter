<script setup lang="ts">
import { computed } from 'vue'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenValue } from '../../utils/format'
import { isOpaqueModelId } from '../../utils/modelDisplay'
import type { AppLocale, CurrencySettings, StatisticsModelBreakdown, StatisticsSummary } from '../../types'
import { shareThemeById } from './shareThemes'

const props = defineProps<{
  locale: AppLocale
  summary: StatisticsSummary | null
  currency: CurrencySettings
  rangeLabel: string
  scopeLabel: string
  generatedAtLabel: string
  displayName: string
  theme: string
  visual: 'chart' | 'calendar' | 'year'
  calendarBounds?: { start: number; end: number } | null
  includeStats: boolean
  includeTrend: boolean
  includeModels: boolean
}>()

const activeTheme = computed(() => shareThemeById(props.theme))
const themeVars = computed(() => ({
  '--share-background': activeTheme.value.background,
  '--share-paper': activeTheme.value.foreground,
  '--share-muted': activeTheme.value.muted,
  '--share-ink': activeTheme.value.ink,
  '--share-on-ink': activeTheme.value.background
}))
const reportDate = computed(() => props.summary ? new Intl.DateTimeFormat(props.locale, {
  year: 'numeric', month: '2-digit', day: '2-digit'
}).format(new Date(props.summary.generatedAtEpoch * 1000)) : '—')
const MODEL_LIMIT = 5

const totals = computed(() => props.summary?.totals ?? null)
const cursorEvents = computed(() => props.summary?.capability.cursorUsageEvents ?? 0)
const cursorOnly = computed(() => cursorEvents.value > 0 && cursorEvents.value === totals.value?.requestCount)
const unknownCursorCost = computed(() => cursorOnly.value && (props.summary?.capability.cursorUnknownCostEvents ?? 0) === cursorEvents.value)
const incompleteCursorTokens = computed(() => cursorOnly.value && (props.summary?.capability.cursorIncompleteUsageEvents ?? 0) > 0)
const hasData = computed(() => (totals.value?.totalTokens ?? 0) > 0 || (totals.value?.requestCount ?? 0) > 0)

const totalTokensValue = computed(() => totals.value?.totalTokens ?? 0)
// Only the user-provided display name appears on the poster; the scope fallback is dropped.
const ownerLabel = computed(() => props.displayName.trim())

const numberFormatter = computed(() => new Intl.NumberFormat(props.locale))
function formatExact(value: number): string {
  return numberFormatter.value.format(Math.round(value))
}

const exactTotalTokens = computed(() => incompleteCursorTokens.value ? '—' : formatExact(totalTokensValue.value))
const exactInputTokens = computed(() => incompleteCursorTokens.value ? '—' : formatExact((totals.value?.inputTokens ?? 0) + (totals.value?.cacheReadTokens ?? 0)))
const exactOutputTokens = computed(() => incompleteCursorTokens.value ? '—' : formatExact(totals.value?.outputTokens ?? 0))

const trendValues = computed(() => {
  const values = (props.summary?.trend ?? []).map(point => point.totalTokens)
  return values.length ? values : [0, 0, 0, 0, 0]
})

const heroFontSize = computed(() => hasData.value ? `${Math.min(props.includeTrend && props.visual !== 'chart' ? 145 : 176, 1780 / exactTotalTokens.value.length) * (cursorEvents.value > 0 ? 0.85 : 1)}px` : '72px')
const trendLabels = computed(() => {
  const points = props.summary?.trend ?? []
  const indexes = [...new Set([0, Math.floor((points.length - 1) / 2), points.length - 1])]
  return indexes.filter(index => index >= 0 && points[index]).map(index => points[index].label)
})

const topModels = computed(() => {
  return [...(props.summary?.models ?? [])]
    .filter(model => model.totalTokens > 0 && !isOpaqueModelId(model.modelName))
    .sort((a, b) => b.totalTokens - a.totalTokens)
    .slice(0, props.includeTrend && props.visual !== 'chart' ? 3 : MODEL_LIMIT)
})

// Bars are scaled relative to the leader's share so the #1 row fills the track,
// matching the reference where the top model spans nearly the full width.
const topModelPercent = computed(() => topModels.value[0]?.percent ?? 0)

function modelBarWidth(model: StatisticsModelBreakdown): string {
  const max = topModelPercent.value
  if (max <= 0) return '4%'
  const ratio = (model.percent / max) * 100
  return `${Math.max(4, Math.min(100, ratio)).toFixed(1)}%`
}

const visualMode = computed<'chart' | 'calendar' | 'year'>(() => props.visual)
const isHeatmap = computed(() => visualMode.value === 'calendar' || visualMode.value === 'year')

// Bar chart for short ranges (<30 days): one bar per trend bucket, height by tokens.
const barChart = computed(() => {
  const values = trendValues.value
  const max = Math.max(...values, 1)
  return values.map((value, index) => {
    const pct = (value / max) * 100
    // Keep non-zero buckets faintly visible even when tiny.
    const heightPct = value > 0 ? Math.max(2, pct) : 0
    return { key: `${index}-${value}`, heightPct }
  })
})

// GitHub-style contribution heatmap, built from daily trend points.
const HEATMAP_COLORS = [
  'color-mix(in srgb, var(--share-paper) 8%, var(--share-background))',
  'color-mix(in srgb, var(--share-ink) 25%, var(--share-background))',
  'color-mix(in srgb, var(--share-ink) 45%, var(--share-background))',
  'color-mix(in srgb, var(--share-ink) 70%, var(--share-background))',
  'var(--share-ink)'
]

function dayKeyFromEpoch(epoch: number): string {
  const date = new Date(epoch * 1000)
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

interface HeatCell {
  key: string
  date: Date | null
  tokens: number
  level: number
  inRange: boolean
}

const dailyTokenMap = computed(() => {
  const map = new Map<string, number>()
  for (const point of props.summary?.trend ?? []) {
    const key = dayKeyFromEpoch(point.startEpoch)
    map.set(key, (map.get(key) ?? 0) + point.totalTokens)
  }
  return map
})

function levelForTokens(tokens: number, max: number): number {
  if (tokens <= 0) return 0
  const ratio = tokens / max
  if (ratio < 0.25) return 1
  if (ratio < 0.5) return 2
  if (ratio < 0.75) return 3
  return 4
}

function rangeBounds(): { first: Date; last: Date } | null {
  const range = props.summary?.range
  if (!range || range.endEpoch < range.startEpoch) return null
  const first = new Date(range.startEpoch * 1000)
  const last = new Date(range.endEpoch * 1000)
  first.setHours(0, 0, 0, 0)
  last.setHours(0, 0, 0, 0)
  return { first, last }
}

// A continuous Monday-aligned grid keeps every date exactly once, including empty days.
const heatmapWeeks = computed<HeatCell[][]>(() => {
  const bounds = rangeBounds()
  if (!bounds) return []
  const { first, last } = bounds

  const gridStart = new Date(first)
  const offset = (gridStart.getDay() + 6) % 7 // 0 = Monday
  gridStart.setDate(gridStart.getDate() - offset)

  const map = dailyTokenMap.value
  const max = Math.max(1, ...map.values())

  const weeks: HeatCell[][] = []
  const cursor = new Date(gridStart)
  let guard = 0
  while (cursor <= last && guard < 60) {
    const week: HeatCell[] = []
    for (let d = 0; d < 7; d += 1) {
      const cellDate = new Date(cursor)
      const key = dayKeyFromEpoch(Math.floor(cellDate.getTime() / 1000))
      const inRange = cellDate >= first && cellDate <= last
      const tokens = map.get(key) ?? 0
      week.push({ key, date: cellDate, tokens, level: inRange ? levelForTokens(tokens, max) : 0, inRange })
      cursor.setDate(cursor.getDate() + 1)
    }
    weeks.push(week)
    guard += 1
  }
  return weeks
})

// Month view: calendar layout — rows are weeks, columns are Mon->Sun weekdays.
// The grid spans the calendar bounds from the parent (e.g. "本月" fills the entire
// month, so remaining days of the month appear as empty cells), falling back to the
// query range when no explicit bounds are provided.
const calendarWeeks = computed<HeatCell[][]>(() => {
  let first: Date
  let last: Date
  if (props.calendarBounds) {
    first = new Date(props.calendarBounds.start * 1000)
    last = new Date(props.calendarBounds.end * 1000)
  } else {
    const bounds = rangeBounds()
    if (!bounds) return []
    first = bounds.first
    last = bounds.last
  }
  first.setHours(0, 0, 0, 0)
  last.setHours(0, 0, 0, 0)

  const gridStart = new Date(first)
  const startOffset = (gridStart.getDay() + 6) % 7
  gridStart.setDate(gridStart.getDate() - startOffset)

  const gridEnd = new Date(last)
  const endOffset = 6 - ((gridEnd.getDay() + 6) % 7)
  gridEnd.setDate(gridEnd.getDate() + endOffset)

  const map = dailyTokenMap.value
  const max = Math.max(1, ...map.values())

  const weeks: HeatCell[][] = []
  const cursor = new Date(gridStart)
  let guard = 0
  while (cursor <= gridEnd && guard < 12) {
    const week: HeatCell[] = []
    for (let d = 0; d < 7; d += 1) {
      const cellDate = new Date(cursor)
      const key = dayKeyFromEpoch(Math.floor(cellDate.getTime() / 1000))
      const inRange = cellDate >= first && cellDate <= last
      const tokens = map.get(key) ?? 0
      week.push({ key, date: inRange ? cellDate : null, tokens, level: inRange ? levelForTokens(tokens, max) : 0, inRange })
      cursor.setDate(cursor.getDate() + 1)
    }
    weeks.push(week)
    guard += 1
  }
  return weeks
})

// Split the rolling year into four balanced bands without duplicating boundary weeks.
const yearBands = computed(() => {
  const weeks = heatmapWeeks.value
  const formatter = new Intl.DateTimeFormat(props.locale, { year: 'numeric', month: 'short', day: 'numeric' })
  return Array.from({ length: 4 }, (_, index) => {
    const bandWeeks = weeks.slice(Math.floor(index * weeks.length / 4), Math.floor((index + 1) * weeks.length / 4))
    const dates = bandWeeks.flat().filter(cell => cell.inRange && cell.date).map(cell => cell.date!)
    return {
      key: index,
      weeks: bandWeeks,
      startLabel: dates.length ? formatter.format(dates[0]) : '',
      endLabel: dates.length ? formatter.format(dates[dates.length - 1]) : ''
    }
  }).filter(band => band.weeks.length)
})

const heatmapRangeLabel = computed(() => {
  const bounds = props.calendarBounds
    ? { first: new Date(props.calendarBounds.start * 1000), last: new Date(props.calendarBounds.end * 1000) }
    : rangeBounds()
  if (!bounds) return ''
  const formatter = new Intl.DateTimeFormat(props.locale, { year: 'numeric', month: 'short', day: 'numeric' })
  return `${formatter.format(bounds.first)} — ${formatter.format(bounds.last)}`
})

const calendarStyle = computed(() => ({
  gridTemplateRows: `28px repeat(${Math.max(1, calendarWeeks.value.length)}, minmax(0, 1fr))`
}))

function cellDescription(cell: HeatCell): string {
  if (!cell.inRange || !cell.date) return ''
  return t(props.locale, 'statistics.shareHeatmapDayValue', {
    date: new Intl.DateTimeFormat(props.locale, { year: 'numeric', month: 'short', day: 'numeric' }).format(cell.date),
    tokens: formatExact(cell.tokens)
  })
}

function cellStyle(cell: HeatCell) {
  const inverse = activeTheme.value.appearance === 'dark' ? cell.level >= 3 : cell.level === 4
  return {
    backgroundColor: cell.inRange ? HEATMAP_COLORS[cell.level] : 'transparent',
    color: inverse ? 'var(--share-on-ink)' : 'var(--share-paper)'
  }
}

function calendarMonth(date: Date): string {
  return new Intl.DateTimeFormat(props.locale, { month: 'short' }).format(date)
}

const weekdayLabels = computed(() => {
  const formatter = new Intl.DateTimeFormat(props.locale, { weekday: 'short' })
  // Monday-anchored reference dates (2024-01-01 is a Monday).
  return [1, 2, 3, 4, 5, 6, 7].map(d => formatter.format(new Date(2024, 0, d)))
})

// Compact single-glyph weekday labels (Mon->Sun) for the year grid's left rail.
const weekdayNarrow = computed(() => {
  const formatter = new Intl.DateTimeFormat(props.locale, { weekday: 'narrow' })
  return [1, 2, 3, 4, 5, 6, 7].map(d => formatter.format(new Date(2024, 0, d)))
})

const heatmapActivity = computed(() => {
  let active = 0
  let streak = 0
  let longestStreak = 0
  let peak: HeatCell | null = null
  for (const cell of heatmapWeeks.value.flat()) {
    if (!cell.inRange) continue
    if (cell.tokens > 0) {
      active += 1
      streak += 1
      longestStreak = Math.max(longestStreak, streak)
      if (!peak || cell.tokens > peak.tokens) peak = cell
    } else {
      streak = 0
    }
  }
  return { active, longestStreak, peak }
})

const heatmapInsight = computed(() => {
  const { active, longestStreak, peak } = heatmapActivity.value
  if (visualMode.value === 'year') {
    return t(props.locale, 'statistics.shareHeatmapYearInsight', { count: active, streak: longestStreak })
  }
  if (!peak?.date) return ''
  return t(props.locale, 'statistics.shareHeatmapPeak', {
    tokens: formatTokenValue(peak.tokens),
    date: new Intl.DateTimeFormat(props.locale, { month: 'short', day: 'numeric' }).format(peak.date)
  })
})

const proofMetrics = computed(() => [
  {
    key: 'input',
    label: t(props.locale, 'statistics.inputTokens'),
    value: exactInputTokens.value
  },
  {
    key: 'output',
    label: t(props.locale, 'statistics.outputTokens'),
    value: exactOutputTokens.value
  },
  {
    key: 'requests',
    label: t(props.locale, cursorOnly.value ? 'cursor.eventCount' : 'statistics.requests'),
    value: formatRequestCount(totals.value?.requestCount ?? 0)
  },
  {
    key: 'cost',
    label: t(props.locale, cursorOnly.value ? 'cursor.costValue' : 'statistics.cost'),
    value: unknownCursorCost.value ? '—' : formatCost(totals.value?.cost ?? 0, props.currency)
  }
])

</script>

<template>
  <div class="share-card" :class="{ 'share-card--compact': topModels.length > 3 || cursorEvents > 0, 'share-card--annotated': cursorEvents > 0, 'share-card--heatmap': isHeatmap && includeTrend }" :style="themeVars">
    <header class="share-card__header">
      <div class="share-card__brand-lockup">
        <span class="share-card__mark" aria-hidden="true"><i></i><i></i><i></i></span>
        <p>{{ t(locale, 'app.name') }}</p>
      </div>
      <span class="share-card__date">{{ reportDate }}</span>
    </header>

    <main class="share-card__stage">
      <section class="share-card__hero">
        <p v-if="ownerLabel" class="share-card__owner">{{ ownerLabel }}</p>
        <h2 class="share-card__report-title">{{ t(locale, visualMode === 'year' ? 'statistics.shareYearlyReport' : visualMode === 'calendar' ? 'statistics.shareMonthlyReport' : 'statistics.sharePosterKicker') }}</h2>
        <p class="share-card__eyebrow">{{ t(locale, 'statistics.shareReportEyebrow') }}</p>
        <div class="share-card__title-row">
          <h1 :style="{ fontSize: heroFontSize }">{{ hasData ? exactTotalTokens : t(locale, 'statistics.shareNoData') }}</h1>
          <span>{{ t(locale, 'statistics.totalTokens') }}</span>
        </div>
      </section>

      <section v-if="includeStats" class="share-card__proof-grid">
        <article v-for="item in proofMetrics" :key="item.key" class="share-card__proof">
          <span>{{ item.label }}</span>
          <strong>{{ item.value }}</strong>
        </article>
      </section>

      <section class="share-card__lower">
        <article v-if="includeTrend" class="share-card__panel share-card__panel--trend" :aria-label="t(locale, isHeatmap ? 'statistics.tokenHeatmap' : 'statistics.tokenTrend')">
          <template v-if="isHeatmap">
            <div class="share-card__panel-head">
              <div>
                <h2>{{ t(locale, visualMode === 'year' ? 'statistics.shareYearActivity' : 'statistics.shareDailyUsage') }}</h2>
                <p class="share-card__heatmap-range">{{ heatmapRangeLabel }}</p>
              </div>
              <strong>{{ t(locale, 'statistics.shareHeatmapActiveDays', { count: heatmapActivity.active }) }}</strong>
            </div>
            <div class="share-card__heatmap">
              <div v-if="visualMode === 'year'" class="share-card__year">
                <div v-for="band in yearBands" :key="band.key" class="share-card__year-band">
                  <p class="share-card__year-label"><span>{{ band.startLabel }}</span><span>— {{ band.endLabel }}</span></p>
                  <div class="share-card__year-body">
                    <div class="share-card__year-weekdays" aria-hidden="true">
                      <span v-for="(day, index) in weekdayNarrow" :key="day + index" :class="{ 'share-card__year-weekday--hide': index % 2 !== 0 }">{{ day }}</span>
                    </div>
                    <div class="share-card__year-grid" :style="{ gridTemplateColumns: `repeat(${band.weeks.length}, minmax(0, 1fr))` }">
                      <div v-for="(week, wIndex) in band.weeks" :key="wIndex" class="share-card__year-col">
                        <span
                          v-for="cell in week"
                          :key="cell.key"
                          class="share-card__heatmap-cell"
                          :class="{ 'share-card__heatmap-cell--empty': !cell.inRange }"
                          :style="cellStyle(cell)"
                          :data-date="cell.inRange ? cell.key : undefined"
                          :title="cellDescription(cell)"
                          :aria-label="cellDescription(cell) || undefined"
                          :role="cell.inRange ? 'img' : undefined"
                        ></span>
                      </div>
                    </div>
                  </div>
                </div>
              </div>

              <div v-else class="share-card__calendar" :style="calendarStyle">
                <div class="share-card__calendar-weekdays" aria-hidden="true">
                  <span v-for="day in weekdayLabels" :key="day">{{ day }}</span>
                </div>
                <div v-for="(week, wIndex) in calendarWeeks" :key="wIndex" class="share-card__calendar-row">
                  <div
                    v-for="cell in week"
                    :key="cell.key"
                    class="share-card__heatmap-cell share-card__calendar-cell"
                    :class="{ 'share-card__heatmap-cell--empty': !cell.inRange }"
                    :style="cellStyle(cell)"
                    :data-date="cell.inRange ? cell.key : undefined"
                    :title="cellDescription(cell)"
                    :aria-label="cellDescription(cell) || undefined"
                    :role="cell.inRange ? 'img' : undefined"
                  >
                    <template v-if="cell.inRange && cell.date">
                      <div class="share-card__calendar-date">
                        <time :datetime="cell.key">{{ cell.date.getDate() }}</time>
                        <span v-if="cell.date.getDate() === 1">{{ calendarMonth(cell.date) }}</span>
                      </div>
                      <strong v-if="cell.tokens > 0">{{ formatTokenValue(cell.tokens) }}</strong>
                    </template>
                  </div>
                </div>
              </div>

              <div class="share-card__heatmap-caption">
                <p>{{ heatmapInsight }}</p>
                <div class="share-card__heatmap-legend">
                  <span>{{ t(locale, 'statistics.shareHeatmapLess') }}</span>
                  <i v-for="color in HEATMAP_COLORS" :key="color" :style="{ backgroundColor: color }"></i>
                  <span>{{ t(locale, 'statistics.shareHeatmapMore') }}</span>
                </div>
              </div>
            </div>
          </template>
          <template v-else>
            <div class="share-card__panel-head">
              <div>
                <h2>{{ t(locale, 'statistics.tokenTrend') }}</h2>
              </div>
            </div>
            <div class="share-card__chart">
              <div class="share-card__bars">
                <div
                  v-for="bar in barChart"
                  :key="bar.key"
                  class="share-card__bar-col"
                >
                  <span class="share-card__bar-fill" :style="{ height: `${bar.heightPct}%` }"></span>
                </div>
              </div>
            </div>
            <div class="share-card__chart-labels"><span v-for="label in trendLabels" :key="label">{{ label }}</span></div>
          </template>
        </article>

        <article v-if="includeModels" class="share-card__panel share-card__panel--models">
          <div class="share-card__models-head">
            <h2>{{ t(locale, 'statistics.shareModelUsage') }}</h2>
          </div>
          <p v-if="!topModels.length" class="share-card__models-empty">{{ t(locale, 'statistics.shareNoData') }}</p>
          <div v-else class="share-card__models">
            <div
              v-for="(model, index) in topModels"
              :key="model.modelName"
              class="share-card__model"
            >
              <span class="share-card__model-rank">{{ index + 1 }}</span>
              <span class="share-card__model-name">{{ model.modelName }}</span>
              <div class="share-card__model-track">
                <b
                  :style="{ width: modelBarWidth(model) }"
                ></b>
              </div>
              <div class="share-card__model-stats">
                <strong>{{ model.percent.toFixed(1) }}%</strong>
                <em>{{ formatExact(model.totalTokens) }} {{ t(locale, 'statistics.metricTokens') }}</em>
              </div>
            </div>
          </div>
        </article>
      </section>
    </main>

    <footer class="share-card__footer">
      <div>
        <p v-if="cursorEvents > 0">{{ t(locale, 'cursor.shareScopeNote') }}</p>
        <p v-if="(summary?.capability.cursorUnknownCostEvents ?? 0) > 0">{{ t(locale, 'cursor.partialCost', { count: summary?.capability.cursorUnknownCostEvents ?? 0 }) }}</p>
        <p v-if="(summary?.capability.cursorIncompleteUsageEvents ?? 0) > 0">{{ t(locale, 'cursor.knownTotalsNote') }}</p>
        <strong>{{ rangeLabel }}</strong>
      </div>
      <p>{{ t(locale, 'statistics.generatedBy') }}</p>
    </footer>
  </div>
</template>

<style scoped>
.share-card {
  --share-rule: color-mix(in srgb, var(--share-paper) 25%, transparent);
  --share-rule-soft: color-mix(in srgb, var(--share-paper) 12%, transparent);
  position: relative;
  display: flex;
  flex-direction: column;
  width: 1200px;
  height: 1760px;
  overflow: hidden;
  box-sizing: border-box;
  padding: 72px 88px;
  color: var(--share-paper);
  background: var(--share-background);
  font-family: var(--font-sans, system-ui, sans-serif);
}

.share-card * { box-sizing: border-box; }
.share-card p, .share-card h1, .share-card h2 { margin: 0; }

.share-card__header {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 40px;
  height: 52px;
}

.share-card__brand-lockup {
  display: flex;
  align-items: center;
  gap: 24px;
}

.share-card__brand-lockup p {
  font-size: 24px;
  font-weight: 550;
  letter-spacing: 0.24em;
  text-transform: uppercase;
}

.share-card__mark {
  display: flex;
  align-items: flex-end;
  gap: 7px;
  width: 40px;
  height: 40px;
}

.share-card__mark i { width: 8px; background: var(--share-paper); }
.share-card__mark i:nth-child(1) { height: 18px; }
.share-card__mark i:nth-child(2) { height: 29px; }
.share-card__mark i:nth-child(3) { height: 40px; }

.share-card__date {
  color: var(--share-muted);
  font-size: 24px;
  font-variant-numeric: tabular-nums;
}

.share-card__stage {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 0;
  padding-top: 44px;
}

.share-card__hero { flex: 0 0 auto; padding-bottom: 24px; }

.share-card__owner {
  overflow: hidden;
  max-width: 100%;
  margin-bottom: 16px !important;
  color: var(--share-muted);
  font-size: 24px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__report-title {
  font-family: 'Songti SC', 'Songti TC', 'Noto Serif CJK SC', ui-serif, Georgia, serif;
  font-size: 38px;
  font-weight: 500;
  letter-spacing: 0.16em;
  line-height: 1.3;
}

.share-card__eyebrow {
  margin-top: 12px !important;
  color: var(--share-muted);
  font-size: 18px;
  letter-spacing: 0.24em;
}

.share-card__title-row {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 10px;
  margin-top: 24px;
}

.share-card__title-row h1 {
  max-width: 100%;
  font-family: 'Baskerville', 'Times New Roman', 'Songti SC', serif;
  font-weight: 400;
  letter-spacing: -0.04em;
  line-height: 1;
  font-variant-numeric: lining-nums;
  white-space: nowrap;
}

.share-card__title-row > span {
  color: var(--share-muted);
  font-size: 22px;
  line-height: 1.4;
  letter-spacing: 0.06em;
}

.share-card__proof-grid {
  display: grid;
  flex: 0 0 auto;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  border-top: 1px solid var(--share-rule);
  border-bottom: 1px solid var(--share-rule);
}

.share-card__proof {
  position: relative;
  min-width: 0;
  padding: 20px 0;
}

.share-card__proof:nth-child(even) { padding-left: 52px; }
.share-card__proof:nth-child(even)::before {
  content: '';
  position: absolute;
  left: 0;
  top: 28px;
  bottom: 28px;
  width: 1px;
  background: var(--share-rule);
}
.share-card__proof:nth-child(n+3) { border-top: 1px solid var(--share-rule-soft); }

.share-card__proof > span { color: var(--share-muted); font-size: 22px; line-height: 1.4; }
.share-card__proof strong {
  display: block;
  overflow: hidden;
  margin-top: 10px;
  font-family: 'Baskerville', 'Times New Roman', serif;
  font-size: 42px;
  line-height: 1.1;
  font-weight: 400;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__lower {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 0;
  margin-top: 30px;
}

.share-card__panel {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

.share-card__panel--trend { flex: 1 1 auto; padding-bottom: 28px; }
.share-card__panel--models { flex: 0 0 auto; padding-top: 28px; border-top: 1px solid var(--share-rule); }
.share-card__panel--models:first-child { border-top: 0; padding-top: 0; }

.share-card__panel-head,
.share-card__models-head {
  display: flex;
  flex: 0 0 auto;
  align-items: baseline;
  justify-content: space-between;
  gap: 24px;
  margin-bottom: 24px;
}

.share-card__panel h2 { font-size: 30px; line-height: 1.2; font-weight: 500; letter-spacing: 0.02em; }
.share-card__panel-head > strong { color: var(--share-muted); font-size: 22px; font-weight: 400; }

.share-card__chart {
  flex: 1 1 auto;
  min-height: 160px;
  max-height: 420px;
  padding: 16px 0 0;
}

.share-card__bars {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
  height: 100%;
  border-bottom: 1px solid var(--share-rule);
  background: repeating-linear-gradient(to top, transparent 0, transparent calc(25% - 1px), var(--share-rule-soft) calc(25% - 1px), var(--share-rule-soft) 25%);
}

.share-card__bar-col {
  display: flex;
  flex: 1 1 0;
  align-items: flex-end;
  justify-content: center;
  height: 100%;
  min-width: 0;
}

.share-card__bar-fill {
  display: block;
  width: 76%;
  max-width: 62px;
  border-radius: 5px 5px 0 0;
  background: var(--share-ink);
}

.share-card__chart-labels {
  display: flex;
  flex: 0 0 auto;
  justify-content: space-between;
  gap: 16px;
  margin-top: 14px;
  color: var(--share-muted);
  font-size: 20px;
  font-variant-numeric: tabular-nums;
}

.share-card__heatmap-range { margin-top: 8px !important; color: var(--share-muted); font-size: 20px; }
.share-card__heatmap {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 20px;
  min-height: 0;
}
.share-card__year { display: grid; flex: 1 1 auto; grid-template-rows: repeat(4, minmax(0, 1fr)); gap: 12px; min-height: 0; }
.share-card__year-band { display: flex; min-height: 0; gap: 16px; }
.share-card__year-band + .share-card__year-band { padding-top: 10px; border-top: 1px solid var(--share-rule-soft); }
.share-card__year-label { flex: 0 0 160px; color: var(--share-muted); font-size: 18px; line-height: 1.5; }
.share-card__year-label span { display: block; }
.share-card__year-body { display: flex; flex: 1 1 auto; min-height: 0; gap: 12px; }
.share-card__year-weekdays { display: grid; grid-template-rows: repeat(7, minmax(0, 1fr)); gap: 4px; width: 20px; flex: 0 0 auto; }
.share-card__year-weekdays span { display: flex; align-items: center; color: var(--share-muted); font-size: 16px; line-height: 1; }
.share-card__year-weekday--hide { visibility: hidden; }
.share-card__year-grid { display: grid; flex: 1 1 auto; min-width: 0; gap: 6px; }
.share-card__year-col { display: grid; grid-template-rows: repeat(7, minmax(0, 1fr)); gap: 4px; min-height: 0; }
.share-card__calendar { display: grid; flex: 1 1 auto; gap: 10px; min-height: 0; }
.share-card__calendar-weekdays, .share-card__calendar-row { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); gap: 10px; min-height: 0; }
.share-card__calendar-weekdays span { color: var(--share-muted); font-size: 20px; line-height: 1.2; text-align: center; }
.share-card__heatmap-cell { min-width: 0; min-height: 0; border-radius: 4px; }
.share-card__calendar-cell { display: flex; flex-direction: column; justify-content: space-between; padding: 10px 12px; border-radius: 6px; }
.share-card__calendar-date { display: flex; align-items: baseline; justify-content: space-between; gap: 4px; font-size: 24px; font-weight: 600; line-height: 1.1; }
.share-card__calendar-date > span { font-size: 20px; font-weight: 600; }
.share-card__calendar-cell > strong { overflow: hidden; font-size: 24px; font-weight: 600; line-height: 1.2; text-align: right; text-overflow: ellipsis; white-space: nowrap; }
.share-card__heatmap-caption { display: flex; flex: 0 0 auto; align-items: center; justify-content: space-between; gap: 20px; min-height: 26px; color: var(--share-muted); font-size: 20px; }
.share-card__heatmap-caption > p { min-width: 0; }
.share-card__heatmap-legend { display: flex; flex: 0 0 auto; align-items: center; gap: 8px; font-size: 18px; }
.share-card__heatmap-legend i { width: 24px; height: 24px; border-radius: 3px; }

.share-card__models { display: flex; flex-direction: column; }
.share-card__models-empty { color: var(--share-muted); font-size: 24px; }
.share-card__model {
  display: grid;
  grid-template-columns: 30px minmax(0, 1fr) minmax(0, 0.8fr) 210px;
  align-items: center;
  gap: 20px;
  min-width: 0;
  min-height: 68px;
}
.share-card__model + .share-card__model { border-top: 1px solid var(--share-rule-soft); }
.share-card__model-rank { color: var(--share-muted); font-size: 22px; font-variant-numeric: tabular-nums; }
.share-card__model-name {
  min-width: 0;
  overflow: hidden;
  font-size: 27px;
  font-weight: 400;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.share-card__model-track { overflow: hidden; height: 9px; border-radius: 20px; background: var(--share-rule-soft); }
.share-card__model-track b { display: block; height: 100%; border-radius: 20px; background: var(--share-ink); }
.share-card__model-stats { display: flex; flex-direction: column; align-items: flex-end; gap: 6px; text-align: right; }
.share-card__model-stats strong { font-size: 27px; line-height: 1; font-weight: 400; font-variant-numeric: tabular-nums; }
.share-card__model-stats em { color: var(--share-muted); font-size: 17px; font-style: normal; font-variant-numeric: tabular-nums; white-space: nowrap; }

.share-card__footer {
  display: flex;
  flex: 0 0 auto;
  align-items: flex-end;
  justify-content: space-between;
  gap: 28px;
  margin-top: 28px;
  padding-top: 24px;
  border-top: 1px solid var(--share-rule);
}
.share-card__footer div { min-width: 0; }
.share-card__footer span { display: block; color: var(--share-muted); font-size: 18px; }
.share-card__footer strong { display: block; font-size: 20px; font-weight: 400; line-height: 1.4; }
.share-card__footer > p { flex-shrink: 0; color: var(--share-muted); font-size: 21px; white-space: nowrap; }
.share-card__footer div p { max-width: 760px; margin-bottom: 8px; color: var(--share-muted); font-size: 18px; line-height: 1.4; }

.share-card--compact .share-card__stage { padding-top: 32px; }
.share-card--compact .share-card__report-title { font-size: 34px; }
.share-card--compact .share-card__title-row { margin-top: 18px; }
.share-card--compact .share-card__models-head { margin-bottom: 18px; }
.share-card--compact .share-card__model { min-height: 58px; }
.share-card--compact .share-card__model-stats { gap: 4px; }
.share-card--compact .share-card__model-stats strong { font-size: 25px; }
.share-card--compact .share-card__model-stats em { font-size: 16px; line-height: 1.3; }

.share-card--annotated .share-card__stage { padding-top: 20px; }
.share-card--annotated .share-card__hero { padding-bottom: 18px; }
.share-card--annotated .share-card__proof { padding-block: 16px; }
.share-card--annotated .share-card__proof strong { font-size: 38px; }
.share-card--annotated .share-card__lower { margin-top: 22px; }

/* Match the calendar design: compact overview, substantial daily detail, top three models. */
.share-card--heatmap { padding-block: 56px; }
.share-card--heatmap .share-card__header { height: 44px; }
.share-card--heatmap .share-card__stage { padding-top: 20px; }
.share-card--heatmap .share-card__hero { padding-bottom: 16px; }
.share-card--heatmap .share-card__report-title { font-size: 34px; }
.share-card--heatmap .share-card__eyebrow { margin-top: 8px !important; font-size: 16px; }
.share-card--heatmap .share-card__title-row { margin-top: 16px; gap: 6px; }
.share-card--heatmap .share-card__proof { padding-block: 10px; }
.share-card--heatmap .share-card__proof > span { font-size: 20px; }
.share-card--heatmap .share-card__proof strong { margin-top: 6px; font-size: 32px; }
.share-card--heatmap .share-card__lower { margin-top: 24px; }
.share-card--heatmap .share-card__panel-head { margin-bottom: 18px; }
.share-card--heatmap .share-card__models-head { margin-bottom: 14px; }
.share-card--heatmap .share-card__model { min-height: 54px; }
.share-card--heatmap .share-card__footer { margin-top: 20px; padding-top: 18px; }
.share-card--heatmap.share-card--annotated .share-card__calendar-cell { padding: 6px 10px; }
.share-card--heatmap.share-card--annotated .share-card__calendar-date { font-size: 22px; }
.share-card--heatmap.share-card--annotated .share-card__calendar-cell > strong { font-size: 20px; }
</style>
