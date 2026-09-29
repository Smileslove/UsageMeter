/**
 * 活动页数据逻辑 composable（从 DesktopActivity.vue 拆出，行为逐点一致）。
 *
 * 职责：会话活动加载的完整状态机与分页——
 * - 选中会话 → get_session_activity_summary（null 且索引未建时先 rebuild 再查）；
 * - 事件分页 100/页（后端 SQL 过滤 + 滚动加载 loadEvents(false)）；
 * - 过滤构建 buildFilter（事件种类 + 代理）与 selectedKinds/activeAgentKey 变更自动重置分页；
 * - 派生展示状态：capabilityBadge/badgeMeta（顶部能力徽标）、deepIndexEnabled/fulltextEnabled、
 *   relationLevel（代理树层级）。
 *
 * 会话内搜索状态不在此（随 ActivityTimeline 挂载生命周期自然重置：任何 loadSession
 * 都会使 viewState 离开 'ready'，时间线子组件随之卸载，重挂载时搜索状态即为初始值，
 * 与原 loadSession 内 clearSearch() 等效）。
 */
import { computed, ref, watch } from 'vue'
import { useMonitorStore } from '../../stores/monitor'
import {
  getSessionActivitySummary, getSessionAgents, getSessionEvents,
  getSessionToolSummary, rebuildSessionActivityIndex
} from '../../api/activityApi'
import { ALL_KINDS } from '../views/activity/eventMeta'
import type {
  AgentNodeDto, SessionActivitySummary, SessionEventFilter, SessionEventKind,
  SessionEventListItem, ToolSummaryRow
} from '../../types'

export type ViewState =
  | 'no-session' | 'privacy-off' | 'loading' | 'aggregate-only' | 'no-data' | 'error' | 'ready'

export const PAGE_SIZE = 100

