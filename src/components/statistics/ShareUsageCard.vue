<script setup lang="ts">
import { computed } from 'vue'
import { t } from '../../i18n'
import { formatCost, formatRequestCount } from '../../utils/format'
import { isOpaqueModelId } from '../../utils/modelDisplay'
import type { AppLocale, CurrencySettings, StatisticsModelBreakdown, StatisticsSummary, StatisticsTrendPoint } from '../../types'
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
  includeTrend: boolean
  includeModels: boolean
}>()

const activeTheme = computed(() => shareThemeById(props.theme))
const themeVars = computed(() => ({
  '--share-accent': activeTheme.value.accent,
  '--share-accent-deep': activeTheme.value.accentDeep,
  '--share-grad-1': activeTheme.value.grad1,
  '--share-grad-2': activeTheme.value.grad2,
  '--share-orb-a': activeTheme.value.orbA,
  '--share-orb-b': activeTheme.value.orbB
}))

// Rank colors for the model leaderboard, ordered #1 -> #5 to echo the reference design.
const rankPalette = ['#a855f7', '#3b82f6', '#10b981', '#f59e0b', '#ef4444', '#6366f1']
const MODEL_LIMIT = 5

const totals = computed(() => props.summary?.totals ?? null)
const hasData = computed(() => (totals.value?.totalTokens ?? 0) > 0 || (totals.value?.requestCount ?? 0) > 0)

const totalTokensValue = computed(() => totals.value?.totalTokens ?? 0)
// Only the user-provided display name appears on the poster; the scope fallback is dropped.
const ownerLabel = computed(() => props.displayName.trim())

const numberFormatter = computed(() => new Intl.NumberFormat(props.locale))
function formatExact(value: number): string {
  return numberFormatter.value.format(Math.round(value))
}

const exactTotalTokens = computed(() => formatExact(totalTokensValue.value))
const exactInputTokens = computed(() => formatExact((totals.value?.inputTokens ?? 0) + (totals.value?.cacheReadTokens ?? 0)))
const exactOutputTokens = computed(() => formatExact(totals.value?.outputTokens ?? 0))

const peakPoint = computed(() => {
  const points = props.summary?.trend ?? []
  return points.reduce<StatisticsTrendPoint | null>((peak, point) => {
    if (!peak || point.totalTokens > peak.totalTokens) return point
    return peak
  }, null)
})

const trendValues = computed(() => {
  const values = (props.summary?.trend ?? []).map(point => point.totalTokens)
  return values.length ? values : [0, 0, 0, 0, 0]
})

const trendSummary = computed(() => {
  const values = trendValues.value
  const first = values[0] ?? 0
  const last = values[values.length - 1] ?? 0
  if (first <= 0 && last <= 0) return t(props.locale, 'statistics.shareTrendSteady')
  const change = first > 0 ? ((last - first) / first) * 100 : 100
  if (change >= 5) return t(props.locale, 'statistics.shareTrendUp', { percent: Math.abs(change).toFixed(0) })
  if (change <= -5) return t(props.locale, 'statistics.shareTrendDown', { percent: Math.abs(change).toFixed(0) })
  return t(props.locale, 'statistics.shareTrendSteady')
})

