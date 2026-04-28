import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type { AlertEvent, AppSettings, ClientToolSettings, ModelPricingSettings, MonthActivity, ProjectStats, ProxyStatus, ProxyUsageSnapshot, SessionStats, StatisticsMetric, StatisticsQuery, StatisticsSummary, UsageSnapshot, WindowQuota, WindowRateSummary, YearActivity, SourceAwareSettings } from '../types'

const defaultQuotas: WindowQuota[] = [
  { window: '5h', enabled: true, tokenLimit: 500000, requestLimit: 500 },
  { window: '24h', enabled: true, tokenLimit: 1000000, requestLimit: 1000 },
  { window: 'today', enabled: true, tokenLimit: 1000000, requestLimit: 1000 },
  { window: '7d', enabled: true, tokenLimit: 5000000, requestLimit: 5000 },
  { window: '30d', enabled: true, tokenLimit: 20000000, requestLimit: 20000 },
  { window: 'current_month', enabled: true, tokenLimit: 30000000, requestLimit: 30000 }
]

const defaultModelPricing: ModelPricingSettings = {
  matchMode: 'fuzzy',
  lastSyncTime: null,
  pricings: []
}

const defaultSourceAware: SourceAwareSettings = {
  sources: [],
  activeSourceFilter: null
}

const defaultClientTools: ClientToolSettings = {
  profiles: [
    { id: 'claude_code', tool: 'claude_code', displayName: 'Claude Code', pathPrefix: 'claude-code', enabled: true, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'claudecode' },
    { id: 'codex', tool: 'codex', displayName: 'Codex', pathPrefix: 'codex', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'codex' },
    { id: 'cursor', tool: 'cursor', displayName: 'Cursor', pathPrefix: 'cursor', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'cursor' },
    { id: 'opencode', tool: 'opencode', displayName: 'OpenCode', pathPrefix: 'opencode', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'opencode' }
  ],
  activeToolFilter: null
}

function invokeWithTimeout<T>(command: string, args: Record<string, unknown>, timeoutMs = 120000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | null = null
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject('ERR_STATISTICS_TIMEOUT'), timeoutMs)
  })

  return Promise.race([invoke<T>(command, args), timeout]).finally(() => {
    if (timer) {
      clearTimeout(timer)
    }
  })
}

const defaultSettings: AppSettings = {
  locale: 'zh-CN',
  timezone: 'Asia/Shanghai',
  refreshIntervalSeconds: 30,
  warningThreshold: 70,
  criticalThreshold: 90,
  billingType: 'both',
  quotas: defaultQuotas,
  summaryWindow: '24h',
  dataSource: 'ccusage',
  proxy: {
    enabled: false,
    port: 18765,
    autoStart: false,
    includeErrorRequests: true
  },
  theme: 'system',
  modelPricing: defaultModelPricing,
  autoStart: false,
  sourceAware: defaultSourceAware,
  clientTools: defaultClientTools
}

