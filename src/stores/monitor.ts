import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { setNumberFormatMode } from '../utils/format'
import type { AppSettings, MonthActivity, OverviewBreakdown, ProjectStats, ProxyStatus, ProxyUsageSnapshot, RequestRecord, SessionStats, StatisticsMetric, StatisticsQuery, StatisticsSummary, UsageRefreshBundle, UsageSnapshot, WindowRateSummary, YearActivity, SubscriptionQueryResult, SubscriptionQuota, LimitSurvivalSnapshot, SourceQuotaBindingConfig, CopilotAuthStatus, SourceQuotaBindingRuntimeState, SourceQuotaProfileDescriptor } from '../types'
import {
  CONFIGURED_SOURCE_FAILURE_BASE_MS,
  CONFIGURED_SOURCE_FAILURE_MAX_MS,
  CONFIGURED_SOURCE_SUCCESS_REFRESH_MS,
  createDefaultSettings
} from './monitorDefaults'
import {
  queryProjectStats,
  queryRecentRequestRecords,
  querySessionDetail,
  querySessions
} from './sessionQueries'
import {
  queryMonthActivity,
  queryOverviewBreakdown,
  queryOverviewDeferredBundle,
  queryStatisticsSummary,
  queryYearActivity
} from './statisticsQueries'
import {
  probeSourceQuotaBinding,
  queryBoolean,
  queryConfiguredSourceQuotas,
  queryCopilotAccounts,
  queryCopilotAuthStatus,
  querySourceQuotaBindingStates,
  querySourceQuotaProfiles,
  querySubscriptionQuota,
  testSourceQuotaBinding
} from './subscriptionQueries'
import {
  addKeyPrefix,
  deleteSource,
  mergeSource,
  renameSource,
  updateSourceKeyNote
} from './sourceQueries'
import { failedSubscriptionQuery } from './subscriptionErrors'

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}