export function useActivityData() {
  const store = useMonitorStore()

  // ============ 活动数据状态 ============
  const activeSessionKey = ref<string | null>(null)
  const summary = ref<SessionActivitySummary | null>(null)
  const agents = ref<AgentNodeDto[]>([])
  const toolSummary = ref<ToolSummaryRow[]>([])
  const events = ref<SessionEventListItem[]>([])
  const total = ref(0)
  const hasMore = ref(false)
  const loading = ref(false)
  const loadingMore = ref(false)
  const building = ref(false)
  const error = ref('')
  const viewState = ref<ViewState>('no-session')

  const selectedKinds = ref<Set<SessionEventKind>>(new Set(ALL_KINDS))
  const activeAgentKey = ref<string | null>(null)

  let eventsOffset = 0
  let loadGeneration = 0

  const ensureRebuildSucceeded = async (scope: string) => {
    const result = await rebuildSessionActivityIndex(scope)
    if (result.sessionsFailed > 0 || result.errors.length > 0) {
      throw new Error(result.errors[0] || 'ERR_ACTIVITY_REBUILD_FAILED')
    }
    return result
  }

  const deepIndexEnabled = computed(() => store.settings.deepIndexLevel !== 'off')
  const capabilityLevel = computed(() => summary.value?.capability.level ?? null)
  const relationLevel = computed(() => summary.value?.capability.agentRelations ?? 'none')

  const capabilityBadge = computed<'off' | 'none' | 'metadata' | 'structured' | 'fullContent' | null>(() => {
    if (!deepIndexEnabled.value) return 'off'
    return capabilityLevel.value
  })

  /** 顶部能力状态徽标（Structured / Off / 仅聚合…）。返回 i18n key，由组件层渲染。 */
  const badgeMeta = computed<{ labelKey: string; cls: string } | null>(() => {
    switch (capabilityBadge.value) {
      case 'off':
        return { labelKey: 'desktop.activity.capabilityOff', cls: 'bg-slate-500/10 text-slate-500 dark:text-slate-300' }
      case 'none':
      case 'metadata':
        return { labelKey: 'desktop.activity.capabilityAggregate', cls: 'bg-amber-500/10 text-amber-600 dark:text-amber-300' }
      case 'structured':
        return { labelKey: 'desktop.activity.capabilityStructured', cls: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-300' }
      case 'fullContent':
        return { labelKey: 'desktop.activity.capabilityOnDemand', cls: 'bg-sky-500/10 text-sky-600 dark:text-sky-300' }
      default:
        return null
    }
  })

  /** 全文索引档启用（M3：会话内与跨会话搜索仅 fulltext 档可用，其余档位禁用搜索执行）。 */
  const fulltextEnabled = computed(() => store.settings.deepIndexLevel === 'fulltext')

  const buildFilter = (): SessionEventFilter | null => {
    const kinds = [...selectedKinds.value]
    if (kinds.length === ALL_KINDS.length && !activeAgentKey.value) return null
    const filter: SessionEventFilter = {}
    if (kinds.length !== ALL_KINDS.length) filter.kinds = kinds
    if (activeAgentKey.value) filter.agents = [activeAgentKey.value]
    return Object.keys(filter).length ? filter : null
  }

  const resetData = () => {
    loadGeneration += 1
    summary.value = null
    agents.value = []
    toolSummary.value = []
    events.value = []
    total.value = 0
    hasMore.value = false
    error.value = ''
    eventsOffset = 0
  }

  const loadEvents = async (reset: boolean) => {
    const key = activeSessionKey.value
    if (!key || !deepIndexEnabled.value) return
    const generation = loadGeneration
    if (reset) {
      eventsOffset = 0
      events.value = []
      total.value = 0
      loading.value = true
    } else {
      loadingMore.value = true
    }
    try {
      const page = await getSessionEvents(store.settings, key, buildFilter(), eventsOffset, PAGE_SIZE)
      if (generation !== loadGeneration) return
      events.value = reset ? page.items : [...events.value, ...page.items]
      total.value = page.total
      hasMore.value = page.hasMore
      eventsOffset += page.items.length
    } catch (e) {
      if (generation !== loadGeneration) return
      error.value = e instanceof Error ? e.message : String(e)
      if (reset) viewState.value = 'error'
    } finally {
      if (generation === loadGeneration) {
        loading.value = false
        loadingMore.value = false
      }
    }
  }

  const loadSession = async (key: string | null) => {
    loadGeneration += 1
    activeSessionKey.value = key
    resetData()
    if (!key) {
      viewState.value = 'no-session'
      return
    }
    if (!deepIndexEnabled.value) {
      viewState.value = 'privacy-off'
      return
    }
    viewState.value = 'loading'
    const generation = loadGeneration
    try {
      let s = await getSessionActivitySummary(store.settings, key)
      if (generation !== loadGeneration) return
      // 懒索引后仍无深度索引 → 自动重建一次再查。
      if (!s) {
        building.value = true
        try {
          await ensureRebuildSucceeded(`session:${key}`)
          if (generation !== loadGeneration) return
          s = await getSessionActivitySummary(store.settings, key)
        } finally {
          building.value = false
        }
      }
      if (generation !== loadGeneration) return
      summary.value = s
      if (!s) {
        viewState.value = 'no-data'
        return
      }
      if (s.capability.level === 'none') {
        viewState.value = 'aggregate-only'
        return
      }
      viewState.value = 'ready'
      void Promise.all([
        getSessionAgents(key).then(list => {
          if (generation === loadGeneration) agents.value = list
        }).catch(() => {}),
        getSessionToolSummary(key).then(list => {
          if (generation === loadGeneration) toolSummary.value = list
        }).catch(() => {})
      ])
      await loadEvents(true)
    } catch (e) {
      if (generation !== loadGeneration) return
      error.value = e instanceof Error ? e.message : String(e)
      viewState.value = 'error'
    } finally {
      if (generation === loadGeneration) loading.value = false
    }
  }

  const rebuildAndReload = async () => {
    const key = activeSessionKey.value
    if (!key || !deepIndexEnabled.value) return
    building.value = true
    error.value = ''
    try {
      await ensureRebuildSucceeded(`session:${key}`)
      await loadSession(key)
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
      viewState.value = 'error'
    } finally {
      building.value = false
    }
  }

  const retry = () => {
    if (activeSessionKey.value) void loadSession(activeSessionKey.value)
  }

  // 过滤变化 → 重置分页（后端 SQL 过滤 + 分页）
  watch(selectedKinds, () => {
    if (viewState.value === 'ready') void loadEvents(true)
  })
  watch(activeAgentKey, () => {
    if (viewState.value === 'ready') void loadEvents(true)
  })

  return {
    activeSessionKey, summary, agents, toolSummary, events, total, hasMore,
    loading, loadingMore, building, error, viewState,
    selectedKinds, activeAgentKey,
    deepIndexEnabled, capabilityLevel, relationLevel, capabilityBadge, badgeMeta, fulltextEnabled,
    buildFilter, loadEvents, loadSession, rebuildAndReload, retry
  }
}
