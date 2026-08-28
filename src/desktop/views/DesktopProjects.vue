<script setup lang="ts">
/**
 * 桌面主窗口「项目」页（master-detail 两栏）。
 * 左侧项目列表（可搜索、可滚动）+ 右侧项目摘要（统计 / 工具构成 / 模型构成 / 会话列表）。
 * 布局与请求页/会话页一致：flex h-full flex-col，工具栏 + 内容区各独立滚动。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { FileQuestionMark, Folder, Globe } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { t } from '../../i18n'
import type { ProjectStats, SessionStats } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useSessionViewData } from '../../composables/useSessionViewData'
import LobeIcon from '../../components/LobeIcon.vue'
import DesktopSelect from '../components/DesktopSelect.vue'
import type { SelectOption } from '../components/DesktopSelect.vue'
import SearchInput from '../components/SearchInput.vue'

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

const {
  formatTime,
  formatTokens,
  formatCost,
  requestToolLabel,
  sessionUsageVisible,
  displaySessionTitle,
  displayProjectName,
  getToolIcon,
} = useSessionDisplay(store)

const activeTab = ref<'recent' | 'requests' | 'projects'>('projects')
const {
  selectedTool,
  reloadProjectStats,
  initialize,
  dispose,
} = useSessionViewData(store, activeTab)

// —— 会话搜索（作用于项目内会话列表） ——
const searchQuery = ref('')

const matchesSearch = (session: SessionStats): boolean => {
  const q = searchQuery.value.trim().toLowerCase()
  if (!q) return true
  return [
    displaySessionTitle(session),
    session.topic,
    session.lastPrompt,
    session.projectName,
    session.cwd,
    session.sessionId
  ].some(value => value?.toLowerCase().includes(q))
}

const filteredSessions = computed(() => {
  const list = store.sessions.filter(session => matchesSearch(session))
  return [...list].sort((a, b) => (b.lastRequestTime || 0) - (a.lastRequestTime || 0))
})

// —— 项目搜索（作用于左侧项目列表） ——
const projectSearchQuery = ref('')

// —— 项目视图（master-detail） ——
const selectedProjectKey = ref<string | null>(null)
const projectKeyOf = (project: ProjectStats) => project.projectKey || project.projectPath || project.name

const projectList = computed(() => {
  const sorted = [...store.projectStats].sort((a, b) => (b.lastActive || 0) - (a.lastActive || 0))
  const q = projectSearchQuery.value.trim().toLowerCase()
  if (!q) return sorted.slice(0, 200)
  return sorted.filter(p => {
    return [
      displayProjectName(p),
      p.projectPath,
      p.name,
    ].some(value => value?.toLowerCase().includes(q))
  }).slice(0, 200)
})
const projectTruncated = computed(() => {
  const q = projectSearchQuery.value.trim().toLowerCase()
  const total = q
    ? store.projectStats.filter(p => [displayProjectName(p), p.projectPath, p.name].some(v => v?.toLowerCase().includes(q))).length
    : store.projectStats.length
  return total > 200
})

const selectProject = (project: ProjectStats) => { selectedProjectKey.value = projectKeyOf(project) }
const selectedProject = computed(() => store.projectStats.find(project => projectKeyOf(project) === selectedProjectKey.value) ?? null)

// 首次加载后默认选中第一个项目
watch(projectList, list => {
  if (!selectedProjectKey.value && list.length > 0) {
    selectedProjectKey.value = projectKeyOf(list[0])
  }
})

const projectTotalTokens = (project: ProjectStats) =>
  (project.totalInputTokens || 0) + (project.totalOutputTokens || 0) + (project.totalCacheCreateTokens || 0) + (project.totalCacheReadTokens || 0)

const sessionTotalTokens = (session: SessionStats) =>
  (session.totalInputTokens || 0) + (session.totalOutputTokens || 0) + (session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)

const sessionsOfProject = computed(() => {
  const project = selectedProject.value
  if (!project) return []
  return filteredSessions.value.filter(session => {
    if (project.projectIdentity === 'global') return session.projectIdentity === 'global'
    if (project.projectIdentity === 'unknown') return session.projectIdentity === 'unknown'
    return !!session.projectName && session.projectName === project.name
  })
})

const modelsOfProject = computed(() => {
  const counts = new Map<string, number>()
  for (const session of sessionsOfProject.value) {
    const model = session.models[0]
    if (!model) continue
    counts.set(model, (counts.get(model) ?? 0) + 1)
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12)
})

const openWorkspace = (session: SessionStats) => {
  nav.openSession(session.sessionId)
}

const toolOptions = computed(() => {
  const tools = new Set<string>()
  for (const session of store.sessions) tools.add(session.tool)
  for (const request of store.requestRecords) tools.add(request.tool)
  return [...tools].sort((a, b) => a.localeCompare(b))
})

const toolSelectOptions = computed<SelectOption[]>(() => [
  { value: null, label: t(locale.value, 'desktop.allTools') },
  ...toolOptions.value.map(tool => ({ value: tool, label: requestToolLabel(tool) }))
])

const projectCount = computed(() => store.projectStats.length)

async function applyPendingFilters() {
  const pending = nav.consumePendingFilters()
  if (!pending) return
  if (pending.sourceId && store.settings.sourceAware.activeSourceFilter !== pending.sourceId) {
    await store.setActiveSourceFilter(pending.sourceId)
  }
  if (pending.tool && store.settings.clientTools.activeToolFilter !== pending.tool) {
    await store.setActiveToolFilter(pending.tool)
  }
  if (pending.sessionKey && !nav.activeSessionKey) {
    nav.openSession(pending.sessionKey)
  }
}

watch(
  () => nav.pendingConsumeTick,
  () => {
    if (nav.currentPage === 'projects') void applyPendingFilters()
  }
)

onMounted(async () => {
  await applyPendingFilters()
  await initialize()
  await reloadProjectStats()
})

onUnmounted(() => {
  dispose()
})
</script>

<template>
  <div class="flex h-full flex-col gap-3">
    <!-- 工具栏 -->
    <div class="flex shrink-0 flex-wrap items-center gap-2 text-xs">
      <DesktopSelect
        v-model="selectedTool"
        :options="toolSelectOptions"
        :aria-label="t(locale, 'desktop.sessions.filterTool')"
      />
      <span class="ml-auto shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.requests.totalRecords', { count: projectCount }) }}</span>
    </div>

    <!-- 项目视图（master-detail 两栏） -->
    <div v-if="store.projectStatsLoading && store.projectStats.length === 0" class="flex flex-1 items-center justify-center">
      <div class="h-5 w-5 animate-spin rounded-full border-2 border-[var(--theme-border-strong)] border-t-[var(--theme-accent-primary)]"></div>
    </div>
    <div v-else-if="store.projectStats.length === 0" class="flex flex-1 items-center justify-center text-xs text-[var(--theme-text-tertiary)]">
      {{ t(locale, 'desktop.sessions.noProjects') }}
    </div>
    <div v-else class="flex min-h-0 flex-1 gap-3">
      <!-- 左：项目列表 -->
      <div class="flex w-72 shrink-0 flex-col overflow-hidden rounded-lg border" style="background: var(--theme-surface-gradient)">
        <!-- 项目搜索 -->
        <div class="shrink-0 border-b border-[var(--theme-border-default)] p-2">
          <SearchInput v-model="projectSearchQuery" width="w-full" :placeholder="t(locale, 'desktop.sessions.searchPlaceholder')" />
        </div>
        <!-- 列表标题 -->
        <div class="shrink-0 px-3 py-1.5 text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.sessions.projectsColumn') }}
          <span v-if="projectTruncated" class="ml-1 normal-case text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.sessions.projectsTruncated', { count: 200 }) }}</span>
        </div>
        <!-- 项目列表（滚动） -->
        <div class="min-h-0 flex-1 overflow-y-auto">
          <button
            v-for="project in projectList"
            :key="projectKeyOf(project)"
            type="button"
            class="relative flex w-full items-start gap-2 border-b border-[var(--theme-border-subtle)] px-3 py-2 text-left transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-ring-focus)]"
            :class="selectedProjectKey === projectKeyOf(project) ? 'bg-[var(--theme-accent-soft)]' : ''"
            @click="selectProject(project)"
          >
            <span
              v-if="selectedProjectKey === projectKeyOf(project)"
              class="absolute left-0 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-r-full"
              style="background: var(--theme-accent-primary)"
            ></span>
            <span v-if="project.projectIdentity === 'global'" class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-slate-500/10 text-slate-500 dark:text-slate-300"><Globe class="h-3.5 w-3.5" aria-hidden="true" /></span>
            <span v-else-if="project.projectIdentity === 'unknown'" class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-amber-500/10 text-amber-600 dark:text-amber-300"><FileQuestionMark class="h-3.5 w-3.5" aria-hidden="true" /></span>
            <span v-else class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-indigo-500/10 text-indigo-500 dark:text-indigo-300"><Folder class="h-3.5 w-3.5" aria-hidden="true" /></span>
            <span class="min-w-0 flex-1">
              <span class="flex items-center justify-between gap-2">
                <span class="truncate text-xs font-semibold text-[var(--theme-text-primary)]">{{ displayProjectName(project) }}</span>
                <span class="shrink-0 text-xs text-[var(--theme-text-quaternary)]">{{ formatTime(project.lastActive) }}</span>
              </span>
              <span v-if="project.projectPath" class="mt-0.5 block truncate font-mono text-xs text-[var(--theme-text-tertiary)]">{{ project.projectPath }}</span>
              <span class="mt-1 flex items-center gap-2 text-xs text-[var(--theme-text-secondary)]">
                <span class="font-mono">{{ project.sessionCount ?? 0 }} {{ t(locale, 'desktop.sessions.projectSessions') }}</span>
                <span class="text-[var(--theme-text-quaternary)]">·</span>
                <span class="font-mono">{{ formatTokens(projectTotalTokens(project)) }}</span>
                <span class="text-[var(--theme-text-quaternary)]">·</span>
                <span class="font-mono text-[var(--theme-chart-cost)]">{{ formatCost(project.totalCost) }}</span>
              </span>
            </span>
          </button>
        </div>
      </div>

      <!-- 右：项目摘要（独立滚动） -->
      <div class="min-w-0 flex-1 overflow-y-auto">
        <template v-if="selectedProject">
          <!-- 项目标题 -->
          <div class="mb-3 rounded-lg border p-4" style="background: var(--theme-surface-gradient)">
            <div class="flex flex-wrap items-center gap-2">
              <h2 class="text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ displayProjectName(selectedProject) }}</h2>
              <span v-if="selectedProject.projectIdentity === 'global'" class="rounded bg-slate-500/10 px-1.5 py-0.5 text-xs font-semibold text-slate-500 dark:text-slate-300">{{ t(locale, 'common.global') }}</span>
              <span v-else-if="selectedProject.projectIdentity === 'unknown'" class="rounded bg-amber-500/10 px-1.5 py-0.5 text-xs font-semibold text-amber-600 dark:text-amber-300">{{ t(locale, 'common.unknownProject') }}</span>
              <span class="ml-auto text-xs text-[var(--theme-text-tertiary)]">{{ formatTime(selectedProject.lastActive) }}</span>
            </div>
            <p v-if="selectedProject.projectPath" class="mt-1 truncate font-mono text-xs text-[var(--theme-text-tertiary)]">{{ selectedProject.projectPath }}</p>
          </div>

          <!-- 汇总统计 -->
          <div class="mb-3 grid grid-cols-2 gap-2 sm:grid-cols-4">
            <div class="rounded-lg border p-3" style="background: var(--theme-surface-gradient)">
              <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.projectSessions') }}</div>
              <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ selectedProject.sessionCount ?? 0 }}</div>
            </div>
            <div class="rounded-lg border p-3" style="background: var(--theme-surface-gradient)">
              <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnRequests') }}</div>
              <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ selectedProject.requestCount ?? 0 }}</div>
            </div>
            <div class="rounded-lg border p-3" style="background: var(--theme-surface-gradient)">
              <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnTokens') }}</div>
              <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(projectTotalTokens(selectedProject)) }}</div>
            </div>
            <div class="rounded-lg border p-3" style="background: var(--theme-surface-gradient)">
              <div class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnCost') }}</div>
              <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-chart-cost)]">{{ formatCost(selectedProject.totalCost) }}</div>
            </div>
          </div>

          <!-- 工具构成 -->
          <div class="mb-3 rounded-lg border" style="background: var(--theme-surface-gradient)">
            <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.toolComposition') }}</div>
            <div v-if="!selectedProject.toolBreakdown || selectedProject.toolBreakdown.length === 0" class="px-3 py-4 text-center text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noToolData') }}</div>
            <table v-else class="w-full text-xs">
              <thead>
                <tr class="whitespace-nowrap border-b border-[var(--theme-border-subtle)] text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
                  <th class="px-3 py-1.5 text-left">{{ t(locale, 'sessions.tool') }}</th>
                  <th class="px-3 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnRequests') }}</th>
                  <th class="px-3 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnTokens') }}</th>
                  <th class="px-3 py-1.5 text-right">{{ t(locale, 'desktop.sessions.columnCost') }}</th>
                  <th class="px-3 py-1.5 text-right">{{ t(locale, 'sessions.lastActive') }}</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="tool in selectedProject.toolBreakdown" :key="tool.tool" class="border-b border-[var(--theme-border-subtle)] last:border-0">
                  <td class="px-3 py-1.5">
                    <span class="flex items-center gap-1.5">
                      <LobeIcon v-if="getToolIcon(tool.tool)" :slug="getToolIcon(tool.tool) ?? 'claudecode'" :size="12" @error="() => {}" />
                      <span v-else class="h-2 w-2 rounded-full bg-[var(--theme-border-strong)]"></span>
                      <span class="truncate text-[var(--theme-text-secondary)]">{{ requestToolLabel(tool.tool) }}</span>
                    </span>
                  </td>
                  <td class="whitespace-nowrap px-3 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ tool.requestCount ?? 0 }}</td>
                  <td class="whitespace-nowrap px-3 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens((tool.totalInputTokens || 0) + (tool.totalOutputTokens || 0) + (tool.totalCacheCreateTokens || 0) + (tool.totalCacheReadTokens || 0)) }}</td>
                  <td class="whitespace-nowrap px-3 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(tool.totalCost) }}</td>
                  <td class="whitespace-nowrap px-3 py-1.5 text-right text-[var(--theme-text-tertiary)]">{{ formatTime(tool.lastActive) }}</td>
                </tr>
              </tbody>
            </table>
          </div>

          <!-- 模型构成 -->
          <div class="mb-3 rounded-lg border" style="background: var(--theme-surface-gradient)">
            <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.modelComposition') }}</div>
            <div v-if="modelsOfProject.length === 0" class="px-3 py-4 text-center text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noModelData') }}</div>
            <div v-else class="space-y-1.5 px-3 py-2.5">
              <div v-for="[model, count] in modelsOfProject" :key="model" class="flex items-center gap-2 text-xs">
                <span class="min-w-0 flex-1 truncate font-mono text-[var(--theme-text-secondary)]">{{ model }}</span>
                <span class="h-1.5 w-24 shrink-0 rounded-full bg-[var(--theme-border-subtle)]">
                  <span class="block h-full rounded-full bg-[var(--theme-accent-primary)]" :style="{ width: `${(count / (modelsOfProject[0]?.[1] ?? 1)) * 100}%` }"></span>
                </span>
                <span class="w-10 shrink-0 text-right font-mono text-[var(--theme-text-primary)]">{{ count }}</span>
              </div>
            </div>
          </div>

          <!-- 会话列表 -->
          <div class="rounded-lg border" style="background: var(--theme-surface-gradient)">
            <div class="flex items-center justify-between border-b border-[var(--theme-border-default)] px-3 py-2">
              <span class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.projectSessionsList') }}</span>
              <span class="text-xs text-[var(--theme-text-quaternary)]">{{ sessionsOfProject.length }}</span>
            </div>
            <!-- 会话搜索 -->
            <div class="border-b border-[var(--theme-border-subtle)] p-2">
              <SearchInput v-model="searchQuery" width="w-full" :placeholder="t(locale, 'desktop.sessions.searchPlaceholder')" />
            </div>
            <div v-if="sessionsOfProject.length === 0" class="px-3 py-4 text-center text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noSessions') }}</div>
            <button
              v-for="session in sessionsOfProject"
              :key="session.sessionId"
              type="button"
              tabindex="0"
              class="flex w-full items-center gap-2 border-b border-[var(--theme-border-subtle)] px-3 py-1.5 text-left transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-ring-focus)]"
              :title="t(locale, 'desktop.sessions.openWorkspaceHint')"
              @click="openWorkspace(session)"
            >
              <span class="min-w-0 flex-1 truncate text-xs font-medium text-[var(--theme-text-primary)]">{{ displaySessionTitle(session) }}</span>
              <span class="shrink-0 text-xs text-[var(--theme-text-tertiary)]">{{ formatTime(session.lastRequestTime) }}</span>
              <span class="w-16 shrink-0 text-right font-mono text-xs text-[var(--theme-text-secondary)]">{{ session.totalRequests ?? 0 }}</span>
              <span class="w-20 shrink-0 text-right font-mono text-xs text-[var(--theme-text-secondary)]">{{ sessionUsageVisible(session) ? formatTokens(sessionTotalTokens(session)) : '—' }}</span>
            </button>
          </div>
        </template>
        <div v-else class="flex h-full items-center justify-center text-xs text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.sessions.selectProjectHint') }}
        </div>
      </div>
    </div>
  </div>
</template>