const topModels = computed(() => {
  return [...(props.summary?.models ?? [])]
    .filter(model => model.totalTokens > 0 && !isOpaqueModelId(model.modelName))
    .sort((a, b) => b.totalTokens - a.totalTokens)
    .slice(0, MODEL_LIMIT)
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

// Single-glyph badge for the model icon, derived from the model name.
function modelGlyph(name: string): string {
  const trimmed = name.trim()
  return trimmed ? trimmed.charAt(0).toUpperCase() : '?'
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
const HEATMAP_COLORS = ['#ebedf0', '#9be9a8', '#40c463', '#30a14e', '#216e39']

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
  const points = props.summary?.trend ?? []
  if (!points.length) return null
  const epochs = points.map(p => p.startEpoch)
  const first = new Date(Math.min(...epochs) * 1000)
  const last = new Date(Math.max(...epochs) * 1000)
  first.setHours(0, 0, 0, 0)
  last.setHours(0, 0, 0, 0)
  return { first, last }
}

// Year view: GitHub contribution grid — columns are weeks, rows are Mon->Sun.
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
      // Keep the date even out of range so month labels can be derived per column.
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
// trend data range when no explicit bounds are provided.
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

// Year grid metrics: cells grow to fill the container HEIGHT (7 rows) so each block
// is clearly visible, then shrink only if the column count would overflow the width.
// Reserves room on the left for weekday labels and on top for month labels.
const YEAR_GRID_WIDTH = 860 // panel width minus the weekday label column
const YEAR_GRID_HEIGHT = 224 // container height minus the month label row
const yearWeekdayWidth = 22 // fixed width of the Mon/Wed/Fri rail glyphs
const yearMetrics = computed(() => {
  const weekCount = Math.max(1, heatmapWeeks.value.length)
  const gap = weekCount > 30 ? 4 : 6
  const cellByHeight = (YEAR_GRID_HEIGHT - gap * 6) / 7
  const cellByWidth = (YEAR_GRID_WIDTH - gap * (weekCount - 1)) / weekCount
  const cell = Math.round(Math.max(9, Math.min(cellByHeight, cellByWidth)))
  const radius = Math.max(3, Math.round(cell * 0.22))
  return { cell, gap, radius }
})

// Month labels above the year grid: one label per column where a new month starts.
const yearMonthLabels = computed(() => {
  const formatter = new Intl.DateTimeFormat(props.locale, { month: 'short' })
  const labels: Array<{ key: string; label: string; column: number }> = []
  let prevMonth = -1
  heatmapWeeks.value.forEach((week, index) => {
    const firstDay = week[0]?.date
    if (!firstDay) return
    const month = firstDay.getMonth()
    if (month !== prevMonth) {
      labels.push({ key: `${firstDay.getFullYear()}-${month}`, label: formatter.format(firstDay), column: index })
      prevMonth = month
    }
  })
  return labels
})

// Calendar metrics: 7 columns fill the width as large rounded squares, while the
// whole block stays within the fixed heatmap container height.
const CALENDAR_GRID_WIDTH = 760
const CALENDAR_AREA_HEIGHT = 250 // content band minus the weekday header row
const calendarMetrics = computed(() => {
  const rowCount = Math.max(1, calendarWeeks.value.length)
  const gap = 14
  const cellByWidth = (CALENDAR_GRID_WIDTH - gap * 6) / 7
  const cellByHeight = (CALENDAR_AREA_HEIGHT - gap * (rowCount - 1)) / rowCount
  const cell = Math.round(Math.max(28, Math.min(cellByWidth, cellByHeight)))
  const radius = Math.max(8, Math.round(cell * 0.22))
  return { cell, gap, radius }
})

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

const activeDayCount = computed(() => {
  let count = 0
  for (const tokens of dailyTokenMap.value.values()) {
    if (tokens > 0) count += 1
  }
  return count
})

const proofMetrics = computed(() => [
  {
    key: 'input',
    label: t(props.locale, 'statistics.inputTokens'),
    value: exactInputTokens.value,
    tone: 'violet'
  },
  {
    key: 'output',
    label: t(props.locale, 'statistics.outputTokens'),
    value: exactOutputTokens.value,
    tone: 'rose'
  },
  {
    key: 'requests',
    label: t(props.locale, 'statistics.requests'),
    value: formatRequestCount(totals.value?.requestCount ?? 0),
    tone: 'emerald'
  },
  {
    key: 'cost',
    label: t(props.locale, 'statistics.cost'),
    value: formatCost(totals.value?.cost ?? 0, props.currency),
    tone: 'amber'
  }
])

</script>

<template>
  <div class="share-card" :style="themeVars">
    <div class="share-card__paper-grain"></div>
    <div class="share-card__orb share-card__orb--one"></div>
    <div class="share-card__orb share-card__orb--two"></div>
    <header class="share-card__header">
      <div class="share-card__brand-lockup">
        <span class="share-card__mark"></span>
        <div>
          <p>{{ t(locale, 'app.name') }}</p>
          <strong>{{ t(locale, 'statistics.sharePosterKicker') }}</strong>
        </div>
      </div>
    </header>

    <main class="share-card__stage">
      <section class="share-card__hero">
        <p v-if="ownerLabel" class="share-card__owner">{{ ownerLabel }}</p>
        <div class="share-card__title-row">
          <h1>{{ hasData ? exactTotalTokens : t(locale, 'statistics.shareNoData') }}</h1>
          <span>{{ t(locale, 'statistics.metricTokens') }}</span>
        </div>
      </section>

      <section class="share-card__proof-grid">
        <article v-for="item in proofMetrics" :key="item.key" :class="['share-card__proof', `share-card__proof--${item.tone}`]">
          <span>{{ item.label }}</span>
          <strong>{{ item.value }}</strong>
        </article>
      </section>

      <section class="share-card__lower">
        <article v-if="includeTrend" class="share-card__panel share-card__panel--trend">
          <template v-if="isHeatmap">
            <div class="share-card__panel-head">
              <div>
                <h2>{{ t(locale, 'statistics.tokenHeatmap') }}</h2>
              </div>
              <strong>{{ t(locale, 'statistics.shareHeatmapActiveDays', { count: activeDayCount }) }}</strong>
            </div>
            <div class="share-card__heatmap">
              <!-- Year: GitHub contribution grid with month + weekday labels. -->
              <div v-if="visualMode === 'year'" class="share-card__year">
                <div
                  class="share-card__year-months"
                  :style="{ marginLeft: `${yearWeekdayWidth + 14}px`, height: '24px' }"
                >
                  <span
                    v-for="month in yearMonthLabels"
                    :key="month.key"
                    :style="{ left: `${month.column * (yearMetrics.cell + yearMetrics.gap)}px` }"
                  >{{ month.label }}</span>
                </div>
                <div class="share-card__year-body">
                  <div class="share-card__year-weekdays" :style="{ gap: `${yearMetrics.gap}px` }">
                    <span
                      v-for="(day, index) in weekdayNarrow"
                      :key="day + index"
                      :style="{ height: `${yearMetrics.cell}px` }"
                      :class="{ 'share-card__year-weekday--hide': index % 2 !== 0 }"
                    >{{ day }}</span>
                  </div>
                  <div class="share-card__year-grid" :style="{ gap: `${yearMetrics.gap}px` }">
                    <div
                      v-for="(week, wIndex) in heatmapWeeks"
                      :key="wIndex"
                      class="share-card__year-col"
                      :style="{ gap: `${yearMetrics.gap}px` }"
                    >
                      <span
                        v-for="cell in week"
                        :key="cell.key"
                        class="share-card__heatmap-cell"
                        :class="{ 'share-card__heatmap-cell--empty': !cell.inRange }"
                        :style="{
                          width: `${yearMetrics.cell}px`,
                          height: `${yearMetrics.cell}px`,
                          borderRadius: `${yearMetrics.radius}px`,
                          backgroundColor: cell.inRange ? HEATMAP_COLORS[cell.level] : 'transparent'
                        }"
                      ></span>
                    </div>
                  </div>
                </div>
              </div>

              <!-- Month: calendar layout, rows are weeks, columns are weekdays. -->
              <div v-else class="share-card__calendar" :style="{ gap: `${calendarMetrics.gap}px` }">
                <div class="share-card__calendar-weekdays" :style="{ gap: `${calendarMetrics.gap}px` }">
                  <span v-for="day in weekdayLabels" :key="day" :style="{ width: `${calendarMetrics.cell}px` }">{{ day }}</span>
                </div>
                <div
                  v-for="(week, wIndex) in calendarWeeks"
                  :key="wIndex"
                  class="share-card__calendar-row"
                  :style="{ gap: `${calendarMetrics.gap}px` }"
                >
                  <span
                    v-for="cell in week"
                    :key="cell.key"
                    class="share-card__heatmap-cell"
                    :class="{ 'share-card__heatmap-cell--empty': !cell.inRange }"
                    :style="{
                      width: `${calendarMetrics.cell}px`,
                      height: `${calendarMetrics.cell}px`,
                      borderRadius: `${calendarMetrics.radius}px`,
                      backgroundColor: cell.inRange ? HEATMAP_COLORS[cell.level] : 'transparent'
                    }"
                  ></span>
                </div>
              </div>

              <div class="share-card__heatmap-legend">
                <span>{{ t(locale, 'statistics.shareHeatmapLess') }}</span>
                <i v-for="color in HEATMAP_COLORS" :key="color" :style="{ backgroundColor: color }"></i>
                <span>{{ t(locale, 'statistics.shareHeatmapMore') }}</span>
              </div>
            </div>
          </template>
          <template v-else>
            <div class="share-card__panel-head">
              <div>
                <h2>{{ t(locale, 'statistics.tokenTrend') }}</h2>
                <span>{{ trendSummary }}</span>
              </div>
              <strong v-if="peakPoint">{{ peakPoint.label }}</strong>
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
          </template>
        </article>

        <article v-if="includeModels" class="share-card__panel share-card__panel--models">
          <div class="share-card__models-head">
            <h2>{{ t(locale, 'statistics.topModels') }}</h2>
            <span v-if="topModels.length">Top {{ topModels.length }}</span>
          </div>
          <p v-if="!topModels.length" class="share-card__models-empty">{{ t(locale, 'statistics.shareNoData') }}</p>
          <div v-else class="share-card__models">
            <div
              v-for="(model, index) in topModels"
              :key="model.modelName"
              class="share-card__model"
            >
              <span class="share-card__model-rank">{{ index + 1 }}</span>
              <span
                class="share-card__model-icon"
                :style="{ color: rankPalette[index % rankPalette.length] }"
              >{{ modelGlyph(model.modelName) }}</span>
              <span class="share-card__model-name">{{ model.modelName }}</span>
              <div class="share-card__model-track">
                <b
                  :style="{
                    width: modelBarWidth(model),
                    background: `linear-gradient(90deg, ${rankPalette[index % rankPalette.length]}, ${rankPalette[index % rankPalette.length]}cc)`
                  }"
                ></b>
              </div>
              <div class="share-card__model-stats">
                <strong :style="{ color: rankPalette[index % rankPalette.length] }">{{ model.percent.toFixed(1) }}%</strong>
                <em>{{ formatExact(model.totalTokens) }} {{ t(locale, 'statistics.metricTokens') }}</em>
              </div>
            </div>
          </div>
        </article>
      </section>
    </main>

    <footer class="share-card__footer">
      <div>
        <span>{{ t(locale, 'statistics.activeRange') }}</span>
        <strong>{{ rangeLabel }}</strong>
      </div>
      <p>{{ t(locale, 'statistics.generatedBy') }}</p>
    </footer>
  </div>
