<script setup lang="ts">
/**
 * 桌面主窗口「活动」页（设计文档第 9 章：三栏布局 9.2 / 左栏 9.3 / 时间线 9.4 /
 * 工具卡 9.5 / 子代理 9.6 / 检查器 9.7 / 搜索 9.8 / 请求关联 9.9）。
 *
 * 本文件为页面骨架：三栏布局 + 视图状态机 + 会话生命周期；职责块拆分如下——
 * - 数据加载/过滤/分页：useActivityData composable；
 * - 会话选择器 / 跨会话搜索 / 导出对话框 / 检查器 / 时间线 / 左栏：activity/* 子组件；
 * - 展示元数据常量（KIND_META/statusMeta/linkMeta/格式化）：activity/eventMeta.ts。
 *
 * - 顶部：会话选择器（自由模式）+ 跨会话搜索 + 能力状态徽标；
 * - 数据流：选中会话 → get_session_activity_summary（null 且索引未建时先
 *   rebuild_session_activity_index("session:<key>") 再查）→ 分页 100/页滚动加载事件；
 *   隐私 off（deep_index_level='off'）时后端返回空，显示“深度活动未启用”+ 去设置；
 * - 三栏布局：≥1180px 左 240px + 中 min 480px + 右 360px（可折叠）；
 *   960-1179px 左栏折叠为顶部按钮、右栏 overlay drawer；最小宽度无横向滚动；
 * - 事件正文/工具输入输出一律来自后端脱敏 payload，前端不二次处理敏感逻辑、不使用 v-html。
 *
 * fixedSessionKey prop：会话工作区（SessionWorkspace 活动 tab）内嵌模式——隐藏顶部
 * 会话选择器并固定到该会话，其余逻辑与自由模式完全一致。
 */
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import {
  Bot, Eye, EyeOff, FileOutput, Layers, Loader2, PanelLeft, RefreshCw, TriangleAlert
} from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useDesktopNavigationStore } from '../../desktop/stores/desktopNavigation'
import { t, backendErrorLabel } from '../../i18n'
import type { GlobalSearchHit, SessionEventListItem, SessionStats } from '../../types'
import { useActivityData } from '../composables/useActivityData'
import SessionPicker from './activity/SessionPicker.vue'
import GlobalSearchPanel from './activity/GlobalSearchPanel.vue'
import ExportDialog from './activity/ExportDialog.vue'
import EventInspector from './activity/EventInspector.vue'
import ActivityTimeline from './activity/ActivityTimeline.vue'
import ActivitySidebar from './activity/ActivitySidebar.vue'

const props = defineProps<{
  /** 会话工作区内嵌模式：固定到指定会话并隐藏顶部会话选择器。 */
  fixedSessionKey?: string
}>()

const store = useMonitorStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

// ============ 活动数据（过滤构建 + 分页加载 + 能力徽标） ============
const {
  activeSessionKey, viewState, agents, toolSummary, events, total, hasMore,
  loading, loadingMore, building, error, selectedKinds, activeAgentKey,
  relationLevel, badgeMeta, fulltextEnabled,
  loadSession, loadEvents, rebuildAndReload, retry
} = useActivityData()

// ============ 布局断点（设计 9.2）：≥1180px 三栏，否则左栏按钮 + 右栏 drawer ============
const wideQuery = window.matchMedia('(min-width: 1180px)')
const wideMode = ref(wideQuery.matches)
const onWideChange = (event: MediaQueryListEvent) => { wideMode.value = event.matches }
const sidebarOpen = ref(false)          // 窄屏左栏 overlay
const inspectorOpen = ref(false)        // 窄屏检查器 drawer
const inspectorCollapsed = ref(false)   // 宽屏检查器折叠
wideQuery.addEventListener('change', onWideChange)

