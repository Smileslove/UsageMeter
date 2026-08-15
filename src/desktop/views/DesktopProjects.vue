<script setup lang="ts">
/**
 * 桌面主窗口「项目」页（master-detail 两栏）。
 * 由 DesktopSessions.vue 的项目视图拆分而来：左侧项目列表 + 右侧项目摘要
 * （统计卡片 / 工具构成 / 模型构成 / 项目内会话列表）。
 * 数据加载复用 useSessionViewData（store.fetchSessionsForTool / fetchProjectStatsForTool），
 * activeTab 固定为 'projects'；onMounted 显式调用 reloadProjectStats 加载项目统计。
 * 项目内会话双击打开工作区：nav.openSession 跳回会话页（hash 由 desktopNavigation 路由处理）。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { FileQuestionMark, Folder, Globe, Search } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t } from '../../i18n'
import type { ProjectStats, SessionStats } from '../../types'
import { useSessionDisplay } from '../../composables/useSessionDisplay'
import { useSessionViewData } from '../../composables/useSessionViewData'
import LobeIcon from '../../components/LobeIcon.vue'

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
  displayProjectHint,
  getToolIcon,
} = useSessionDisplay(store)

// —— 视图数据（activeTab 固定为 'projects'；selectedTool 切换触发后端按工具重载） ——
const activeTab = ref<'recent' | 'requests' | 'projects'>('projects')
const {
  selectedTool,
  reloadProjectStats,
  initialize,
  dispose,
} = useSessionViewData(store, activeTab)

// —— 搜索（无全文索引：仅标题 / topic / 项目 / cwd；作用于项目内会话列表） ——
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

// —— 项目视图（master-detail） ——
const selectedProjectKey = ref<string | null>(null)
const projectList = computed(() => {
  const list = [...store.projectStats].sort((a, b) => (b.lastActive || 0) - (a.lastActive || 0))
  return list.slice(0, 200) // 超长保护：最多渲染 200 个
})
const projectTruncated = computed(() => store.projectStats.length > 200)
const projectKeyOf = (project: ProjectStats) => project.projectKey || project.projectPath || project.name
const selectProject = (project: ProjectStats) => { selectedProjectKey.value = projectKeyOf(project) }
const selectedProject = computed(() => store.projectStats.find(project => projectKeyOf(project) === selectedProjectKey.value) ?? null)

const projectTotalTokens = (project: ProjectStats) =>
  (project.totalInputTokens || 0) + (project.totalOutputTokens || 0) + (project.totalCacheCreateTokens || 0) + (project.totalCacheReadTokens || 0)

const sessionTotalTokens = (session: SessionStats) =>
  (session.totalInputTokens || 0) + (session.totalOutputTokens || 0) + (session.totalCacheCreateTokens || 0) + (session.totalCacheReadTokens || 0)

// 项目详情中的会话（预设项目过滤，受搜索框影响）
const sessionsOfProject = computed(() => {
  const project = selectedProject.value
  if (!project) return []
  return filteredSessions.value.filter(session => {
    if (project.projectIdentity === 'global') return session.projectIdentity === 'global'
    if (project.projectIdentity === 'unknown') return session.projectIdentity === 'unknown'
    return !!session.projectName && session.projectName === project.name
  })
})

// 项目详情中的模型构成（从该项目会话聚合）
const modelsOfProject = computed(() => {
  const counts = new Map<string, number>()
  for (const session of sessionsOfProject.value) {
    const model = session.models[0]
    if (!model) continue
    counts.set(model, (counts.get(model) ?? 0) + 1)
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 12)
})

// —— 项目内会话双击打开工作区：跳回会话页（hash 路由由 desktopNavigation 处理） ——
const openWorkspace = (session: SessionStats) => {
  nav.openSession(session.sessionId)
}

// —— 工具筛选选项（从已加载的会话 / 请求记录取已见工具） ——
const toolOptions = computed(() => {
  const tools = new Set<string>()
  for (const session of store.sessions) tools.add(session.tool)
  for (const request of store.requestRecords) tools.add(request.tool)
  return [...tools].sort((a, b) => a.localeCompare(b))
})

/** 深链/事件导航带来的全局筛选上下文（sourceId/tool）应用；sessionKey 走会话页 hash 路由。 */
async function applyPendingFilters() {
  const pending = nav.consumePendingFilters()
  if (!pending) return
  if (pending.sourceId && store.settings.sourceAware.activeSourceFilter !== pending.sourceId) {
    await store.setActiveSourceFilter(pending.sourceId)
  }
  if (pending.tool && store.settings.clientTools.activeToolFilter !== pending.tool) {
    await store.setActiveToolFilter(pending.tool)
  }
  // 深链携带 sessionKey 时直接打开对应会话工作区（复用 openSession 的 hash/恢复逻辑）
  if (pending.sessionKey && !nav.activeSessionKey) {
    nav.openSession(pending.sessionKey)
  }
}