</template>

<style scoped>
.share-card {
  position: relative;
  display: flex;
  flex-direction: column;
  width: 1200px;
  height: 1760px;
  overflow: hidden;
  box-sizing: border-box;
  padding: 64px 70px;
  color: #101828;
  background:
    linear-gradient(135deg, rgba(255, 255, 255, 0.92), rgba(244, 247, 246, 0.9) 38%, rgba(249, 245, 238, 0.94)),
    #f8faf7;
  font-family: var(--font-sans, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif);
}

.share-card * {
  box-sizing: border-box;
}

.share-card__paper-grain {
  position: absolute;
  inset: 0;
  opacity: 0.52;
  pointer-events: none;
  background-image:
    linear-gradient(rgba(15, 23, 42, 0.025) 1px, transparent 1px),
    linear-gradient(90deg, rgba(15, 23, 42, 0.02) 1px, transparent 1px);
  background-size: 34px 34px;
  mask-image: linear-gradient(180deg, rgba(0, 0, 0, 0.84), transparent 78%);
}

.share-card__orb {
  position: absolute;
  border-radius: 999px;
  pointer-events: none;
}

.share-card__orb--one {
  top: -180px;
  right: -110px;
  width: 650px;
  height: 650px;
  background:
    radial-gradient(circle at 44% 44%, var(--share-orb-a), transparent 36%),
    radial-gradient(circle at 62% 58%, var(--share-orb-b), transparent 42%);
  filter: blur(1px);
}

