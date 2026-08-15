import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { ref, watch, type Ref } from 'vue'
import type { ProjectStats, RequestRecord, SessionStats } from '../types'
import type { useMonitorStore } from '../stores/monitor'
import { SESSION_SOURCE_TOOLS } from '../toolCatalog'

export { SESSION_SOURCE_TOOLS }

export const normalizeSessionTool = (tool: string | null | undefined) => (
  tool && SESSION_SOURCE_TOOLS.has(tool) ? tool : null
)

interface SessionCacheEntry {
  items: SessionStats[]
  currentPage: number
  hasMore: boolean
}

interface RequestCacheEntry {
  items: RequestRecord[]
  currentPage: number
  hasMore: boolean
}

type SessionTab = 'recent' | 'requests' | 'projects'

export function useSessionViewData(
  store: ReturnType<typeof useMonitorStore>,
  activeTab: Ref<SessionTab>
) {
  const selectedTool = ref<string | null>(normalizeSessionTool(store.settings.clientTools.activeToolFilter))
  const lastGlobalTool = ref<string | null>(normalizeSessionTool(store.settings.clientTools.activeToolFilter))
  const sessionCache = new Map<string, SessionCacheEntry>()
  const requestCache = new Map<string, RequestCacheEntry>()
  const projectCache = new Map<string, ProjectStats[]>()
  const lastProxyRecordCount = ref<number | null>(null)
  const currentPage = ref(0)
  const hasMore = ref(true)
  const loadingMore = ref(false)
  const requestCurrentPage = ref(0)
  const requestHasMore = ref(true)
  const loadingMoreRequests = ref(false)
  const pageSize = 30
  const requestPageSize = 30
  const proxyRefreshDebounceMs = 10000
  let proxyRefreshTimer: ReturnType<typeof setTimeout> | null = null
  let unlistenLocalUsageSynced: UnlistenFn | null = null
  /** 事件监听失败（如自定义 capability 未授权）时降级：不崩页面，仅失去实时刷新。 */
  async function setupLocalUsageListener() {
    try {
      unlistenLocalUsageSynced = await listen('local_usage_synced', () => scheduleProxyRefresh())
    } catch (err) {
      console.warn('[useSessionViewData] listen local_usage_synced 失败，降级为无实时刷新', err)
    }
  }

  const cacheKey = () => `${selectedTool.value ?? '__all__'}`
  const projectCacheKey = () => `projects:${selectedTool.value ?? '__all__'}`

  const applySessionCache = (entry: SessionCacheEntry) => {
    store.sessions = entry.items
    currentPage.value = entry.currentPage
    hasMore.value = entry.hasMore
    store.sessionsLoading = false
  }

  const rememberSessionCache = (key: string) => {
    sessionCache.set(key, {
      items: [...store.sessions],
      currentPage: currentPage.value,
      hasMore: hasMore.value
    })
  }

  const applyRequestCache = (entry: RequestCacheEntry) => {
    store.requestRecords = entry.items
    requestCurrentPage.value = entry.currentPage
    requestHasMore.value = entry.hasMore
    store.requestRecordsLoading = false
  }

  const rememberRequestCache = (key: string) => {
    requestCache.set(key, {
      items: [...store.requestRecords],
      currentPage: requestCurrentPage.value,
      hasMore: requestHasMore.value
    })
  }

  const clearViewCaches = () => {
    sessionCache.clear()
    requestCache.clear()
    projectCache.clear()
  }

  const reloadSessions = async (force = false) => {
    const key = cacheKey()
    if (!force) {
      const cached = sessionCache.get(key)
      if (cached) {
        applySessionCache(cached)
        return
      }
    }

    const fetchLimit = force && store.sessions.length > pageSize
      ? Math.max(pageSize, Math.ceil(store.sessions.length / pageSize) * pageSize)
      : pageSize
    currentPage.value = 0
    hasMore.value = true
    const count = await store.fetchSessionsForTool(selectedTool.value, fetchLimit, 0, false)
    currentPage.value = Math.max(0, Math.ceil(count / pageSize) - 1)
    if (count < fetchLimit) hasMore.value = false
    rememberSessionCache(key)
  }

  const reloadRequestRecords = async (force = false) => {
    const key = cacheKey()
    if (!force) {
      const cached = requestCache.get(key)
      if (cached) {
        applyRequestCache(cached)
        return
      }
    }

    const fetchLimit = force && store.requestRecords.length > requestPageSize
      ? Math.min(200, Math.max(requestPageSize, Math.ceil(store.requestRecords.length / requestPageSize) * requestPageSize))
      : requestPageSize
    requestCurrentPage.value = 0
    requestHasMore.value = true
    const count = await store.fetchRecentRequestRecordsForTool(selectedTool.value, fetchLimit, 0, false)
    requestCurrentPage.value = Math.max(0, Math.ceil(count / requestPageSize) - 1)
    if (count < fetchLimit || count >= 200) requestHasMore.value = false
    rememberRequestCache(key)
  }

  const reloadProjectStats = async (force = false) => {
    const key = projectCacheKey()
    if (!force) {
      const cached = projectCache.get(key)
      if (cached) {
        store.projectStats = [...cached]
        store.projectStatsLoading = false
        return
      }
    }

    await store.fetchProjectStatsForTool(selectedTool.value)
    projectCache.set(key, [...store.projectStats])
  }

  const triggerSessionViewRefresh = async () => {
    clearViewCaches()
    if (activeTab.value === 'requests') await reloadRequestRecords(true)
    else await reloadSessions(true)
    if (activeTab.value === 'projects') await reloadProjectStats(true)
    else store.projectStats = []
  }

  const scheduleProxyRefresh = () => {
    if (proxyRefreshTimer) clearTimeout(proxyRefreshTimer)
    proxyRefreshTimer = setTimeout(() => {
      proxyRefreshTimer = null
      void triggerSessionViewRefresh()
    }, proxyRefreshDebounceMs)
  }

  const loadMore = async () => {
    if (loadingMore.value || !hasMore.value) return
    loadingMore.value = true
    currentPage.value++
    try {
      const count = await store.fetchSessionsForTool(selectedTool.value, pageSize, currentPage.value * pageSize, true)
      if (count < pageSize) hasMore.value = false
      rememberSessionCache(cacheKey())
    } finally {
      loadingMore.value = false
    }
  }

  const loadMoreRequests = async () => {
    if (loadingMoreRequests.value || !requestHasMore.value) return
    const remaining = 200 - store.requestRecords.length
    if (remaining <= 0) {
      requestHasMore.value = false
      return
    }

    loadingMoreRequests.value = true
    requestCurrentPage.value++
    try {
      const nextLimit = Math.min(requestPageSize, remaining)
      const count = await store.fetchRecentRequestRecordsForTool(
        selectedTool.value,
        nextLimit,
        requestCurrentPage.value * requestPageSize,
        true
      )
      if (count < nextLimit || store.requestRecords.length >= 200) requestHasMore.value = false
      rememberRequestCache(cacheKey())
    } finally {
      loadingMoreRequests.value = false
    }
  }

  watch(activeTab, async newTab => {
    if (newTab === 'requests') await reloadRequestRecords()
    if (newTab === 'projects') await reloadProjectStats()
  })

  watch(selectedTool, async () => {
    if (activeTab.value === 'requests') await reloadRequestRecords()
    else await reloadSessions()
    if (activeTab.value === 'projects') await reloadProjectStats()
    else store.projectStats = []
  })

  watch(() => store.settings.clientTools.activeToolFilter, globalTool => {
    const normalizedGlobalTool = normalizeSessionTool(globalTool)
    if (selectedTool.value === lastGlobalTool.value) selectedTool.value = normalizedGlobalTool
    lastGlobalTool.value = normalizedGlobalTool
  })

  watch(() => store.sessionViewsRevision, async (current, previous) => {
    if (current !== previous) await triggerSessionViewRefresh()
  })

  watch(() => store.proxyStatus?.recordCount ?? null, (current, previous) => {
    if (current === null || previous === null || lastProxyRecordCount.value === null) {
      lastProxyRecordCount.value = current
      return
    }
    if (current <= lastProxyRecordCount.value) {
      lastProxyRecordCount.value = current
      return
    }
    lastProxyRecordCount.value = current
    scheduleProxyRefresh()
  })

  const initialize = async () => {
    await reloadSessions()
    await setupLocalUsageListener()
  }

  const dispose = () => {
    unlistenLocalUsageSynced?.()
    unlistenLocalUsageSynced = null
    if (proxyRefreshTimer) {
      clearTimeout(proxyRefreshTimer)
      proxyRefreshTimer = null
    }
  }

  return {
    selectedTool,
    hasMore,
    loadingMore,
    requestHasMore,
    loadingMoreRequests,
    reloadSessions,
    reloadRequestRecords,
    reloadProjectStats,
    loadMore,
    loadMoreRequests,
    initialize,
    dispose,
  }
}