export const useMonitorStore = defineStore('monitor', {
  state: () => ({
    settings: createDefaultSettings(),
    snapshot: null as UsageSnapshot | null,
    proxyStatus: null as ProxyStatus | null,
    proxyUsage: null as ProxyUsageSnapshot | null,
    rateSummary: null as WindowRateSummary | null,
    loading: false,
    saving: false,
    proxyLoading: false,
    error: '' as string,
    lastUpdatedEpoch: null as number | null,
    sessionViewsRevision: 0,
    refreshTimer: null as ReturnType<typeof setTimeout> | null,
    autoRefreshGeneration: 0,
    // 会话相关状态
    sessions: [] as SessionStats[],
    sessionsLoading: false,
    selectedSession: null as SessionStats | null,
    requestRecords: [] as RequestRecord[],
    requestRecordsLoading: false,
    // 项目统计（基于所有会话聚合，不受分页影响）
    projectStats: [] as ProjectStats[],
    projectStatsLoading: false,
    // 统计面板
    statisticsSummary: null as StatisticsSummary | null,
    monthActivity: null as MonthActivity | null,
    yearActivity: null as YearActivity | null,
    statisticsLoading: false,
    monthActivityLoading: false,
    yearActivityLoading: false,
    statisticsError: '' as string,
    statisticsRequestSeq: 0,
    statisticsRequestKey: '' as string,
    monthActivityRequestSeq: 0,
    yearActivityRequestSeq: 0,
    monthActivityRequestKey: '',
    yearActivityRequestKey: '',
    // 概览归因排行
    overviewBreakdown: null as OverviewBreakdown | null,
    overviewBreakdownLoading: false,
    overviewBreakdownError: '' as string,
    overviewBreakdownRequestSeq: 0,
    rateSummaryRequestSeq: 0,
    overviewDeferredRequestSeq: 0,
    // 订阅查询
    subscriptionQuota: null as SubscriptionQueryResult | null,
    subscriptionLoading: false,
    hasChatGptOAuth: false,
    // Claude 官方配额查询
    claudeQuota: null as SubscriptionQueryResult | null,
    claudeLoading: false,
    hasClaudeOAuth: false,
    sourceQuotaProfiles: [] as SourceQuotaProfileDescriptor[],
    sourceQuotaBindingStates: {} as Record<string, SourceQuotaBindingRuntimeState>,
    // 各工具已配置来源的第三方中转额度/余额（一行一来源；A 静默降级：空数组表示不可得）
    configuredSourceQuotas: [] as SubscriptionQuota[],
    configuredSourceLoading: false,
    configuredSourceFetchPromise: null as Promise<void> | null,
    configuredSourceRequestSeq: 0,
    configuredSourceActiveSeq: 0,
    configuredSourceQueued: false,
    configuredSourceLastSuccessAt: null as number | null,
    configuredSourceLastAttemptAt: null as number | null,
    configuredSourceLastFailureAt: null as number | null,
    configuredSourceConsecutiveFailures: 0,
    configuredSourceNextEligibleAt: 0,
    configuredSourceLastError: null as string | null,
    // 限额生存层（本地信号：会话锚定块 / 燃烧速率 / 历史基线）
    limitSurvival: null as LimitSurvivalSnapshot | null,
    // Gemini 配额查询
    geminiQuota: null as SubscriptionQueryResult | null,
    geminiQuotaLoading: false,
    hasGeminiOAuth: false,
    // Copilot 认证与配额
    copilotAuthStatus: null as CopilotAuthStatus | null,
    copilotQuota: null as SubscriptionQueryResult | null,
    copilotQuotaLoading: false,
    hasCopilotAuth: false
  }),
  getters: {
    hasData: state => !!state.snapshot,
    windows: state => state.snapshot?.windows ?? [],
    isProxyRunning: state => state.proxyStatus?.running ?? false,
  },
  actions: {
    async initialize() {
      await this.loadSettings()
      await this.refreshUsage()

      // 网关监听是基础服务，与 Claude/Codex 等工具的接管开关无关。
      // 后端会先行启动；前端重复调用是幂等兜底，覆盖热重载等时序。
      try {
        await this.startProxyOnly(this.settings.proxy.port)
      } catch (e) {
        console.error('Failed to start local gateway listener:', e)
      }

      // 检查是否有 ChatGPT OAuth 配置，如果有则查询订阅
      await this.checkChatGptOAuth()
      if (this.hasChatGptOAuth) {
        await this.fetchSubscriptionQuota()
      }

      // 检查是否有 Claude OAuth 凭据，如果有则查询官方配额（限额生存 T1）
      await this.checkClaudeOAuth()
      if (this.hasClaudeOAuth) {
        await this.fetchClaudeQuota()
      }

      // 第三方中转额度查询（已配置来源；静默降级）
      await this.fetchSourceQuotaProfiles()
      await this.forceFetchConfiguredSourceQuotas()
      await this.fetchSourceQuotaBindingStates()

      // 检查是否有 Gemini CLI OAuth 凭据，如果有则查询额度
      await this.checkGeminiOAuth()
      if (this.hasGeminiOAuth) {
        await this.fetchGeminiQuota()
      }

      await this.refreshCopilotAuthStatus()
      if (this.hasCopilotAuth) {
        await this.fetchCopilotQuota()
      }
    },
    async loadSettings() {
      try {
        this.error = ''
        this.settings = await invoke<AppSettings>('load_settings')
        // 后端按 String 持久化，手改配置文件可能出现未知值，归一化避免设置页下拉框空白
        if (this.settings.numberFormat !== 'chinese') {
          this.settings.numberFormat = 'international'
        }
      } catch (e) {
        this.error = errorMessage(e)
      } finally {
        // 主窗口与分享窗口（#/share）都经此同步数值单位显示模式
        setNumberFormatMode(this.settings.numberFormat, this.settings.locale)
      }
    },
    async saveSettings() {
      this.saving = true
      setNumberFormatMode(this.settings.numberFormat, this.settings.locale)
      try {
        this.error = ''
        await invoke('save_settings', { settings: this.settings })
        // 不在这里调用 startAutoRefresh()，避免在设置页面时触发刷新
      } catch (e) {
        this.error = errorMessage(e)
        throw e
      } finally {
        this.saving = false
      }
    },
    async refreshUsage() {
      if (this.loading) {
        return
      }

      this.loading = true
      const startTime = Date.now()
      try {
        this.error = ''

        // 来源额度查询与本地快照并行启动，但不阻塞主刷新路径。
        // 它有独立的 configuredSourceLoading 视觉反馈，不应拖慢核心 usage 刷新。
        void this.scheduleConfiguredSourceQuotaRefreshIfDue()

        const bundle = await invoke<UsageRefreshBundle>('refresh_usage_bundle', { settings: this.settings })
        this.snapshot = bundle.snapshot
        this.limitSurvival = bundle.limitSurvival

        this.lastUpdatedEpoch = this.snapshot.generatedAtEpoch
        const summaryWindow = this.settings.summaryWindow
        void this.fetchOverviewDeferredBundle(summaryWindow)
      } catch (e) {
        this.error = errorMessage(e)
      } finally {
        // 确保最小加载时间为 300ms，让用户能看到刷新动画反馈
        const elapsed = Date.now() - startTime
        const minLoadingMs = 300
        if (elapsed < minLoadingMs) {
          await new Promise(resolve => setTimeout(resolve, minLoadingMs - elapsed))
        }
        this.loading = false
      }
    },
    async refreshUsageAndSessionViews() {
      await this.refreshUsage()
      this.sessionViewsRevision += 1
    },
    async fetchStatisticsSummary(query: StatisticsQuery) {
      const requestKey = JSON.stringify(query)
      if (this.statisticsLoading && this.statisticsRequestKey === requestKey) {
        return
      }

      const requestSeq = ++this.statisticsRequestSeq
      this.statisticsRequestKey = requestKey
      this.statisticsLoading = true
      try {
        this.statisticsError = ''
        const summary = await queryStatisticsSummary(this.settings, query)
        if (requestSeq === this.statisticsRequestSeq) {
          this.statisticsSummary = summary
        }
      } catch (e) {
        if (requestSeq === this.statisticsRequestSeq) {
          this.statisticsError = errorMessage(e)
        }
      } finally {
        if (requestSeq === this.statisticsRequestSeq) {
          this.statisticsLoading = false
        }
      }
    },
    async fetchMonthActivity(year: number, month: number, metric: StatisticsMetric) {
      const requestKey = JSON.stringify({ year, month, metric, settings: this.settings })
      if (this.monthActivityLoading && this.monthActivityRequestKey === requestKey) {
        return
      }
      const requestSeq = ++this.monthActivityRequestSeq
      this.monthActivityRequestKey = requestKey
      this.monthActivityLoading = true
      try {
        this.statisticsError = ''
        const activity = await queryMonthActivity(this.settings, year, month, metric)
        if (requestSeq === this.monthActivityRequestSeq) {
          this.monthActivity = activity
        }
      } catch (e) {
        if (requestSeq === this.monthActivityRequestSeq) {
          this.statisticsError = errorMessage(e)
        }
      } finally {
        if (requestSeq === this.monthActivityRequestSeq) {
          this.monthActivityLoading = false
        }
      }
    },
    async fetchYearActivity(year: number, metric: StatisticsMetric) {
      const requestKey = JSON.stringify({ year, metric, settings: this.settings })
      if (this.yearActivityLoading && this.yearActivityRequestKey === requestKey) {
        return
      }
      const requestSeq = ++this.yearActivityRequestSeq
      this.yearActivityRequestKey = requestKey
      this.yearActivityLoading = true
      try {
        this.statisticsError = ''
        const activity = await queryYearActivity(this.settings, year, metric)
        if (requestSeq === this.yearActivityRequestSeq) {
          this.yearActivity = activity
        }
      } catch (e) {
        if (requestSeq === this.yearActivityRequestSeq) {
          this.statisticsError = errorMessage(e)
        }
      } finally {
        if (requestSeq === this.yearActivityRequestSeq) {
          this.yearActivityLoading = false
        }
      }
    },
    async fetchOverviewBreakdown(window: string) {
      const requestSeq = ++this.overviewBreakdownRequestSeq
      this.overviewBreakdownLoading = true
      try {
        this.overviewBreakdownError = ''
        const breakdown = await queryOverviewBreakdown(this.settings, window)
        if (requestSeq === this.overviewBreakdownRequestSeq) {
          this.overviewBreakdown = breakdown
        }
      } catch (e) {
        if (requestSeq === this.overviewBreakdownRequestSeq) {
          this.overviewBreakdownError = errorMessage(e)
          this.overviewBreakdown = null
        }
      } finally {
        if (requestSeq === this.overviewBreakdownRequestSeq) {
          this.overviewBreakdownLoading = false
        }
      }
    },
    async fetchOverviewDeferredBundle(window: string) {
      const requestSeq = ++this.overviewDeferredRequestSeq
      const rateSummaryRequestSeq = ++this.rateSummaryRequestSeq
      const overviewBreakdownRequestSeq = ++this.overviewBreakdownRequestSeq
      this.overviewBreakdownLoading = true
      try {
        this.overviewBreakdownError = ''
        const bundle = await queryOverviewDeferredBundle(this.settings, window)
        if (requestSeq === this.overviewDeferredRequestSeq) {
          if (this.snapshot) {
            const existingWindows = this.snapshot.windows.filter(item => item.window !== bundle.windowUsage.window)
            this.snapshot = {
              ...this.snapshot,
              windows: [...existingWindows, bundle.windowUsage],
              summary: bundle.usageSummary,
              modelDistribution: bundle.modelDistribution
            }
          }
        }
        if (rateSummaryRequestSeq === this.rateSummaryRequestSeq) {
          this.rateSummary = bundle.rateSummary
        }
        if (overviewBreakdownRequestSeq === this.overviewBreakdownRequestSeq) {
          this.overviewBreakdown = bundle.overviewBreakdown
        }
      } catch (e) {
        if (overviewBreakdownRequestSeq === this.overviewBreakdownRequestSeq) {
          this.overviewBreakdownError = errorMessage(e)
          this.overviewBreakdown = null
        }
        if (rateSummaryRequestSeq === this.rateSummaryRequestSeq) {
          this.rateSummary = {
            window,
            overall: {
              requestCount: 0,
              totalOutputTokens: 0,
              totalDurationMs: 0,
              avgTokensPerSecond: 0
            },
            byModel: [],
            ttft: {
              requestCount: 0,
              avgTtftMs: 0,
              minTtftMs: 0,
              maxTtftMs: 0
            },
            ttftByModel: []
          }
        }
      } finally {
        if (overviewBreakdownRequestSeq === this.overviewBreakdownRequestSeq) {
          this.overviewBreakdownLoading = false
        }
      }
    },
    // 代理相关操作
    /**
     * 启动代理服务器（仅启动，不保存设置）
     * 用于初始化时恢复代理状态
     */
    async startProxyOnly(port: number) {
      this.proxyLoading = true
      try {
        this.error = ''
        await invoke('start_proxy', { port })
        await this.getProxyStatus()
      } catch (e) {
        this.error = errorMessage(e)
        throw e
      } finally {
        this.proxyLoading = false
      }
    },
    async getProxyStatus() {
      try {
        this.proxyStatus = await invoke<ProxyStatus>('get_proxy_status')
      } catch (e) {
        console.error('Failed to get proxy status:', e)
        this.proxyStatus = {
          running: false,
          port: 0,
          uptimeSeconds: 0,
          totalRequests: 0,
          successRequests: 0,
          failedRequests: 0,
          activeConnections: 0,
          configTakenOver: false,
          recordCount: 0,
          status2xx: 0,
          status4xx: 0,
          status5xx: 0
        }
      }
    },
    startAutoRefresh() {
      this.stopAutoRefresh()
      const interval = Math.max(5, this.settings.refreshIntervalSeconds) * 1000
      const generation = ++this.autoRefreshGeneration
      const scheduleNext = () => {
        if (generation !== this.autoRefreshGeneration) return
        this.refreshTimer = setTimeout(async () => {
          if (generation !== this.autoRefreshGeneration) return
          await this.refreshUsage()
          if (this.isProxyRunning) {
            await this.getProxyStatus()
          }
          scheduleNext()
        }, interval)
      }
      scheduleNext()
    },
    stopAutoRefresh() {
      this.autoRefreshGeneration += 1
      if (this.refreshTimer) {
        clearTimeout(this.refreshTimer)
        this.refreshTimer = null
      }
    },
    /**
     * 准备退出：停止代理并恢复 Claude 配置
     * 在应用退出前调用，确保用户可以正常使用 Claude
     */
    async prepareExit() {
      // 如果代理正在运行，先停止并恢复配置
      if (this.isProxyRunning) {
        try {
          await invoke('stop_proxy_runtime_only')
        } catch (e) {
          console.error('Failed to stop proxy on exit:', e)
          // 即使失败也继续退出，下次启动时会通过孤立状态恢复
        }
      }
    },
    // 会话相关操作
    /**
     * 获取会话列表
     * 支持分页：每次加载 limit 个，offset 为偏移量
     */
    async fetchSessions(limit: number = 50, offset: number = 0, append: boolean = false) {
      if (offset === 0) {
        this.sessionsLoading = true
      }
      try {
        const newSessions = await querySessions(this.settings, limit, offset)
        if (append) {
          this.sessions = [...this.sessions, ...newSessions]
        } else {
          this.sessions = newSessions
        }
        return newSessions.length
      } catch (e) {
        console.error('Failed to fetch sessions:', e)
        if (!append) {
          this.sessions = []
        }
        return 0
      } finally {
        this.sessionsLoading = false
      }
    },
    async fetchSessionsForTool(toolFilter: string | null, limit: number = 50, offset: number = 0, append: boolean = false) {
      if (offset === 0) {
        this.sessionsLoading = true
      }
      try {
        const newSessions = await querySessions(this.settings, limit, offset, toolFilter)
        if (append) {
          this.sessions = [...this.sessions, ...newSessions]
        } else {
          this.sessions = newSessions
        }
        return newSessions.length
      } catch (e) {
        console.error('Failed to fetch sessions:', e)
        if (!append) {
          this.sessions = []
        }
        return 0
      } finally {
        this.sessionsLoading = false
      }
    },
    /**
     * 获取会话详情
     */
    async fetchSessionDetail(sessionId: string) {
      try {
        this.selectedSession = await querySessionDetail(this.settings, sessionId)
      } catch (e) {
        console.error('Failed to fetch session detail:', e)
        this.selectedSession = null
      }
    },
    /**
     * 清除选中会话
     */
    clearSelectedSession() {
      this.selectedSession = null
    },
    async fetchRecentRequestRecordsForTool(toolFilter: string | null, limit: number = 30, offset: number = 0, append: boolean = false) {
      if (offset === 0) {
        this.requestRecordsLoading = true
      }
      try {
        const records = await queryRecentRequestRecords(this.settings, toolFilter, limit, offset)
        if (append) {
          this.requestRecords = [...this.requestRecords, ...records]
        } else {
          this.requestRecords = records
        }
        return records.length
      } catch (e) {
        console.error('Failed to fetch request records:', e)
        if (!append) {
          this.requestRecords = []
        }
        return 0
      } finally {
        this.requestRecordsLoading = false
      }
    },
    /**
     * 获取项目统计（基于所有会话聚合，不受分页影响）
     */
    async fetchProjectStats() {
      this.projectStatsLoading = true
      try {
        this.projectStats = await queryProjectStats(this.settings)
      } catch (e) {
        console.error('Failed to fetch project stats:', e)
        this.projectStats = []
      } finally {
        this.projectStatsLoading = false
      }
    },
    async fetchProjectStatsForTool(toolFilter: string | null) {
      this.projectStatsLoading = true
      try {
        this.projectStats = await queryProjectStats(this.settings, toolFilter)
      } catch (e) {
        console.error('Failed to fetch project stats:', e)
        this.projectStats = []
      } finally {
        this.projectStatsLoading = false
      }
    },
    async refreshFilteredViews() {
      await this.refreshUsage()
      await Promise.all([
        this.fetchSessions(30, 0, false),
        this.fetchProjectStats()
      ])
    },
    // === 来源管理 ===
    /**
     * 设置当前激活的来源过滤器
     */
    async setActiveSourceFilter(sourceId: string | null) {
      this.settings.sourceAware.activeSourceFilter = sourceId
      await this.saveSettings()
      await this.refreshFilteredViews()
    },
    /**
     * 设置当前激活的工具过滤器
     */
    async setActiveToolFilter(toolId: string | null) {
      this.settings.clientTools.activeToolFilter = toolId
      await this.saveSettings()
      await this.refreshFilteredViews()
    },
    /**
     * 重命名来源
     */
    async renameSource(sourceId: string, name: string) {
      const source = this.settings.sourceAware.sources.find(s => s.id === sourceId)
      if (source) {
        source.displayName = name.trim() || undefined
        source.autoDetected = false
      }
      await renameSource(sourceId, name)
      await this.loadSettings()
    },
    /**
     * 删除来源
     */
    async deleteSource(sourceId: string, alsoDeleteRecords: boolean = false) {
      await deleteSource(sourceId, alsoDeleteRecords)
      await this.loadSettings()
      await this.refreshUsage()
    },
    /**
     * 合并两个来源
     */
    async mergeSource(sourceIdFrom: string, sourceIdInto: string) {
      await mergeSource(sourceIdFrom, sourceIdInto)
      await this.loadSettings()
      await this.refreshUsage()
    },
    /**
     * 添加 Key 前缀到来源
     */
    async addKeyPrefixToSource(sourceId: string, keyPrefix: string) {
      await addKeyPrefix(sourceId, keyPrefix)
      await this.loadSettings()
    },
    /**
     * 更新 Key 前缀备注
     */
    async updateSourceKeyNote(sourceId: string, keyPrefix: string, note: string) {
      const source = this.settings.sourceAware.sources.find(s => s.id === sourceId)
      if (source) {
        if (!source.apiKeyNotes) source.apiKeyNotes = {}
        const value = note.trim()
        if (value) {
          source.apiKeyNotes[keyPrefix] = value
        } else {
          delete source.apiKeyNotes[keyPrefix]
        }
        source.autoDetected = false
      }
      await updateSourceKeyNote(sourceId, keyPrefix, note)
      await this.loadSettings()
    },
    async updateSourceQuotaQuery(sourceId: string, quotaQuery: SourceQuotaBindingConfig | null) {
      const source = this.settings.sourceAware.sources.find(s => s.id === sourceId)
      if (!source) return
      source.quotaQuery = quotaQuery ?? undefined
      source.autoDetected = false
      await this.saveSettings()
      await this.fetchSourceQuotaBindingStates()
      await this.forceFetchConfiguredSourceQuotas()
    },
    async fetchSourceQuotaProfiles() {
      this.sourceQuotaProfiles = await querySourceQuotaProfiles()
      return this.sourceQuotaProfiles
    },
    async fetchSourceQuotaBindingStates(sourceId?: string) {
      const states = await querySourceQuotaBindingStates(sourceId)
      const next = { ...this.sourceQuotaBindingStates }
      for (const state of states) {
        next[state.sourceId] = state
      }
      this.sourceQuotaBindingStates = next
      return states
    },
    async probeSourceQuotaQuery(sourceId: string, binding: SourceQuotaBindingConfig | null = null) {
      const state = await probeSourceQuotaBinding(sourceId, binding)
      this.sourceQuotaBindingStates = {
        ...this.sourceQuotaBindingStates,
        [state.sourceId]: state,
      }
      return state
    },
    async testSourceQuotaQuery(sourceId: string, binding: SourceQuotaBindingConfig) {
      const result = await testSourceQuotaBinding(sourceId, binding)
      await this.fetchSourceQuotaBindingStates(sourceId)
      return result
    },
    // === 订阅查询 ===
    /**
     * 检查是否有 ChatGPT OAuth 配置
     */
    async checkChatGptOAuth() {
      try {
        this.hasChatGptOAuth = await queryBoolean('has_chatgpt_oauth')
      } catch (e) {
        console.error('Failed to check ChatGPT OAuth:', e)
        this.hasChatGptOAuth = false
      }
    },
    /**
     * 获取订阅配额
     */
    async fetchSubscriptionQuota() {
      this.subscriptionLoading = true
      try {
        this.subscriptionQuota = await querySubscriptionQuota('gpt')
      } catch (e) {
        console.error('Failed to fetch subscription quota:', e)
        // 设置错误状态，避免显示旧数据
        this.subscriptionQuota = failedSubscriptionQuery(e)
      } finally {
        this.subscriptionLoading = false
      }
    },
    /**
     * 刷新订阅配额（强制刷新）
     */
    async refreshSubscriptionQuota() {
      this.subscriptionLoading = true
      try {
        this.subscriptionQuota = await querySubscriptionQuota('gpt', true)
      } catch (e) {
        console.error('Failed to refresh subscription quota:', e)
        // 设置错误状态，避免显示旧数据
        this.subscriptionQuota = failedSubscriptionQuery(e)
      } finally {
        this.subscriptionLoading = false
      }
    },
    /**
     * 检查是否有 Claude OAuth 凭据
     */
    async checkClaudeOAuth() {
      try {
        this.hasClaudeOAuth = await queryBoolean('has_claude_oauth')
      } catch (e) {
        console.error('Failed to check Claude OAuth:', e)
        this.hasClaudeOAuth = false
      }
    },
    /**
     * 获取 Claude 官方配额
     */
    async fetchClaudeQuota() {
      this.claudeLoading = true
      try {
        this.claudeQuota = await querySubscriptionQuota('claude')
      } catch (e) {
        console.error('Failed to fetch Claude quota:', e)
        this.claudeQuota = failedSubscriptionQuery(e)
      } finally {
        this.claudeLoading = false
      }
    },
    /**
     * 刷新 Claude 官方配额（强制刷新）
     */
    async refreshClaudeQuota() {
      this.claudeLoading = true
      try {
        this.claudeQuota = await querySubscriptionQuota('claude', true)
      } catch (e) {
        console.error('Failed to refresh Claude quota:', e)
        this.claudeQuota = failedSubscriptionQuery(e)
      } finally {
        this.claudeLoading = false
      }
    },
    // === Gemini 额度查询 ===
    /**
     * 检查是否有 Gemini CLI OAuth 凭据
     */
    async checkGeminiOAuth() {
      try {
        this.hasGeminiOAuth = await queryBoolean('has_gemini_oauth')
      } catch (e) {
        console.error('Failed to check Gemini OAuth:', e)
        this.hasGeminiOAuth = false
      }
    },
    /**
     * 获取 Gemini 额度
     */
    async fetchGeminiQuota() {
      this.geminiQuotaLoading = true
      try {
        this.geminiQuota = await querySubscriptionQuota('gemini')
      } catch (e) {
        console.error('Failed to fetch Gemini quota:', e)
        this.geminiQuota = failedSubscriptionQuery(e)
      } finally {
        this.geminiQuotaLoading = false
      }
    },
    /**
     * 刷新 Gemini 额度（强制刷新）
     */
    async refreshGeminiQuota() {
      this.geminiQuotaLoading = true
      try {
        this.geminiQuota = await querySubscriptionQuota('gemini', true)
      } catch (e) {
        console.error('Failed to refresh Gemini quota:', e)
        this.geminiQuota = failedSubscriptionQuery(e)
      } finally {
        this.geminiQuotaLoading = false
      }
    },
    async refreshCopilotAuthStatus() {
      try {
        this.copilotAuthStatus = await queryCopilotAuthStatus()
        this.hasCopilotAuth = !!this.copilotAuthStatus?.authenticated
      } catch (e) {
        console.error('Failed to fetch Copilot auth status:', e)
        this.copilotAuthStatus = null
        this.hasCopilotAuth = false
      }
    },
    async checkCopilotAuth() {
      try {
        this.hasCopilotAuth = await queryBoolean('copilot_is_authenticated')
      } catch (e) {
        console.error('Failed to check Copilot auth:', e)
        this.hasCopilotAuth = false
      }
    },
    async fetchCopilotQuota() {
      this.copilotQuotaLoading = true
      try {
        this.copilotQuota = await querySubscriptionQuota('copilot')
      } catch (e) {
        console.error('Failed to fetch Copilot quota:', e)
        this.copilotQuota = failedSubscriptionQuery(e)
      } finally {
        this.copilotQuotaLoading = false
      }
    },
    async refreshCopilotQuota() {
      this.copilotQuotaLoading = true
      try {
        this.copilotQuota = await querySubscriptionQuota('copilot', true)
      } catch (e) {
        console.error('Failed to refresh Copilot quota:', e)
        this.copilotQuota = failedSubscriptionQuery(e)
      } finally {
        this.copilotQuotaLoading = false
      }
    },
    async copilotListAccounts() {
      return queryCopilotAccounts()
    },
    /**
     * 查询各工具已配置来源的第三方中转额度/余额（一行一来源，A 静默降级）。
     * 检测到可识别来源且能读到凭据则自动查询；缺凭据/查询失败都静默降级为空数组，不报错。
     */
    async fetchConfiguredSourceQuotas() {
      return this.fetchConfiguredSourceQuotasInternal(false)
    },
    async forceFetchConfiguredSourceQuotas() {
      return this.fetchConfiguredSourceQuotasInternal(true)
    },
    async scheduleConfiguredSourceQuotaRefreshIfDue() {
      const now = Date.now()
      if (this.configuredSourceFetchPromise) {
        this.configuredSourceQueued = true
        return this.configuredSourceFetchPromise
      }
      if (now < this.configuredSourceNextEligibleAt) {
        return
      }
      return this.fetchConfiguredSourceQuotasInternal(false)
    },
    async fetchConfiguredSourceQuotasInternal(force: boolean) {
      const now = Date.now()
      if (!force && !this.configuredSourceFetchPromise && now < this.configuredSourceNextEligibleAt) {
        return
      }

      const wantedSeq = this.configuredSourceRequestSeq + 1
      this.configuredSourceRequestSeq = wantedSeq

      if (this.configuredSourceFetchPromise) {
        this.configuredSourceQueued = true
        return this.configuredSourceFetchPromise
      }

      this.configuredSourceLoading = true
      const runLatest = async (): Promise<void> => {
        const activeSeq = this.configuredSourceRequestSeq
        this.configuredSourceActiveSeq = activeSeq
        this.configuredSourceLastAttemptAt = Date.now()
        try {
          const result = await queryConfiguredSourceQuotas()
          // 仅最新请求可落地结果，避免初始化/旧刷新覆盖更新后的来源状态。
          if (activeSeq === this.configuredSourceRequestSeq) {
            if (result.attemptedCount === 0) {
              this.configuredSourceQuotas = []
              this.configuredSourceLastSuccessAt = result.queriedAt
              this.configuredSourceLastError = null
              this.configuredSourceConsecutiveFailures = 0
              this.configuredSourceLastFailureAt = null
              this.configuredSourceNextEligibleAt = result.queriedAt + CONFIGURED_SOURCE_SUCCESS_REFRESH_MS
            } else if (result.successCount > 0) {
              this.configuredSourceQuotas = result.quotas
              this.configuredSourceLastSuccessAt = result.queriedAt
              this.configuredSourceConsecutiveFailures = 0
              this.configuredSourceLastFailureAt = null
              this.configuredSourceLastError = result.failedCount > 0
                ? result.errors.join(' | ')
                : null
              this.configuredSourceNextEligibleAt = result.queriedAt + CONFIGURED_SOURCE_SUCCESS_REFRESH_MS
            } else {
              this.configuredSourceConsecutiveFailures += 1
              this.configuredSourceLastFailureAt = result.queriedAt
              // Store only carries stable backend codes; UI resolves them through i18n.
              this.configuredSourceLastError = result.errors.join(' | ') || 'ERR_CONFIGURED_SOURCE_QUOTA_FAILED'
              const backoffMs = Math.min(
                CONFIGURED_SOURCE_FAILURE_BASE_MS * (2 ** (this.configuredSourceConsecutiveFailures - 1)),
                CONFIGURED_SOURCE_FAILURE_MAX_MS
              )
              this.configuredSourceNextEligibleAt = result.queriedAt + backoffMs
            }
          }
        } catch (e) {
          console.error('Failed to fetch configured source quotas:', e)
          if (activeSeq === this.configuredSourceRequestSeq) {
            this.configuredSourceConsecutiveFailures += 1
            this.configuredSourceLastFailureAt = Date.now()
            this.configuredSourceLastError = String(e)
            const backoffMs = Math.min(
              CONFIGURED_SOURCE_FAILURE_BASE_MS * (2 ** (this.configuredSourceConsecutiveFailures - 1)),
              CONFIGURED_SOURCE_FAILURE_MAX_MS
            )
            this.configuredSourceNextEligibleAt = Date.now() + backoffMs
          }
        }
      }

      this.configuredSourceFetchPromise = (async () => {
        try {
          do {
            this.configuredSourceQueued = false
            await runLatest()
          } while (this.configuredSourceQueued || this.configuredSourceActiveSeq !== this.configuredSourceRequestSeq)
        } finally {
          this.configuredSourceLoading = false
          this.configuredSourceFetchPromise = null
        }
      })()

      return this.configuredSourceFetchPromise
    }
  }
})