.share-card__orb--two {
  left: -230px;
  bottom: 210px;
  width: 560px;
  height: 560px;
  background:
    radial-gradient(circle at 50% 50%, var(--share-orb-b), transparent 48%),
    radial-gradient(circle at 60% 42%, var(--share-orb-a), transparent 58%);
  opacity: 0.7;
}

.share-card__header,
.share-card__stage,
.share-card__footer {
  position: relative;
  z-index: 1;
}

.share-card__header {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 40px;
  padding-bottom: 26px;
  border-bottom: 1px solid rgba(15, 23, 42, 0.12);
}

.share-card__brand-lockup {
  display: flex;
  align-items: center;
  gap: 18px;
}

.share-card__mark {
  width: 42px;
  height: 42px;
  border-radius: 15px;
  background:
    linear-gradient(135deg, var(--share-grad-1), var(--share-grad-2));
  box-shadow: 0 16px 34px rgba(15, 23, 42, 0.18);
}

.share-card__brand-lockup p,
.share-card__proof span,
.share-card__footer span,
.share-card__panel-head span {
  margin: 0;
  color: rgba(15, 23, 42, 0.5);
  font-size: 20px;
  font-weight: 720;
}

.share-card__brand-lockup p {
  letter-spacing: 0.18em;
  text-transform: uppercase;
}

