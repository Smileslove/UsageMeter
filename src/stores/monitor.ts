import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { setNumberFormatMode } from '../utils/format'
import { pickEffectiveWindow } from '../utils/windowFallback'
import type { AppSettings, MonthActivity, OverviewBreakdown, ProjectStats, ProxyStatus, ProxyUsageSnapshot, RequestRecord, SessionStats, StatisticsMetric, StatisticsQuery, StatisticsSummary, UsageRefreshBundle, UsageSnapshot, WindowRateSummary, YearActivity, SubscriptionQueryResult, SubscriptionQuota, LimitSurvivalSnapshot, SourceQuotaBindingConfig, CopilotAuthStatus, SourceQuotaBindingRuntimeState, SourceQuotaProfileDescriptor } from '../types'
import { createDefaultSettings } from './monitorDefaults'
import {
  applyConfiguredSourceQuotaResult,
  applyConfiguredSourceQuotaFailure,
  createEmptyRateSummary,
  mergeDeferredOverviewSnapshot,
  normalizeSettings
} from './monitorActionHelpers'
import {
  checkOAuthAction,
  fetchMonthActivityAction,
  fetchOverviewBreakdownAction,
  fetchProjectStatsAction,
  fetchRecentRequestRecordsAction,
  fetchSessionDetailAction,
  fetchSessionsAction,
  fetchStatisticsSummaryAction,
  fetchYearActivityAction,
  getProxyStatusAction,
  prepareExitAction,
  refreshCopilotAuthStatusAction,
  runProviderQuotaAction,
  startProxyOnlyAction
} from './monitorDomains'
import { queryOverviewDeferredBundle } from './statisticsQueries'
import {
  probeSourceQuotaBinding,
  queryConfiguredSourceQuotas,
  queryCopilotAccounts,
  querySourceQuotaBindingStates,
  querySourceQuotaProfiles,
  testSourceQuotaBinding
} from './subscriptionQueries'
import {
  addKeyPrefix,
  deleteSource,
  mergeSource,
  renameSource,
  updateSourceKeyNote
} from './sourceQueries'

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
        this.settings = normalizeSettings(this.settings)
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
        // deferred 包（排行/速率）按「生效窗口」拉取：当前所选窗口无数据时
        // 回退到有数据的长窗口，避免概览排行与速率区空白（SummaryPanel 的
        // 回退展示与之一致；用户显式切换窗口走 selectWindow 拉所选窗口）。
        const effective = pickEffectiveWindow(this.snapshot.windows, this.settings.summaryWindow)
        void this.fetchOverviewDeferredBundle(effective.window ?? this.settings.summaryWindow)
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
      return fetchStatisticsSummaryAction(this, query)
    },
    async fetchMonthActivity(year: number, month: number, metric: StatisticsMetric) {
      return fetchMonthActivityAction(this, year, month, metric)
    },
    async fetchYearActivity(year: number, metric: StatisticsMetric) {
      return fetchYearActivityAction(this, year, metric)
    },
    async fetchOverviewBreakdown(window: string) {
      return fetchOverviewBreakdownAction(this, window)
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
          this.snapshot = mergeDeferredOverviewSnapshot(this.snapshot, bundle)
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
          this.rateSummary = createEmptyRateSummary(window)
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
      return startProxyOnlyAction(this, port)
    },
    async getProxyStatus() {
      return getProxyStatusAction(this)
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
      return prepareExitAction(this)
    },
    // 会话相关操作
    /**
     * 获取会话列表
     * 支持分页：每次加载 limit 个，offset 为偏移量
     */
    async fetchSessions(limit: number = 50, offset: number = 0, append: boolean = false) {
      return fetchSessionsAction(this, limit, offset, append)
    },
    async fetchSessionsForTool(toolFilter: string | null, limit: number = 50, offset: number = 0, append: boolean = false) {
      return fetchSessionsAction(this, limit, offset, append, toolFilter)
    },
    /**
     * 获取会话详情
     */
    async fetchSessionDetail(sessionId: string) {
      return fetchSessionDetailAction(this, sessionId)
    },
    /**
     * 清除选中会话
     */
    clearSelectedSession() {
      this.selectedSession = null
    },
    async fetchRecentRequestRecordsForTool(toolFilter: string | null, limit: number = 30, offset: number = 0, append: boolean = false) {
      return fetchRecentRequestRecordsAction(this, toolFilter, limit, offset, append)
    },
    /**
     * 获取项目统计（基于所有会话聚合，不受分页影响）
     */
    async fetchProjectStats() {
      return fetchProjectStatsAction(this)
    },
    async fetchProjectStatsForTool(toolFilter: string | null) {
      return fetchProjectStatsAction(this, toolFilter)
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
      return checkOAuthAction(this, 'has_chatgpt_oauth', 'hasChatGptOAuth')
    },
    /**
     * 获取订阅配额
     */
    async fetchSubscriptionQuota() {
      return runProviderQuotaAction(this, 'gpt', false)
    },
    /**
     * 刷新订阅配额（强制刷新）
     */
    async refreshSubscriptionQuota() {
      return runProviderQuotaAction(this, 'gpt', true)
    },
    /**
     * 检查是否有 Claude OAuth 凭据
     */
    async checkClaudeOAuth() {
      return checkOAuthAction(this, 'has_claude_oauth', 'hasClaudeOAuth')
    },
    /**
     * 获取 Claude 官方配额
     */
    async fetchClaudeQuota() {
      return runProviderQuotaAction(this, 'claude', false)
    },
    /**
     * 刷新 Claude 官方配额（强制刷新）
     */
    async refreshClaudeQuota() {
      return runProviderQuotaAction(this, 'claude', true)
    },
    // === Gemini 额度查询 ===
    /**
     * 检查是否有 Gemini CLI OAuth 凭据
     */
    async checkGeminiOAuth() {
      return checkOAuthAction(this, 'has_gemini_oauth', 'hasGeminiOAuth')
    },
    /**
     * 获取 Gemini 额度
     */
    async fetchGeminiQuota() {
      return runProviderQuotaAction(this, 'gemini', false)
    },
    /**
     * 刷新 Gemini 额度（强制刷新）
     */
    async refreshGeminiQuota() {
      return runProviderQuotaAction(this, 'gemini', true)
    },
    async refreshCopilotAuthStatus() {
      return refreshCopilotAuthStatusAction(this)
    },
    async checkCopilotAuth() {
      return checkOAuthAction(this, 'copilot_is_authenticated', 'hasCopilotAuth')
    },
    async fetchCopilotQuota() {
      return runProviderQuotaAction(this, 'copilot', false)
    },
    async refreshCopilotQuota() {
      return runProviderQuotaAction(this, 'copilot', true)
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
            this.$patch(applyConfiguredSourceQuotaResult(this, result))
          }
        } catch (e) {
          console.error('Failed to fetch configured source quotas:', e)
          if (activeSeq === this.configuredSourceRequestSeq) {
            this.$patch(applyConfiguredSourceQuotaFailure(this, e, Date.now()))
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