export const useMonitorStore = defineStore('monitor', {
  state: () => ({
    settings: defaultSettings as AppSettings,
    snapshot: null as UsageSnapshot | null,
    proxyStatus: null as ProxyStatus | null,
    proxyUsage: null as ProxyUsageSnapshot | null,
    rateSummary: null as WindowRateSummary | null,
    loading: false,
    saving: false,
    proxyLoading: false,
    error: '' as string,
    lastUpdatedEpoch: null as number | null,
    refreshTimer: null as ReturnType<typeof setInterval> | null,
    alerts: [] as AlertEvent[],
    trendHistory: {} as Record<string, number[]>,
    lastAlertLevel: 'safe' as 'safe' | 'warning' | 'critical',
    lastAlertEpoch: 0,
    alertCooldownSeconds: 300,
    // 会话相关状态
    sessions: [] as SessionStats[],
    sessionsLoading: false,
    selectedSession: null as SessionStats | null,
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
    monthActivityRequestSeq: 0,
    yearActivityRequestSeq: 0
  }),
  getters: {
    hasData: state => !!state.snapshot,
    windows: state => state.snapshot?.windows ?? [],
    isProxyRunning: state => state.proxyStatus?.running ?? false,
    isProxyMode: state => state.settings.dataSource === 'proxy'
  },
  actions: {
    async initialize() {
      await this.loadSettings()
      await this.refreshUsage()

      // 如果代理模式已启用，自动启动代理服务器
      if (this.settings.proxy.enabled) {
        try {
          await this.startProxyOnly(this.settings.proxy.port)
        } catch (e) {
          console.error('Failed to auto-start proxy:', e)
          // 启动失败时，更新状态但不阻止应用启动
          this.settings.proxy.enabled = false
        }
      }

      // 如果是代理模式，获取速率数据（解决初始化时序问题）
      if (this.settings.dataSource === 'proxy' && this.settings.summaryWindow) {
        await this.fetchRateSummary(this.settings.summaryWindow)
      }
    },
    async loadSettings() {
      try {
        this.error = ''
        const data = await invoke<AppSettings>('load_settings')
        this.settings = data

        // 确保代理配置存在（迁移兼容）
        if (!this.settings.proxy) {
          this.settings.proxy = {
            enabled: false,
            port: 18765,
            autoStart: false,
            includeErrorRequests: true
          }
        } else {
          // 确保所有字段存在（迁移旧配置兼容）
          if (!this.settings.proxy.port || this.settings.proxy.port === 0) {
            this.settings.proxy.port = 18765
          }
          if (this.settings.proxy.includeErrorRequests === undefined) {
            this.settings.proxy.includeErrorRequests = true
          }
        }

        // 确保模型价格配置存在（迁移兼容）
        if (!this.settings.modelPricing) {
          this.settings.modelPricing = defaultModelPricing
        } else {
          if (this.settings.modelPricing.matchMode === undefined) {
            this.settings.modelPricing.matchMode = 'fuzzy'
          }
          if (!this.settings.modelPricing.pricings) {
            this.settings.modelPricing.pricings = []
          }
        }

        // 确保来源识别配置存在（迁移兼容）
        if (!this.settings.sourceAware) {
          this.settings.sourceAware = defaultSourceAware
        }
        if (!this.settings.clientTools) {
          this.settings.clientTools = defaultClientTools
        } else {
          if (!this.settings.clientTools.profiles?.length) {
            this.settings.clientTools.profiles = defaultClientTools.profiles
          }
          if (this.settings.clientTools.activeToolFilter === undefined) {
            this.settings.clientTools.activeToolFilter = null
          }
        }
      } catch (e) {
        this.error = String(e)
      }
    },
    async saveSettings() {
      this.saving = true
      try {
        this.error = ''
        await invoke('save_settings', { settings: this.settings })
        // 不在这里调用 startAutoRefresh()，避免在设置页面时触发刷新
      } catch (e) {
        this.error = String(e)
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

        // 统一调用 - 后端会根据 settings.dataSource 选择代理或 ccusage
        const data = await invoke<UsageSnapshot>('get_usage_snapshot', {
          settings: this.settings
        })
        this.snapshot = data

        this.lastUpdatedEpoch = this.snapshot.generatedAtEpoch
        this.updateTrendHistory(this.snapshot)
        this.evaluateAlerts(this.snapshot)

        // 如果是代理模式，刷新速率数据
        if (this.settings.dataSource === 'proxy' && this.settings.summaryWindow) {
          await this.fetchRateSummary(this.settings.summaryWindow)
        }
      } catch (e) {
        this.error = String(e)
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
    async fetchStatisticsSummary(query: StatisticsQuery) {
      const requestSeq = ++this.statisticsRequestSeq
      this.statisticsLoading = true
      try {
        this.statisticsError = ''
        const summary = await invokeWithTimeout<StatisticsSummary>('get_statistics_summary', {
          query,
          settings: this.settings
        })
        if (requestSeq === this.statisticsRequestSeq) {
          this.statisticsSummary = summary
        }
      } catch (e) {
        if (requestSeq === this.statisticsRequestSeq) {
          this.statisticsError = String(e)
        }
      } finally {
        if (requestSeq === this.statisticsRequestSeq) {
          this.statisticsLoading = false
        }
      }
    },
    async fetchMonthActivity(year: number, month: number, metric: StatisticsMetric) {
      const requestSeq = ++this.monthActivityRequestSeq
      this.monthActivityLoading = true
      try {
        this.statisticsError = ''
        const activity = await invokeWithTimeout<MonthActivity>('get_month_activity', {
          year,
          month,
          metric,
          settings: this.settings
        })
        if (requestSeq === this.monthActivityRequestSeq) {
          this.monthActivity = activity
        }
      } catch (e) {
        if (requestSeq === this.monthActivityRequestSeq) {
          this.statisticsError = String(e)
        }
      } finally {
        if (requestSeq === this.monthActivityRequestSeq) {
          this.monthActivityLoading = false
        }
      }
    },
    async fetchYearActivity(year: number, metric: StatisticsMetric) {
      const requestSeq = ++this.yearActivityRequestSeq
      this.yearActivityLoading = true
      try {
        this.statisticsError = ''
        const activity = await invokeWithTimeout<YearActivity>('get_year_activity', {
          year,
          metric,
          settings: this.settings
        })
        if (requestSeq === this.yearActivityRequestSeq) {
          this.yearActivity = activity
        }
      } catch (e) {
        if (requestSeq === this.yearActivityRequestSeq) {
          this.statisticsError = String(e)
        }
      } finally {
        if (requestSeq === this.yearActivityRequestSeq) {
          this.yearActivityLoading = false
        }
      }
    },
    calculatePercent(used: number, limit: number | null): number | null {
      if (limit === null || limit === 0) return null
      return (used / limit) * 100
    },
    calculateRiskLevel(tokenPercent: number | null, requestPercent: number | null): 'safe' | 'warning' | 'critical' {
      const max = Math.max(tokenPercent ?? 0, requestPercent ?? 0)
      if (max >= this.settings.criticalThreshold) return 'critical'
      if (max >= this.settings.warningThreshold) return 'warning'
      return 'safe'
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
        this.error = String(e)
        throw e
      } finally {
        this.proxyLoading = false
      }
    },
    async startProxy(port?: number) {
      this.proxyLoading = true
      try {
        this.error = ''
        const proxyPort = port ?? this.settings.proxy.port ?? 18765
        await invoke('start_proxy', { port: proxyPort })
        this.settings.proxy.enabled = true
        this.settings.proxy.port = proxyPort
        await this.saveSettings()
        await this.getProxyStatus()
      } catch (e) {
        this.error = String(e)
      } finally {
        this.proxyLoading = false
      }
    },
    async stopProxy() {
      this.proxyLoading = true
      try {
        this.error = ''
        await invoke('stop_proxy')
        this.settings.proxy.enabled = false
        await this.saveSettings()
        await this.getProxyStatus()
      } catch (e) {
        this.error = String(e)
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
    async toggleProxy() {
      if (this.isProxyRunning) {
        await this.stopProxy()
      } else {
        await this.startProxy()
      }
    },
    startAutoRefresh() {
      this.stopAutoRefresh()
      const interval = Math.max(5, this.settings.refreshIntervalSeconds) * 1000
      this.refreshTimer = setInterval(() => {
        this.refreshUsage()
        if (this.settings.proxy.enabled) {
          this.getProxyStatus()
        }
      }, interval)
    },
    stopAutoRefresh() {
      if (this.refreshTimer) {
        clearInterval(this.refreshTimer)
        this.refreshTimer = null
      }
    },
    evaluateAlerts(snapshot: UsageSnapshot) {
      const maxLevel = snapshot.windows.reduce<'safe' | 'warning' | 'critical'>((acc, current) => {
        if (current.riskLevel === 'critical') return 'critical'
        if (current.riskLevel === 'warning' && acc === 'safe') return 'warning'
        return acc
      }, 'safe')

      const now = Math.floor(Date.now() / 1000)
      const cooldownPassed = now - this.lastAlertEpoch >= this.alertCooldownSeconds

      if (maxLevel !== 'safe' && (maxLevel !== this.lastAlertLevel || cooldownPassed)) {
        const source = snapshot.source === 'ccusage-api' || snapshot.source === 'local-jsonl' || snapshot.source === 'no-data' || snapshot.source === 'simulated' || snapshot.source === 'proxy' ? snapshot.source : 'unknown'
        this.alerts.unshift({
          level: maxLevel,
          source,
          createdAtEpoch: now
        })
        this.alerts = this.alerts.slice(0, 20)
        this.lastAlertEpoch = now
      }

      this.lastAlertLevel = maxLevel
    },
    updateTrendHistory(snapshot: UsageSnapshot) {
      for (const window of snapshot.windows) {
        const current = this.trendHistory[window.window] ?? []
        const percent = Math.max(window.tokenPercent ?? 0, window.requestPercent ?? 0)
        const next = [...current, Math.max(0, Math.min(100, percent))].slice(-24)
        this.trendHistory[window.window] = next
      }
    },
    // 速率统计操作（仅代理模式）
    async fetchRateSummary(window: string) {
      if (this.settings.dataSource !== 'proxy') {
        this.rateSummary = null
        return
      }

      try {
        this.rateSummary = await invoke<WindowRateSummary>('get_window_rate_summary', { window })
      } catch (e) {
        console.error('Failed to fetch rate summary:', e)
        // 出错时返回空统计
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
    },
    /**
     * 准备退出：停止代理并恢复 Claude 配置
     * 在应用退出前调用，确保用户可以正常使用 Claude
     */
    async prepareExit() {
      // 如果代理正在运行，先停止并恢复配置
      if (this.isProxyRunning) {
        try {
          await invoke('stop_proxy')
          this.settings.proxy.enabled = false
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
        const newSessions = await invoke<SessionStats[]>('get_sessions', { limit, offset, settings: this.settings })
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
        this.selectedSession = await invoke<SessionStats | null>('get_session_detail', { sessionId, settings: this.settings })
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
    /**
     * 获取项目统计（基于所有会话聚合，不受分页影响）
     */
    async fetchProjectStats() {
      this.projectStatsLoading = true
      try {
        this.projectStats = await invoke<ProjectStats[]>('get_project_stats', { settings: this.settings })
      } catch (e) {
        console.error('Failed to fetch project stats:', e)
        this.projectStats = []
      } finally {
        this.projectStatsLoading = false
      }
    },
    // === 来源管理 ===
    /**
     * 设置当前激活的来源过滤器
     */
    async setActiveSourceFilter(sourceId: string | null) {
      this.settings.sourceAware.activeSourceFilter = sourceId
      await this.saveSettings()
      await this.refreshUsage()
    },
    /**
     * 设置当前激活的工具过滤器
     */
    async setActiveToolFilter(toolId: string | null) {
      this.settings.clientTools.activeToolFilter = toolId
      await this.saveSettings()
      await this.refreshUsage()
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
      await invoke('rename_api_source', { sourceId, name })
      await this.loadSettings()
    },
    /**
     * 删除来源
     */
    async deleteSource(sourceId: string, alsoDeleteRecords: boolean = false) {
      await invoke('delete_api_source', { sourceId, alsoDeleteRecords })
      await this.loadSettings()
      await this.refreshUsage()
    },
    /**
     * 合并两个来源
     */
    async mergeSource(sourceIdFrom: string, sourceIdInto: string) {
      await invoke('merge_api_source', { sourceIdFrom, sourceIdInto })
      await this.loadSettings()
      await this.refreshUsage()
    },
    /**
     * 添加 Key 前缀到来源
     */
    async addKeyPrefixToSource(sourceId: string, keyPrefix: string) {
      await invoke('add_key_prefix_to_source', { sourceId, keyPrefix })
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
      await invoke('update_api_source_key_note', { sourceId, keyPrefix, note })
      await this.loadSettings()
    }
  }
})