.share-card__brand-lockup strong {
  display: block;
  margin-top: 7px;
  color: #0f172a;
  font-size: 27px;
  line-height: 1;
  font-weight: 820;
}

.share-card__stage {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
  padding-top: 40px;
}

.share-card__hero {
  flex: 0 0 auto;
  max-width: 900px;
}

.share-card__owner {
  display: block;
  overflow: hidden;
  margin: 0 0 18px;
  color: rgba(15, 23, 42, 0.55);
  font-size: 24px;
  font-weight: 780;
  letter-spacing: 0.01em;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__title-row {
  display: flex;
  align-items: flex-end;
  gap: 26px;
}

.share-card__title-row h1 {
  max-width: 960px;
  overflow: hidden;
  margin: 0;
  color: #0f172a;
  font-family: ui-serif, Georgia, 'Times New Roman', serif;
  font-size: 118px;
  line-height: 0.9;
  font-weight: 500;
  letter-spacing: -0.01em;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__title-row span {
  margin-bottom: 16px;
  color: var(--share-accent);
  font-size: 29px;
  font-weight: 840;
  letter-spacing: 0.1em;
  text-transform: uppercase;
}

.share-card__proof,
.share-card__panel {
  border: 1px solid rgba(255, 255, 255, 0.64);
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.68), rgba(255, 255, 255, 0.38)),
    rgba(255, 255, 255, 0.42);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.78),
    0 24px 70px rgba(31, 41, 55, 0.08);
  backdrop-filter: blur(18px);
}

.share-card__proof-grid {
  display: grid;
  flex: 0 0 auto;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 14px;
  margin-top: 32px;
}

.share-card__proof {
  min-width: 0;
  min-height: 104px;
  padding: 20px 26px;
  border-radius: 26px;
}

