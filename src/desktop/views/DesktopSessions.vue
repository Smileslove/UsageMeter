<script setup lang="ts">
/**
 * 桌面主窗口「会话」页。
 * 会话表格 + 覆盖式抽屉（点击行展开详情，不跳转页面）。
 * 客户端分页：对已加载的 filteredSessions 做内存分页，到达末页时自动续载。
 * 表格状态（搜索/筛选/排序/分页/列配置）由 useSessionTable 统一管理。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { ArrowDown, ArrowUp, ExternalLink, Search, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { t } from '../../i18n'
import type { SessionStats } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useClipboard } from '../composables/useClipboard'
import { useFocusTrap } from '../composables/useFocusTrap'
import { useSessionTable, SESSION_COLUMNS, sessionTotalTokens } from '../composables/useSessionTable'
import LobeIcon from '../../components/LobeIcon.vue'
import DesktopSelect from '../components/DesktopSelect.vue'
import type { SelectOption } from '../components/DesktopSelect.vue'
import PaginationBar from '../components/PaginationBar.vue'
import ColumnConfigPopover from '../components/ColumnConfigPopover.vue'

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

const {
  formatTime,
  formatTokens,
  formatCost,
  formatDuration,
  sessionModelLabel,
  requestToolLabel,
  sessionUsageVisible,
  displaySessionTitle,
  displaySessionProjectBadge,
  projectBadgeClasses,
  getToolIcon,
} = useSessionDisplay(store)

const {
  selectedTool,
  loadingMore,
  initializeSessionView,
  disposeSessionView,
  searchQuery,
  filters,
  sortDesc,
  currentPage,
  pageSize,
  pageSizeSelectOptions,
  toggleColumn,
  isColumnVisible,
  visibleCols,
  paginatedSessions,
  total,
  totalPages,
  pageNumbers,
  hasPrev,
  hasNext,
  projectOptions,
  modelOptions,
  toolOptions,
  gotoPage,
  nextPage,
  prevPage,
  cycleSort,
  isSortedBy,
  onPageSizeChange,
  onTimeChange,
  clearAllFilters,
  applyPendingFilters,
} = useSessionTable(store, nav, locale)

// —— 筛选下拉选项 ——
const projectFilterLabel = (value: string | null) => {
  if (!value) return ''
  if (value === '__global__') return t(locale.value, 'common.global')
  if (value === '__unknown__') return t(locale.value, 'common.unknownProject')
  return value
}

const projectSelectOptions = computed<SelectOption[]>(() => [
  { value: null, label: t(locale.value, 'desktop.sessions.filterAllProjects') },
  ...projectOptions.value.map(p => ({ value: p, label: projectFilterLabel(p) })),
])

const modelSelectOptions = computed<SelectOption[]>(() => [
  { value: null, label: t(locale.value, 'desktop.sessions.filterAllModels') },
  ...modelOptions.value.map(m => ({ value: m, label: m })),
])

const timeSelectOptions = computed<SelectOption[]>(() => [
  { value: 'all', label: t(locale.value, 'desktop.sessions.filterTimeAll') },
  { value: 'today', label: t(locale.value, 'desktop.sessions.filterTimeToday') },
  { value: '7d', label: t(locale.value, 'desktop.sessions.filterTime7d') },
  { value: '30d', label: t(locale.value, 'desktop.sessions.filterTime30d') },
  { value: 'custom', label: t(locale.value, 'desktop.sessions.filterTimeCustom') },
])

const coverageSelectOptions = computed<SelectOption[]>(() => [
  { value: 'all', label: t(locale.value, 'desktop.sessions.filterCoverageAll') },
  { value: 'full', label: t(locale.value, 'desktop.sessions.filterCoverageFull') },
  { value: 'partial', label: t(locale.value, 'desktop.sessions.filterCoveragePartial') },
  { value: 'uncovered', label: t(locale.value, 'desktop.sessions.filterCoverageUncovered') },
])

const toolSelectOptions = computed<SelectOption[]>(() => [
  { value: null, label: t(locale.value, 'desktop.allTools') },
  ...toolOptions.value.map(tool => ({ value: tool, label: requestToolLabel(tool) })),
])

// —— 筛选 chips ——
interface Chip {
  id: string
  label: string
  remove: () => void
}
const chips = computed<Chip[]>(() => {
  const result: Chip[] = []
  const timeLabels: Record<string, string> = {
    today: t(locale.value, 'desktop.sessions.filterTimeToday'),
    '7d': t(locale.value, 'desktop.sessions.filterTime7d'),
    '30d': t(locale.value, 'desktop.sessions.filterTime30d'),
    custom: t(locale.value, 'desktop.sessions.filterTimeCustom'),
  }
  if (filters.time !== 'all') {
    result.push({
      id: 'time',
      label: timeLabels[filters.time] ?? '',
      remove: () => {
        filters.time = 'all'
        filters.customRange = null
      },
    })
  }
  if (selectedTool.value) {
    result.push({ id: 'tool', label: requestToolLabel(selectedTool.value), remove: () => { selectedTool.value = null } })
  }
  if (filters.project) {
    result.push({ id: 'project', label: projectFilterLabel(filters.project), remove: () => { filters.project = null } })
  }
  if (filters.model) {
    result.push({ id: 'model', label: filters.model, remove: () => { filters.model = null } })
  }
  if (filters.coverage !== 'all') {
    const coverageLabels: Record<string, string> = {
      full: t(locale.value, 'desktop.sessions.filterCoverageFull'),
      partial: t(locale.value, 'desktop.sessions.filterCoveragePartial'),
      uncovered: t(locale.value, 'desktop.sessions.filterCoverageUncovered'),
    }
    result.push({ id: 'coverage', label: coverageLabels[filters.coverage] ?? '', remove: () => { filters.coverage = 'all' } })
  }
  return result
})
const visibleChips = computed(() => chips.value.slice(0, 4))
const extraChipsCount = computed(() => Math.max(0, chips.value.length - 4))
const chipsCollapsed = ref(true)

// —— 抽屉 ——
const selectedSession = ref<SessionStats | null>(null)
const drawerOpen = ref(false)

const openDrawer = (session: SessionStats) => {
  selectedSession.value = session
  drawerOpen.value = true
}
const closeDrawer = () => {
  drawerOpen.value = false
  selectedSession.value = null
}

const onEscape = (event: KeyboardEvent) => {
  if (event.key === 'Escape' && drawerOpen.value) closeDrawer()
}

const handleRowKeydown = (event: KeyboardEvent, session: SessionStats) => {
  if (event.target !== event.currentTarget) return
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    openDrawer(session)
  }
}

const openInWorkspace = () => {
  const sessionKey = selectedSession.value?.sessionId
  closeDrawer()
  if (sessionKey) nav.openSession(sessionKey)
}

const shortId = (value: string) => (value.length > 12 ? `${value.slice(0, 6)}…${value.slice(-4)}` : value)
const { copiedValue, copyText } = useClipboard()

// —— 右键菜单 ——
interface ContextMenuState {
  session: SessionStats
  x: number
  y: number
}
const contextMenu = ref<ContextMenuState | null>(null)
const contextMenuOpen = computed(() => contextMenu.value !== null)
const contextMenuRef = ref<HTMLElement | null>(null)

const openContextMenu = (clientX: number, clientY: number, session: SessionStats) => {
  const MENU_WIDTH = 176
  const MENU_HEIGHT = 104
  const margin = 8
  const x = Math.max(margin, Math.min(clientX, window.innerWidth - MENU_WIDTH - margin))
  const y = Math.max(margin, Math.min(clientY, window.innerHeight - MENU_HEIGHT - margin))
  contextMenu.value = { session, x, y }
}
const closeContextMenu = () => { contextMenu.value = null }
useFocusTrap({ open: contextMenuOpen, container: contextMenuRef, onClose: closeContextMenu })

const handleRowContextMenu = (event: MouseEvent, session: SessionStats) => {
  openContextMenu(event.clientX, event.clientY, session)
}
const handleRowMenuKeydown = (event: KeyboardEvent, session: SessionStats) => {
  if ((event.shiftKey && event.key === 'F10') || event.key === 'ContextMenu') {
    event.preventDefault()
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
    openContextMenu(rect.left + 32, rect.bottom - 8, session)
  }
}
const handleMenuKeydown = (event: KeyboardEvent) => {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
  event.preventDefault()
  const items = Array.from(contextMenuRef.value?.querySelectorAll<HTMLElement>('button[role="menuitem"]') ?? []).filter(
    el => !(el as HTMLButtonElement).disabled,
  )
  if (items.length === 0) return
  const activeIndex = items.indexOf(document.activeElement as HTMLElement)
  const next = event.key === 'ArrowDown' ? activeIndex + 1 : activeIndex - 1
  items[(next + items.length) % items.length].focus()
}
const runContextAction = (action: 'open' | 'copyId' | 'copyCwd') => {
  const state = contextMenu.value
  if (!state) return
  const { session } = state
  closeContextMenu()
  if (action === 'open') {
    openDrawer(session)
  } else if (action === 'copyId') {
    void copyText(session.sessionId, t(locale.value, 'desktop.sessions.menuCopyId'))
  } else if (action === 'copyCwd' && session.cwd) {
    void copyText(session.cwd, t(locale.value, 'desktop.sessions.menuCopyCwd'))
  }
}

const closeMenuOnPointerDown = (event: PointerEvent) => {
  if (contextMenuRef.value?.contains(event.target as Node)) return
  closeContextMenu()
}
const closeMenuOnScroll = () => { closeContextMenu() }
watch(contextMenuOpen, isOpen => {
  if (isOpen) {
    document.addEventListener('pointerdown', closeMenuOnPointerDown, true)
    window.addEventListener('scroll', closeMenuOnScroll, true)
    window.addEventListener('resize', closeMenuOnScroll)
  } else {
    document.removeEventListener('pointerdown', closeMenuOnPointerDown, true)
    window.removeEventListener('scroll', closeMenuOnScroll, true)
    window.removeEventListener('resize', closeMenuOnScroll)
  }
})

const skeletonRows = [0, 1, 2, 3, 4, 5, 6, 7]

function onPageSizeUpdate(size: number) {
  pageSize.value = size
  onPageSizeChange()
}

onMounted(async () => {
  document.addEventListener('keydown', onEscape)
  await applyPendingFilters()
  await initializeSessionView()
})

onUnmounted(() => {
  document.removeEventListener('keydown', onEscape)
  disposeSessionView()
  document.removeEventListener('pointerdown', closeMenuOnPointerDown, true)
  window.removeEventListener('scroll', closeMenuOnScroll, true)
  window.removeEventListener('resize', closeMenuOnScroll)
})
</script>

<template>
  <div class="flex h-full flex-col gap-3">
    <!-- 工具栏 -->
    <div class="flex shrink-0 flex-wrap items-center gap-2 text-xs">
      <div class="relative">
        <Search class="pointer-events-none absolute left-2 top-1/2 h-3 w-3 -translate-y-1/2 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <input
          v-model="searchQuery"
          type="text"
          class="theme-input h-7 w-44 rounded-lg pl-7 pr-2 text-xs outline-none"
          :placeholder="t(locale, 'desktop.sessions.searchPlaceholder')"
          :aria-label="t(locale, 'desktop.sessions.searchPlaceholder')"
        />
      </div>

      <DesktopSelect
        v-model="filters.time"
        :options="timeSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterTime')"
        @change="onTimeChange"
      />

      <DesktopSelect
        v-model="filters.project"
        :options="projectSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterProject')"
      />

      <DesktopSelect
        v-model="filters.model"
        :options="modelSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterModel')"
      />

      <DesktopSelect
        v-model="filters.coverage"
        :options="coverageSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterCoverage')"
      />

      <DesktopSelect
        v-model="selectedTool"
        :options="toolSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterTool')"
      />

      <button
        v-if="chips.length > 0"
        type="button"
        class="h-7 rounded-lg px-2 text-xs text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
        @click="clearAllFilters"
      >
        {{ t(locale, 'desktop.requests.clearFilters') }}
      </button>

      <div class="ml-auto flex items-center gap-2">
        <span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.requests.totalRecords', { count: total }) }}</span>
        <ColumnConfigPopover
          :columns="SESSION_COLUMNS"
          :is-visible="isColumnVisible"
          :toggle-column="toggleColumn"
          :button-label="t(locale, 'desktop.requests.columnConfig')"
          :title-label="t(locale, 'desktop.requests.columnConfigTitle')"
          label-prefix="desktop.sessions.column"
          :locale="locale"
        />
      </div>
    </div>

    <!-- 筛选 chips -->
    <div v-if="chips.length > 0" class="flex shrink-0 flex-wrap items-center gap-1.5 text-xs">
      <span v-for="chip in (chipsCollapsed ? visibleChips : chips)" :key="chip.id" class="inline-flex items-center gap-1 rounded-full border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-2 py-0.5 font-medium text-[var(--theme-text-secondary)]">
        {{ chip.label }}
        <button type="button" class="rounded-full p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.removeFilter')" :title="t(locale, 'desktop.sessions.removeFilter')" @click="chip.remove()">
          <X class="h-3 w-3" aria-hidden="true" />
        </button>
      </span>
      <button v-if="extraChipsCount > 0" type="button" class="rounded-full border border-[var(--theme-border-default)] px-2 py-0.5 font-medium text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]" @click="chipsCollapsed = !chipsCollapsed">
        {{ chipsCollapsed ? t(locale, 'desktop.sessions.chipsMore', { count: extraChipsCount }) : t(locale, 'desktop.sessions.chipsLess') }}
      </button>
    </div>

    <!-- 表格 + 覆盖式抽屉 -->
    <div class="relative min-h-0 flex-1">
      <div class="theme-surface h-full overflow-auto rounded-xl border">
        <table class="w-full min-w-[860px] border-collapse text-xs">
          <thead class="sticky top-0 z-10">
            <tr class="whitespace-nowrap border-b border-[var(--theme-border-default)] text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
              <th
                v-for="col in visibleCols"
                :key="col.key"
                class="select-none px-1.5 py-1.5 font-semibold"
                :class="[
                  col.align === 'right' ? 'text-right' : 'text-left',
                  col.sortable ? 'cursor-pointer hover:text-[var(--theme-text-primary)]' : '',
                  col.key === 'session' ? 'min-w-60' : '',
                ]"
                @click="col.sortable && col.sortField && cycleSort(col.sortField)"
              >
                <span class="inline-flex items-center gap-0.5" :class="col.align === 'right' ? 'flex-row-reverse' : ''">
                  {{ t(locale, col.labelKey) }}
                  <template v-if="col.sortable && col.sortField && isSortedBy(col.sortField)">
                    <ArrowUp v-if="!sortDesc" class="h-2.5 w-2.5" aria-hidden="true" />
                    <ArrowDown v-else class="h-2.5 w-2.5" aria-hidden="true" />
                  </template>
                </span>
              </th>
            </tr>
          </thead>
          <tbody>
            <template v-if="store.sessionsLoading && store.sessions.length === 0">
              <tr
                v-for="row in skeletonRows"
                :key="`sk-${row}`"
                class="border-b border-[var(--theme-border-subtle)] last:border-0"
              >
                <td v-for="n in visibleCols.length" :key="n" class="px-1.5 py-1.5">
                  <div class="h-2.5 animate-pulse rounded bg-[var(--theme-border-default)]" :style="{ width: `${40 + ((row + n) % 5) * 12}%` }"></div>
                </td>
              </tr>
            </template>
            <tr v-else-if="paginatedSessions.length === 0">
              <td :colspan="visibleCols.length" class="px-3 py-12 text-center text-xs text-[var(--theme-text-tertiary)]">
                {{ store.sessions.length === 0 ? t(locale, 'desktop.sessions.noSessions') : t(locale, 'desktop.sessions.noMatch') }}
              </td>
            </tr>
            <tr
              v-for="session in paginatedSessions"
              :key="session.sessionId"
              tabindex="0"
              class="cursor-pointer border-b border-[var(--theme-border-subtle)] transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-ring-focus)]"
              :class="selectedSession?.sessionId === session.sessionId && drawerOpen ? 'bg-[var(--theme-accent-soft)]' : ''"
              @click="openDrawer(session)"
              @contextmenu.prevent="handleRowContextMenu($event, session)"
              @keydown="handleRowKeydown($event, session); handleRowMenuKeydown($event, session)"
            >
              <td v-if="isColumnVisible('session')" class="max-w-72 px-1.5 py-1.5">
                <div class="flex min-w-0 items-center gap-2">
                  <LobeIcon v-if="getToolIcon(session.tool)" :slug="getToolIcon(session.tool) ?? 'claudecode'" :size="14" @error="() => {}" />
                  <span v-else class="h-2 w-2 shrink-0 rounded-full bg-[var(--theme-border-strong)]"></span>
                  <div class="min-w-0">
                    <p class="truncate font-medium text-[var(--theme-text-primary)]">{{ displaySessionTitle(session) }}</p>
                    <div class="flex items-center gap-1">
                      <span v-if="displaySessionProjectBadge(session)" class="truncate rounded px-1 py-px text-xs font-semibold leading-none" :class="projectBadgeClasses(session.projectIdentity)">
                        {{ displaySessionProjectBadge(session) }}
                      </span>
                      <span v-if="session.wslDistro" class="truncate rounded bg-cyan-500/10 px-1 py-px text-xs font-semibold leading-none text-cyan-600 dark:text-cyan-300" :title="t(locale, 'sessions.wslBadgeTitle', { distro: session.wslDistro })">{{ session.wslDistro }}</span>
                      <span v-if="sessionUsageVisible(session) && (session.uncoveredRequests ?? 0) > 0" class="rounded bg-amber-500/10 px-1 py-px text-xs font-semibold leading-none text-amber-600 dark:text-amber-300" :title="t(locale, 'desktop.sessions.partialCoverageTitle')">
                        {{ t(locale, 'desktop.sessions.partialCoverage') }}
                      </span>
                    </div>
                  </div>
                </div>
              </td>
              <td v-if="isColumnVisible('lastActive')" class="whitespace-nowrap px-1.5 py-1.5 text-[var(--theme-text-secondary)]">{{ formatTime(session.lastRequestTime) }}</td>
              <td v-if="isColumnVisible('model')" class="max-w-44 truncate px-1.5 py-1.5 font-mono text-xs text-[var(--theme-text-secondary)]" :title="session.models.join(', ')">
                {{ sessionModelLabel(session) }}<span v-if="session.models.length > 1" class="text-[var(--theme-text-quaternary)]"> +{{ session.models.length - 1 }}</span>
              </td>
              <td v-if="isColumnVisible('requests')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ session.totalRequests ?? 0 }}</td>
              <td v-if="isColumnVisible('tokens')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ sessionUsageVisible(session) ? formatTokens(sessionTotalTokens(session)) : '—' }}</td>
              <td v-if="isColumnVisible('cost')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ sessionUsageVisible(session) ? formatCost(session.estimatedCost) : '—' }}</td>
              <td v-if="isColumnVisible('rate')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">
                {{ sessionUsageVisible(session) && (session.avgOutputTokensPerSecond || 0) > 0 ? `${session.avgOutputTokensPerSecond.toFixed(1)}t/s` : '—' }}
              </td>
              <td v-if="isColumnVisible('errors')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono" :class="(session.errorRequests ?? 0) > 0 ? 'text-red-500' : 'text-[var(--theme-text-secondary)]'">
                {{ sessionUsageVisible(session) ? (session.errorRequests ?? 0) : '—' }}
              </td>
              <td v-if="isColumnVisible('duration')" class="whitespace-nowrap px-1.5 py-1.5 text-right font-mono text-[var(--theme-text-secondary)]">{{ formatDuration(session.totalDurationMs) }}</td>
            </tr>
          </tbody>
        </table>
      </div>

      <!-- 覆盖式抽屉 -->
      <Transition name="drawer-overlay">
        <div v-if="drawerOpen" class="absolute inset-0 z-20 flex justify-end" @click.self="closeDrawer">
          <aside
            class="theme-surface-elevated flex h-full w-80 flex-col overflow-y-auto rounded-xl border shadow-[0_4px_24px_rgba(0,0,0,0.08)]"
            :aria-label="t(locale, 'desktop.sessions.drawerTitle')"
          >
            <div class="flex items-start justify-between gap-2 border-b border-[var(--theme-border-default)] px-3 py-2.5">
              <div class="min-w-0">
                <div class="mb-1 flex items-center gap-1.5">
                  <span v-if="displaySessionProjectBadge(selectedSession!)" class="rounded px-1 py-px text-xs font-semibold leading-none" :class="projectBadgeClasses(selectedSession!.projectIdentity)">
                    {{ displaySessionProjectBadge(selectedSession!) }}
                  </span>
                  <span class="text-xs text-[var(--theme-text-tertiary)]">{{ formatTime(selectedSession!.lastRequestTime) }}</span>
                </div>
                <h3 class="truncate text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ displaySessionTitle(selectedSession!) }}</h3>
                <p class="mt-0.5 truncate text-xs text-[var(--theme-text-tertiary)]">
                  {{ selectedSession!.projectName || t(locale, 'common.unknownProject') }}
                  · {{ requestToolLabel(selectedSession!.tool) }}
                </p>
              </div>
              <div class="flex shrink-0 items-start gap-1">
                <button
                  type="button"
                  class="inline-flex items-center gap-1 rounded-lg border border-[var(--theme-border-default)] px-2 py-1 text-xs font-semibold text-[var(--theme-text-secondary)] transition-colors hover:border-[var(--theme-accent-primary)] hover:text-[var(--theme-accent-primary)]"
                  :title="t(locale, 'desktop.sessions.openWorkspaceHint')"
                  @click="openInWorkspace"
                >
                  <ExternalLink class="h-3 w-3" aria-hidden="true" />
                  {{ t(locale, 'desktop.sessions.menuOpenSession') }}
                </button>
                <button
                  type="button"
                  class="shrink-0 rounded-lg p-1 transition-colors hover:bg-[var(--theme-bg-hover)]"
                  :aria-label="t(locale, 'common.close')"
                  :title="t(locale, 'common.close')"
                  @click="closeDrawer"
                >
                  <X class="h-4 w-4 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
                </button>
              </div>
            </div>

            <div class="space-y-3 p-3">
              <!-- 概览指标 -->
              <div class="grid grid-cols-3 gap-2">
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnRequests') }}</div>
                  <div class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">{{ selectedSession!.totalRequests ?? 0 }}</div>
                </div>
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'common.totalTokens') }}</div>
                  <div class="font-mono text-xs font-semibold text-[var(--theme-text-primary)]">{{ sessionUsageVisible(selectedSession!) ? formatTokens(sessionTotalTokens(selectedSession!)) : '—' }}</div>
                </div>
                <div class="theme-surface-muted rounded-lg border px-2 py-1.5 text-center">
                  <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.cost') }}</div>
                  <div class="font-mono text-xs font-semibold text-[var(--theme-chart-cost)]">{{ sessionUsageVisible(selectedSession!) ? formatCost(selectedSession!.estimatedCost) : '—' }}</div>
                </div>
              </div>

              <!-- Token 分项 -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.input') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatTokens(selectedSession!.totalInputTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.output') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatTokens(selectedSession!.totalOutputTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.cacheCreate') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatTokens(selectedSession!.totalCacheCreateTokens) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'statistics.cacheRead') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatTokens(selectedSession!.totalCacheReadTokens) }}</span></div>
              </section>

              <!-- 性能 -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'metrics.tokensPerSecond') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ sessionUsageVisible(selectedSession!) && (selectedSession!.avgOutputTokensPerSecond || 0) > 0 ? `${selectedSession!.avgOutputTokensPerSecond.toFixed(1)}t/s` : '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.ttft') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ selectedSession!.avgTtftMs != null ? formatDuration(selectedSession!.avgTtftMs) : '—' }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.duration') }}</span><span class="font-mono text-xs text-[var(--theme-text-primary)]">{{ formatDuration(selectedSession!.totalDurationMs) }}</span></div>
                <div class="flex items-center justify-between py-1"><span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.errors') }}</span><span class="font-mono text-xs" :class="(selectedSession!.errorRequests ?? 0) > 0 ? 'text-red-500' : 'text-[var(--theme-text-primary)]'">{{ selectedSession!.errorRequests ?? 0 }}</span></div>
              </section>

              <!-- 模型列表 -->
              <section v-if="selectedSession!.models.length > 0" class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="mb-1 text-xs font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.workspace.models') }}</div>
                <div class="flex flex-wrap gap-1">
                  <span v-for="model in selectedSession!.models" :key="model" class="rounded bg-[var(--theme-border-subtle)] px-1.5 py-px font-mono text-xs text-[var(--theme-text-secondary)]">{{ model }}</span>
                </div>
              </section>

              <!-- 标识信息 -->
              <section class="theme-surface-muted rounded-lg border px-3 py-2">
                <div class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.sessionId') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedSession!.sessionId">{{ shortId(selectedSession!.sessionId) }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.copyId')" :title="t(locale, 'desktop.sessions.copyId')" @click="copyText(selectedSession!.sessionId, 'id')"><ExternalLink class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div v-if="selectedSession!.cwd" class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'settings.cwd') }}</span>
                  <span class="flex min-w-0 items-center gap-1">
                    <span class="truncate font-mono text-xs text-[var(--theme-text-secondary)]" :title="selectedSession!.cwd">{{ selectedSession!.cwd }}</span>
                    <button type="button" class="shrink-0 rounded p-0.5 text-[var(--theme-text-quaternary)] hover:text-[var(--theme-text-primary)]" :aria-label="t(locale, 'desktop.sessions.menuCopyCwd')" :title="t(locale, 'desktop.sessions.menuCopyCwd')" @click="copyText(selectedSession!.cwd!, 'cwd')"><ExternalLink class="h-3 w-3" aria-hidden="true" /></button>
                  </span>
                </div>
                <div v-if="selectedSession!.topic" class="flex items-center justify-between gap-2 py-1">
                  <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'sessions.lastPrompt') }}</span>
                  <span class="truncate text-xs text-[var(--theme-text-secondary)]" :title="selectedSession!.topic">{{ selectedSession!.topic }}</span>
                </div>
              </section>

              <p v-if="copiedValue" class="text-center text-xs font-medium text-emerald-600 dark:text-emerald-300">{{ t(locale, 'desktop.sessions.copied') }}</p>
            </div>
          </aside>
        </div>
      </Transition>
    </div>

    <!-- 分页控件 -->
    <PaginationBar
      :current-page="currentPage"
      :total-pages="totalPages"
      :total="total"
      :page-numbers="pageNumbers"
      :has-prev="hasPrev"
      :has-next="hasNext"
      :page-size="pageSize"
      :page-size-options="pageSizeSelectOptions"
      :page-size-label="t(locale, 'desktop.requests.pageSize')"
      :page-first-label="t(locale, 'desktop.requests.pageFirst')"
      :page-prev-label="t(locale, 'desktop.requests.pagePrev')"
      :page-next-label="t(locale, 'desktop.requests.pageNext')"
      :page-last-label="t(locale, 'desktop.requests.pageLast')"
      :jump-to-label="t(locale, 'desktop.requests.jumpTo')"
      :page-of-label="t(locale, 'desktop.requests.pageOf', { current: '', total: totalPages })"
      @goto="gotoPage"
      @next="nextPage"
      @prev="prevPage"
      @update:page-size="onPageSizeUpdate"
    >
      <template #extra>
        <span v-if="loadingMore" class="text-xs text-[var(--theme-text-quaternary)]">{{ t(locale, 'common.syncing') }}</span>
      </template>
    </PaginationBar>
  </div>

  <!-- 右键菜单 -->
  <Teleport to="body">
    <div
      v-if="contextMenu"
      ref="contextMenuRef"
      role="menu"
      class="theme-surface-elevated fixed z-[80] w-44 rounded-xl border p-1 shadow-xl"
      :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
      @keydown="handleMenuKeydown"
    >
      <button
        type="button"
        role="menuitem"
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)]"
        @click="runContextAction('open')"
      >
        {{ t(locale, 'desktop.sessions.menuOpenSession') }}
      </button>
      <button
        type="button"
        role="menuitem"
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)]"
        @click="runContextAction('copyId')"
      >
        {{ t(locale, 'desktop.sessions.menuCopyId') }}
      </button>
      <button
        type="button"
        role="menuitem"
        :disabled="!contextMenu.session.cwd"
        :title="contextMenu.session.cwd ? '' : t(locale, 'desktop.sessions.menuCopyCwdDisabled')"
        class="block w-full rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-[var(--theme-text-secondary)] outline-none transition-colors hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus-visible:ring-1 focus-visible:ring-[var(--theme-ring-focus)] disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent"
        @click="runContextAction('copyCwd')"
      >
        {{ t(locale, 'desktop.sessions.menuCopyCwd') }}
      </button>
    </div>

    <div
      v-if="copiedValue"
      class="pointer-events-none fixed bottom-5 left-1/2 z-[90] -translate-x-1/2 rounded-full border border-[var(--theme-border-default)] bg-[var(--theme-bg-elevated)] px-3.5 py-1.5 text-xs font-medium text-emerald-600 shadow-lg dark:text-emerald-300"
    >
      {{ t(locale, 'desktop.sessions.copied') }}
    </div>
  </Teleport>
</template>

<style scoped>
.drawer-overlay-enter-active,
.drawer-overlay-leave-active {
  transition: opacity 0.15s ease-out;
}
.drawer-overlay-enter-active > aside,
.drawer-overlay-leave-active > aside {
  transition: transform 0.18s ease-out;
}
.drawer-overlay-enter-from,
.drawer-overlay-leave-to {
  opacity: 0;
}
.drawer-overlay-enter-from > aside,
.drawer-overlay-leave-to > aside {
  transform: translateX(100%);
}
</style>