// 同页深链：hash 相同页面不重挂载，onMounted 消费路径不执行；
// pendingConsumeTick 变化时若本页激活则补消费（跨页场景由 onMounted 覆盖，这里幂等）。
watch(
  () => nav.pendingConsumeTick,
  () => {
    if (nav.currentPage === 'projects') void applyPendingFilters()
  }
)

onMounted(async () => {
  // 深链/事件导航带来的全局筛选上下文（sourceId/tool）先应用
  await applyPendingFilters()
  // hook 的 initialize 只 reloadSessions（供项目内会话列表过滤）；项目统计必须显式加载
  await initialize()
  await reloadProjectStats()
})

onUnmounted(() => {
  dispose()
})
</script>

<template>
  <div class="flex flex-col gap-3 pb-4">
    <!-- 工具栏：搜索 + 工具下拉（切换触发后端按工具重载项目统计） -->
    <div class="flex flex-wrap items-center gap-2">
      <div class="relative min-w-0 flex-1 basis-56">
        <!-- 搜索框：作用于项目内会话列表（仅标题 / topic / 项目 / cwd 范围） -->
        <Search class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <input
          v-model="searchQuery"
          type="search"
          class="theme-input h-8 w-full rounded-lg pl-8 pr-3 text-[12px] outline-none"
          :placeholder="t(locale, 'desktop.sessions.searchPlaceholder')"
          :aria-label="t(locale, 'desktop.sessions.searchPlaceholder')"
        />
      </div>

      <!-- 工具下拉（复用 useSessionViewData 的 selectedTool，切换触发后端按工具重载） -->
      <select
        v-model="selectedTool"
        class="theme-input ml-auto h-8 max-w-40 rounded-lg px-2 text-[12px] outline-none"
        :aria-label="t(locale, 'desktop.sessions.filterTool')"
      >
        <option :value="null">{{ t(locale, 'desktop.allTools') }}</option>
        <option v-for="tool in toolOptions" :key="tool" :value="tool">{{ requestToolLabel(tool) }}</option>
      </select>
    </div>

    <!-- ================= 项目视图（master-detail 两栏） ================= -->
    <div v-if="store.projectStatsLoading && store.projectStats.length === 0" class="flex justify-center py-10">
      <div class="h-5 w-5 animate-spin rounded-full border-2 border-[var(--theme-border-strong)] border-t-[var(--theme-accent-primary)]"></div>
    </div>
    <div v-else-if="store.projectStats.length === 0" class="py-14 text-center text-[12px] text-[var(--theme-text-tertiary)]">
      {{ t(locale, 'desktop.sessions.noProjects') }}
    </div>
    <div v-else class="flex min-h-0 gap-3">
      <!-- 左：项目列表（最小 320px） -->
      <div class="theme-surface w-80 shrink-0 self-start overflow-hidden rounded-xl border">
        <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.sessions.projectsColumn') }}
          <span v-if="projectTruncated" class="ml-1 normal-case text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.sessions.projectsTruncated', { count: 200 }) }}</span>
        </div>
        <div class="max-h-[calc(100vh-260px)] overflow-y-auto">
          <button
            v-for="project in projectList"
            :key="projectKeyOf(project)"
            type="button"
            class="flex w-full items-start gap-2 border-b border-[var(--theme-border-subtle)] px-3 py-2 text-left transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)]"
            :class="selectedProjectKey === projectKeyOf(project) ? 'bg-[var(--theme-accent-soft)]' : ''"
            @click="selectProject(project)"
          >
            <!-- 系统分组：全局 / 未知项目，不伪装成普通项目 -->
            <span v-if="project.projectIdentity === 'global'" class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-slate-500/10 text-slate-500 dark:text-slate-300"><Globe class="h-3.5 w-3.5" aria-hidden="true" /></span>
            <span v-else-if="project.projectIdentity === 'unknown'" class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-amber-500/10 text-amber-600 dark:text-amber-300"><FileQuestionMark class="h-3.5 w-3.5" aria-hidden="true" /></span>
            <span v-else class="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-indigo-500/10 text-indigo-500 dark:text-indigo-300"><Folder class="h-3.5 w-3.5" aria-hidden="true" /></span>
            <span class="min-w-0 flex-1">
              <span class="flex items-center justify-between gap-2">
                <span class="truncate text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ displayProjectName(project) }}</span>
                <span class="shrink-0 text-[10px] text-[var(--theme-text-quaternary)]">{{ formatTime(project.lastActive) }}</span>
              </span>
              <span v-if="project.projectPath" class="mt-0.5 block truncate font-mono text-[10px] text-[var(--theme-text-tertiary)]">{{ project.projectPath }}</span>
              <span v-if="displayProjectHint(project)" class="mt-0.5 block text-[10px] text-[var(--theme-text-quaternary)]">{{ displayProjectHint(project) }}</span>
              <span class="mt-1 block truncate text-[10px] text-[var(--theme-text-secondary)]">
                {{ t(locale, 'desktop.sessions.projectMeta', { sessions: project.sessionCount, requests: project.requestCount, tokens: formatTokens(projectTotalTokens(project)), cost: formatCost(project.totalCost) }) }}
              </span>
            </span>
          </button>
        </div>
      </div>

      <!-- 右：项目摘要 -->
      <div class="min-w-0 flex-1 space-y-3">
        <template v-if="selectedProject">
          <div class="theme-surface rounded-xl border px-4 py-3">
            <div class="flex flex-wrap items-center gap-2">
              <h2 class="text-[14px] font-bold text-[var(--theme-text-primary)]">{{ displayProjectName(selectedProject) }}</h2>
              <span v-if="selectedProject.projectIdentity === 'global'" class="rounded bg-slate-500/10 px-1.5 py-0.5 text-[9.5px] font-semibold text-slate-500 dark:text-slate-300">{{ t(locale, 'common.global') }}</span>
              <span v-else-if="selectedProject.projectIdentity === 'unknown'" class="rounded bg-amber-500/10 px-1.5 py-0.5 text-[9.5px] font-semibold text-amber-600 dark:text-amber-300">{{ t(locale, 'common.unknownProject') }}</span>
              <span class="ml-auto text-[11px] text-[var(--theme-text-tertiary)]">{{ formatTime(selectedProject.lastActive) }}</span>
            </div>
            <p v-if="selectedProject.projectPath" class="mt-1 truncate font-mono text-[10.5px] text-[var(--theme-text-tertiary)]">{{ selectedProject.projectPath }}</p>
            <p v-if="displayProjectHint(selectedProject)" class="mt-1 text-[10.5px] text-[var(--theme-text-quaternary)]">{{ displayProjectHint(selectedProject) }}</p>
          </div>

          <!-- 汇总统计 -->
          <div class="grid grid-cols-2 gap-2 sm:grid-cols-4">
            <div class="theme-surface rounded-xl border px-3 py-2.5">
              <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.projectSessions') }}</div>
              <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ selectedProject.sessionCount ?? 0 }}</div>
            </div>
            <div class="theme-surface rounded-xl border px-3 py-2.5">
              <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnRequests') }}</div>
              <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ selectedProject.requestCount ?? 0 }}</div>
            </div>
            <div class="theme-surface rounded-xl border px-3 py-2.5">
              <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnTokens') }}</div>
              <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ formatTokens(projectTotalTokens(selectedProject)) }}</div>
            </div>
            <div class="theme-surface rounded-xl border px-3 py-2.5">
              <div class="text-[10px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.columnCost') }}</div>
              <div class="mt-0.5 font-mono text-[15px] font-semibold text-[var(--theme-chart-cost)]">{{ formatCost(selectedProject.totalCost) }}</div>
            </div>
          </div>

          <!-- 工具构成 -->
          <div class="theme-surface rounded-xl border">
            <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.toolComposition') }}</div>
            <div v-if="!selectedProject.toolBreakdown || selectedProject.toolBreakdown.length === 0" class="px-3 py-4 text-center text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noToolData') }}</div>
            <table v-else class="w-full text-[11.5px]">
              <thead>
                <tr class="border-b border-[var(--theme-border-subtle)] text-[10px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">
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
                  <td class="px-3 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ tool.requestCount ?? 0 }}</td>
                  <td class="px-3 py-1.5 text-right font-mono text-[var(--theme-text-primary)]">{{ formatTokens((tool.totalInputTokens || 0) + (tool.totalOutputTokens || 0) + (tool.totalCacheCreateTokens || 0) + (tool.totalCacheReadTokens || 0)) }}</td>
                  <td class="px-3 py-1.5 text-right font-mono text-[var(--theme-chart-cost)]">{{ formatCost(tool.totalCost) }}</td>
                  <td class="px-3 py-1.5 text-right text-[var(--theme-text-tertiary)]">{{ formatTime(tool.lastActive) }}</td>
                </tr>
              </tbody>
            </table>
          </div>

          <!-- 模型构成 -->
          <div class="theme-surface rounded-xl border">
            <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.modelComposition') }}</div>
            <div v-if="modelsOfProject.length === 0" class="px-3 py-4 text-center text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noModelData') }}</div>
            <div v-else class="space-y-1 px-3 py-2">
              <div v-for="[model, count] in modelsOfProject" :key="model" class="flex items-center gap-2 text-[11.5px]">
                <span class="min-w-0 flex-1 truncate font-mono text-[var(--theme-text-secondary)]">{{ model }}</span>
                <span class="h-1.5 flex-1 rounded-full bg-[var(--theme-border-subtle)]">
                  <span class="block h-full rounded-full bg-[var(--theme-accent-primary)]" :style="{ width: `${(count / (modelsOfProject[0]?.[1] ?? 1)) * 100}%` }"></span>
                </span>
                <span class="w-10 text-right font-mono text-[var(--theme-text-primary)]">{{ count }}</span>
              </div>
            </div>
          </div>

          <!-- 会话列表（复用会话行，预设项目过滤） -->
          <div class="theme-surface rounded-xl border">
            <div class="border-b border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.projectSessionsList') }}</div>
            <div v-if="sessionsOfProject.length === 0" class="px-3 py-4 text-center text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.sessions.noSessions') }}</div>
            <button
              v-for="session in sessionsOfProject"
              :key="session.sessionId"
              type="button"
              class="flex w-full items-center gap-2 border-b border-[var(--theme-border-subtle)] px-3 py-1.5 text-left transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)]"
              :title="t(locale, 'desktop.sessions.openWorkspaceHint')"
              @dblclick="openWorkspace(session)"
            >
              <span class="min-w-0 flex-1 truncate text-[12px] font-medium text-[var(--theme-text-primary)]">{{ displaySessionTitle(session) }}</span>
              <span class="shrink-0 text-[10px] text-[var(--theme-text-tertiary)]">{{ formatTime(session.lastRequestTime) }}</span>
              <span class="w-16 shrink-0 text-right font-mono text-[11px] text-[var(--theme-text-secondary)]">{{ session.totalRequests ?? 0 }}</span>
              <span class="w-20 shrink-0 text-right font-mono text-[11px] text-[var(--theme-text-secondary)]">{{ sessionUsageVisible(session) ? formatTokens(sessionTotalTokens(session)) : '—' }}</span>
            </button>
          </div>
        </template>
        <div v-else class="theme-surface flex h-48 items-center justify-center rounded-xl border text-[12px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.sessions.selectProjectHint') }}
        </div>
      </div>
    </div>
  </div>
</template>