// ============ 选中事件（检查器由 EventInspector 子组件渲染，payload 状态自管理） ============
const selectedEvent = ref<SessionEventListItem | null>(null)
const selectEvent = (event: SessionEventListItem) => {
  selectedEvent.value = event
  if (!wideMode.value) inspectorOpen.value = true
}
const closeInspector = () => {
  if (!wideMode.value) inspectorOpen.value = false
  selectedEvent.value = null
}

// ============ 导出对话框开关（对话框本体与导出状态在 ExportDialog 子组件内） ============
const exportDialogOpen = ref(false)
const openExportDialog = () => { exportDialogOpen.value = true }

// ============ 会话选择 / 跨会话搜索跳转 ============
const selectSession = (session: SessionStats) => { void loadSession(session.sessionId) }
const jumpToSession = (hit: GlobalSearchHit) => { nav.openActivity(hit.sessionKey) }

const showPicker = computed(() => !props.fixedSessionKey)
/** 自由模式无深链会话时才让选择器预加载会话列表（与原 ensureSessions 分支等价）。 */
const autoLoadSessions = computed(() => !nav.activeSessionKey)

// ============ 生命周期：固定会话 / 导航深链 ============
onMounted(() => {
  if (props.fixedSessionKey) void loadSession(props.fixedSessionKey)
  else if (nav.activeSessionKey) void loadSession(nav.activeSessionKey)
})
watch(() => props.fixedSessionKey, key => {
  if (key) void loadSession(key)
})
watch(() => nav.activeSessionKey, key => {
  if (!props.fixedSessionKey && key && key !== activeSessionKey.value) void loadSession(key)
})
onUnmounted(() => {
  wideQuery.removeEventListener('change', onWideChange)
})
</script>