.share-card__proof strong {
  display: block;
  overflow: hidden;
  margin-top: 12px;
  color: #0f172a;
  font-size: 32px;
  line-height: 1;
  font-weight: 840;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__proof--emerald strong { color: #047857; }
.share-card__proof--violet strong { color: #4f46e5; }
.share-card__proof--amber strong { color: #b45309; }
.share-card__proof--rose strong { color: #be123c; }

.share-card__lower {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  gap: 18px;
  min-height: 0;
  margin-top: 22px;
}

.share-card__panel {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  padding: 30px;
  border-radius: 34px;
}

.share-card__panel--trend {
  flex: 0 0 auto;
}

.share-card__panel--models {
  flex: 1 1 auto;
  overflow: hidden;
}

.share-card__panel-head {
  display: flex;
  flex: 0 0 auto;
  align-items: flex-start;
  justify-content: space-between;
  gap: 24px;
  margin-bottom: 20px;
}

.share-card__panel-head h2 {
  margin: 0;
  color: #0f172a;
  font-size: 31px;
  line-height: 1.05;
  font-weight: 840;
}

.share-card__panel-head span {
  display: block;
  overflow: hidden;
  margin-top: 8px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__panel-head strong {
  max-width: 280px;
  overflow: hidden;
  color: var(--share-accent-deep);
  font-size: 20px;
  font-weight: 820;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__chart {
  height: 360px;
  padding: 40px 26px 30px;
  border-radius: 28px;
  background: rgba(248, 250, 252, 0.58);
}

.share-card__bars {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 10px;
  width: 100%;
  height: 100%;
  border-bottom: 2px dashed rgba(15, 23, 42, 0.14);
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
  width: 100%;
  max-width: 56px;
  min-height: 0;
  border-radius: 12px 12px 0 0;
  background: linear-gradient(180deg, var(--share-grad-1), var(--share-grad-2));
}

/* Fixed-height heatmap area: keeps the poster height constant across ranges. */
.share-card__heatmap {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  height: 360px;
  padding: 22px 20px 54px;
  border-radius: 28px;
  background: rgba(248, 250, 252, 0.58);
}

/* Year: GitHub contribution grid with month + weekday labels. */
.share-card__year {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.share-card__year-months {
  position: relative;
}

.share-card__year-months span {
  position: absolute;
  top: 0;
  color: rgba(15, 23, 42, 0.5);
  font-size: 19px;
  font-weight: 720;
  white-space: nowrap;
}

.share-card__year-body {
  display: flex;
  align-items: flex-start;
  gap: 14px;
}

.share-card__year-weekdays {
  display: flex;
  flex: 0 0 auto;
  flex-direction: column;
  width: 22px;
}

.share-card__year-weekdays span {
  display: flex;
  align-items: center;
  justify-content: flex-start;
  color: rgba(15, 23, 42, 0.45);
  font-size: 17px;
  font-weight: 700;
  line-height: 1;
}

.share-card__year-weekday--hide {
  visibility: hidden;
}

.share-card__year-grid {
  display: flex;
}

.share-card__year-col {
  display: flex;
  flex-direction: column;
}

/* Month: calendar layout — rows are weeks, columns Mon->Sun. */
.share-card__calendar {
  display: flex;
  flex-direction: column;
  align-items: center;
}

.share-card__calendar-weekdays {
  display: flex;
  margin-bottom: 4px;
}

.share-card__calendar-weekdays span {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  color: rgba(15, 23, 42, 0.42);
  font-size: 17px;
  font-weight: 700;
  line-height: 1;
  white-space: nowrap;
}

.share-card__calendar-row {
  display: flex;
}

.share-card__heatmap-cell {
  flex: 0 0 auto;
  box-shadow: inset 0 0 0 1px rgba(15, 23, 42, 0.04);
}

.share-card__heatmap-cell--empty {
  box-shadow: none;
}

.share-card__heatmap-legend {
  position: absolute;
  right: 26px;
  bottom: 18px;
  display: flex;
  align-items: center;
  gap: 8px;
  color: rgba(15, 23, 42, 0.42);
  font-size: 16px;
  font-weight: 700;
}

.share-card__heatmap-legend i {
  width: 20px;
  height: 20px;
  border-radius: 5px;
  box-shadow: inset 0 0 0 1px rgba(15, 23, 42, 0.04);
}

.share-card__models-head {
  display: flex;
  flex: 0 0 auto;
  align-items: baseline;
  gap: 18px;
  margin-bottom: 16px;
}

.share-card__models-head h2 {
  margin: 0;
  color: #0f172a;
  font-size: 33px;
  line-height: 1;
  font-weight: 860;
}

.share-card__models-head span {
  color: rgba(15, 23, 42, 0.42);
  font-size: 22px;
  font-weight: 760;
  letter-spacing: 0.04em;
}

.share-card__models {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  justify-content: space-between;
  min-height: 0;
}

.share-card__models-empty {
  display: flex;
  flex: 1 1 auto;
  align-items: center;
  justify-content: center;
  margin: 0;
  color: rgba(15, 23, 42, 0.4);
  font-size: 26px;
  font-weight: 720;
}

/* One leaderboard row: rank badge | icon | name | progress track | stats. */
.share-card__model {
  display: grid;
  grid-template-columns: 46px 50px minmax(260px, 1fr) minmax(0, 1.7fr) auto;
  align-items: center;
  gap: 18px;
  min-width: 0;
  padding: 9px 0;
}

.share-card__model + .share-card__model {
  border-top: 1px solid rgba(15, 23, 42, 0.07);
}

.share-card__model-rank {
  display: flex;
  width: 46px;
  height: 46px;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  border-radius: 999px;
  border: 1px solid rgba(15, 23, 42, 0.12);
  background: rgba(255, 255, 255, 0.6);
  color: rgba(15, 23, 42, 0.7);
  font-size: 23px;
  font-weight: 840;
  font-variant-numeric: tabular-nums;
}

.share-card__model-icon {
  display: flex;
  width: 50px;
  height: 50px;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  border-radius: 15px;
  background: rgba(255, 255, 255, 0.78);
  box-shadow:
    inset 0 0 0 1px rgba(15, 23, 42, 0.08),
    0 8px 20px rgba(31, 41, 55, 0.06);
  font-size: 26px;
  font-weight: 860;
}

.share-card__model-name {
  min-width: 0;
  overflow: hidden;
  color: #101828;
  font-size: 27px;
  font-weight: 780;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__model-track {
  overflow: hidden;
  height: 16px;
  border-radius: 999px;
  background: rgba(148, 163, 184, 0.2);
}

.share-card__model-track b {
  display: block;
  height: 100%;
  border-radius: 999px;
}

.share-card__model-stats {
  display: flex;
  flex: 0 0 auto;
  flex-direction: column;
  align-items: flex-end;
  gap: 3px;
  min-width: 210px;
  text-align: right;
}

.share-card__model-stats strong {
  font-size: 30px;
  line-height: 1;
  font-weight: 860;
  font-variant-numeric: tabular-nums;
}

.share-card__model-stats em {
  color: rgba(15, 23, 42, 0.5);
  font-size: 19px;
  font-style: normal;
  font-weight: 720;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.share-card__footer {
  display: flex;
  flex: 0 0 auto;
  align-items: flex-end;
  justify-content: space-between;
  gap: 36px;
  margin-top: 28px;
  padding-top: 26px;
  border-top: 1px solid rgba(15, 23, 42, 0.12);
}

.share-card__footer div {
  min-width: 0;
}

.share-card__footer strong {
  display: block;
  overflow: hidden;
  max-width: 760px;
  margin-top: 8px;
  color: #0f172a;
  font-size: 21px;
  font-weight: 760;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.share-card__footer p {
  margin: 0;
  color: var(--share-accent-deep);
  font-size: 24px;
  font-weight: 860;
  white-space: nowrap;
}
</style>
