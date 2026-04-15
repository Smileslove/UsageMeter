import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type { AlertEvent, AppSettings, ProxyStatus, ProxyUsageSnapshot, UsageSnapshot, WindowQuota, WindowRateSummary } from '../types'

const defaultQuotas: WindowQuota[] = [
  { window: '5h', enabled: true, tokenLimit: 500000, requestLimit: 500 },
  { window: '1d', enabled: false, tokenLimit: 1000000, requestLimit: 1000 },
  { window: '7d', enabled: true, tokenLimit: 5000000, requestLimit: 5000 },
  { window: '30d', enabled: true, tokenLimit: 20000000, requestLimit: 20000 },
  { window: 'current_month', enabled: true, tokenLimit: 30000000, requestLimit: 30000 }
]

const defaultSettings: AppSettings = {
  locale: 'zh-CN',
  timezone: 'Asia/Shanghai',
  refreshIntervalSeconds: 30,
  warningThreshold: 70,
  criticalThreshold: 90,
  billingType: 'both',
  quotas: defaultQuotas,
  summaryWindow: '5h',
  dataSource: 'ccusage',
  proxy: {
    enabled: false,
    port: 18765,
    autoStart: false,
    includeErrorRequests: true
  },
  theme: 'system'
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
    alertCooldownSeconds: 300
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
      } catch (e) {
        this.error = String(e)
      }
    },
    async saveSettings() {
      this.saving = true
      try {
        this.error = ''
        await invoke('save_settings', { settings: this.settings })
        this.startAutoRefresh()
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
      } catch (e) {
        this.error = String(e)
      } finally {
        this.loading = false
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
    }
  }
})