<template>
  <section class="flex min-w-0 flex-col gap-3 pb-4">
    <!-- ============ 顶部：会话选择器 + 跨会话搜索 + 能力徽标 ============ -->
    <div class="flex flex-wrap items-center gap-2">
      <!-- 会话选择器（自由模式） -->
      <SessionPicker
        v-if="showPicker"
        :active-key="activeSessionKey"
        :auto-load="autoLoadSessions"
        @select="selectSession"
      />
      <div v-else class="min-w-0 flex-1" />

      <!-- 跨会话搜索（9.8：fulltext 档可用；结果跳转该会话活动页） -->
      <GlobalSearchPanel v-if="showPicker" @jump="jumpToSession" />

      <!-- 能力状态徽标 -->
      <span
        v-if="badgeMeta"
        class="inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-xs font-bold leading-none"
        :class="badgeMeta.cls"
      >
        <span class="h-1.5 w-1.5 rounded-full bg-current" aria-hidden="true"></span>
        {{ t(locale, badgeMeta.labelKey) }}
      </span>

      <!-- 窄屏：左栏折叠按钮 + 检查器按钮 -->
      <div v-if="!wideMode && viewState === 'ready'" class="flex shrink-0 items-center gap-1.5">
        <button
          type="button"
          class="theme-button-secondary inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-xs font-semibold"
          :aria-label="t(locale, 'desktop.activity.sidebarToggle')"
          :title="t(locale, 'desktop.activity.sidebarToggle')"
          @click="sidebarOpen = !sidebarOpen"
        >
          <PanelLeft class="h-3.5 w-3.5" aria-hidden="true" />
          <span class="hidden sm:inline">{{ t(locale, 'desktop.activity.filtersLabel') }}</span>
        </button>
        <button
          type="button"
          class="theme-button-secondary inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-xs font-semibold"
          :aria-label="t(locale, 'desktop.activity.inspectorToggle')"
          :title="t(locale, 'desktop.activity.inspectorToggle')"
          @click="inspectorOpen = !inspectorOpen"
        >
          <Eye class="h-3.5 w-3.5" aria-hidden="true" />
          <span class="hidden sm:inline">{{ t(locale, 'desktop.activity.inspectorTitle') }}</span>
        </button>
      </div>
    </div>

    <!-- ============ 状态机 ============ -->
    <!-- 未选择会话 -->
    <div
      v-if="viewState === 'no-session'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <Bot class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <h3 class="mt-3 text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.noSessionTitle') }}</h3>
      <p class="mt-1.5 max-w-md text-xs leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.noSessionDesc') }}</p>
    </div>

    <!-- 隐私关闭（deep_index_level=off） -->
    <div
      v-else-if="viewState === 'privacy-off'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <EyeOff class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <h3 class="mt-3 text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.privacyDisabledTitle') }}</h3>
      <p class="mt-1.5 max-w-md text-xs leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.privacyDisabledDesc') }}</p>
      <button
        type="button"
        class="theme-button-secondary mt-5 inline-flex items-center gap-1.5 rounded-lg px-4 py-2 text-xs font-semibold"
        @click="nav.openSettingsSection('privacy')"
      >
        {{ t(locale, 'desktop.activity.openSettings') }}
      </button>
    </div>

    <!-- 初始/切换加载骨架 -->
    <div v-else-if="viewState === 'loading'" class="space-y-2">
      <div v-for="i in 5" :key="i" class="theme-surface animate-pulse rounded-xl border px-4 py-3">
        <div class="h-3 w-1/3 rounded bg-[var(--theme-border-subtle)]"></div>
        <div class="mt-2 h-2.5 w-2/3 rounded bg-[var(--theme-border-subtle)]"></div>
      </div>
    </div>

    <!-- 仅聚合数据（capability.level=none） -->
    <div
      v-else-if="viewState === 'aggregate-only'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <Layers class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <h3 class="mt-3 text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.aggregateOnlyTitle') }}</h3>
      <p class="mt-1.5 max-w-md text-xs leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.aggregateOnlyDesc') }}</p>
      <button
        type="button"
        class="theme-button-secondary mt-5 inline-flex items-center gap-1.5 rounded-lg px-4 py-2 text-xs font-semibold"
        @click="nav.openSettingsSection('dataSources')"
      >
        {{ t(locale, 'desktop.activity.manageDataSources') }}
      </button>
    </div>

    <!-- 无深度数据（rebuild 后仍无） -->
    <div
      v-else-if="viewState === 'no-data'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <FileOutput class="h-7 w-7 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <h3 class="mt-3 text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.emptyTitle') }}</h3>
      <p class="mt-1.5 max-w-md text-xs leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.emptyDesc') }}</p>
      <button
        type="button"
        class="theme-button-secondary mt-5 inline-flex items-center gap-1.5 rounded-lg px-4 py-2 text-xs font-semibold"
        :disabled="building"
        @click="rebuildAndReload"
      >
        <Loader2 v-if="building" class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
        <RefreshCw v-else class="h-3.5 w-3.5" aria-hidden="true" />
        {{ building ? t(locale, 'desktop.activity.buildingIndex') : t(locale, 'desktop.activity.rebuildIndex') }}
      </button>
    </div>

    <!-- 加载失败 -->
    <div
      v-else-if="viewState === 'error'"
      class="theme-surface flex flex-col items-center justify-center rounded-xl border px-6 py-16 text-center"
    >
      <TriangleAlert class="h-7 w-7 text-rose-500" aria-hidden="true" />
      <h3 class="mt-3 text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.activity.errorTitle') }}</h3>
      <p class="mt-1.5 max-w-md break-all text-xs leading-relaxed text-rose-500">{{ backendErrorLabel(locale, error) }}</p>
      <button
        type="button"
        class="theme-button-secondary mt-5 inline-flex items-center gap-1.5 rounded-lg px-4 py-2 text-xs font-semibold"
        @click="retry"
      >
        <RefreshCw class="h-3.5 w-3.5" aria-hidden="true" />
        {{ t(locale, 'desktop.activity.retry') }}
      </button>
    </div>

    <!-- 非 fulltext 档：会话内搜索不可用提示（9.8：不静默降级为本地过滤） -->
    <div
      v-if="viewState === 'ready' && !fulltextEnabled"
      class="flex flex-wrap items-center gap-2 rounded-xl border border-amber-500/20 bg-amber-500/5 px-3 py-2"
    >
      <TriangleAlert class="h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-300" aria-hidden="true" />
      <span class="min-w-0 flex-1 text-xs text-amber-600 dark:text-amber-300">{{ t(locale, 'desktop.activity.searchFtsDisabled') }}</span>
      <button
        type="button"
        class="shrink-0 rounded-md border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-xs font-semibold text-amber-600 transition-colors hover:bg-amber-500/15 dark:text-amber-300"
        @click="nav.openSettingsSection('privacy')"
      >
        {{ t(locale, 'desktop.activity.openSettings') }}
      </button>
    </div>

    <!-- ============ 三栏布局（设计 9.2） ============ -->
    <div v-else-if="viewState === 'ready'" class="relative flex min-h-0 min-w-0 flex-1 items-stretch gap-4">
      <!-- 左栏：章节与代理树（240px；窄屏 overlay） -->
      <ActivitySidebar
        v-if="wideMode || sidebarOpen"
        :agents="agents"
        :relation-level="relationLevel"
        :tool-summary="toolSummary"
        :selected-kinds="selectedKinds"
        :active-agent-key="activeAgentKey"
        :wide="wideMode"
        @update:selected-kinds="selectedKinds = $event"
        @update:active-agent-key="activeAgentKey = $event"
        @close="sidebarOpen = false"
      />

      <!-- 中栏：活动脉络时间线（min 480px） -->
      <ActivityTimeline
        :session-key="activeSessionKey"
        :events="events"
        :total="total"
        :has-more="hasMore"
        :loading="loading"
        :loading-more="loadingMore"
        :selected-kinds="selectedKinds"
        :active-agent-key="activeAgentKey"
        :selected-event-key="selectedEvent?.eventKey ?? null"
        :fulltext-enabled="fulltextEnabled"
        @select="selectEvent"
        @load-more="() => void loadEvents(false)"
        @export="openExportDialog"
      />

      <!-- 右栏：事件检查器（360px；宽屏可折叠，窄屏 overlay drawer） -->
      <EventInspector
        v-if="(wideMode && !inspectorCollapsed) || (!wideMode && inspectorOpen)"
        :event="selectedEvent"
        :wide="wideMode"
        @close="closeInspector"
        @collapse="inspectorCollapsed = true"
      />

      <!-- 宽屏检查器折叠按钮 -->
      <button
        v-if="wideMode && inspectorCollapsed"
        type="button"
        class="theme-surface absolute right-0 top-1/2 z-10 -translate-y-1/2 rounded-xl border p-2 text-[var(--theme-text-tertiary)] shadow-lg transition-colors hover:text-[var(--theme-text-primary)]"
        :aria-label="t(locale, 'desktop.activity.inspectorToggle')"
        :title="t(locale, 'desktop.activity.inspectorToggle')"
        @click="inspectorCollapsed = false"
      >
        <Eye class="h-4 w-4" aria-hidden="true" />
      </button>
    </div>

    <!-- 重建索引中的遮罩提示 -->
    <div v-if="building" class="pointer-events-none fixed inset-0 z-50 flex items-center justify-center">
      <div class="theme-surface-elevated flex items-center gap-2 rounded-xl border px-4 py-3 text-xs font-medium text-[var(--theme-text-secondary)] shadow-xl">
        <Loader2 class="h-4 w-4 animate-spin" aria-hidden="true" />
        {{ t(locale, 'desktop.activity.buildingIndex') }}
      </div>
    </div>

    <!-- 导出对话框（15.3 / 21.5：范围预览，默认不勾选正文与工具 payload；成功显示路径可复制） -->
    <ExportDialog
      :open="exportDialogOpen"
      :session-key="activeSessionKey"
      @close="exportDialogOpen = false"
    />
  </section>
</template>
