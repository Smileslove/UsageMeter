export type AppLocale = 'zh-CN' | 'zh-TW' | 'en-US'

export type BillingType = 'token' | 'request' | 'both'

export type WindowName = '5h' | '1d' | '7d' | '30d' | 'current_month'

export type DataSource = 'ccusage' | 'proxy'

export type ThemeMode = 'light' | 'dark' | 'system'

export interface ProxyConfig {
  enabled: boolean
  port: number
  autoStart: boolean
  includeErrorRequests: boolean  // 在请求数统计中是否包含错误请求（4xx/5xx）
}

export interface WindowQuota {
  window: WindowName
  enabled: boolean
  tokenLimit: number | null
  requestLimit: number | null
}

export interface AppSettings {
  locale: AppLocale
  timezone: string
  refreshIntervalSeconds: number
  warningThreshold: number
  criticalThreshold: number
  billingType: BillingType
  quotas: WindowQuota[]
  summaryWindow: WindowName  // 概览面板汇总展示区显示的窗口
  dataSource: DataSource     // 数据统计方式：ccusage 或 proxy
  proxy: ProxyConfig         // 代理配置
  theme: ThemeMode           // 主题模式：light/dark/system
}

export interface WindowUsage {
  window: string
  tokenUsed: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  requestUsed: number
  tokenLimit: number | null
  requestLimit: number | null
  tokenPercent: number | null
  requestPercent: number | null
  riskLevel: 'safe' | 'warning' | 'critical'
  successRequests: number
  clientErrorRequests: number
  serverErrorRequests: number
}

export interface StatusCodeCount {
  statusCode: number
  count: number
}

export interface ModelUsage {
  modelName: string
  tokenUsed: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  requestCount: number
  percent: number
  statusCodes: StatusCodeCount[]
}

export interface UsageSummary {
  totalTokens: number
  totalRequests: number
  totalInputTokens: number
  totalOutputTokens: number
  totalCacheCreateTokens: number
  totalCacheReadTokens: number
  totalCost: number
  overallRiskLevel: 'safe' | 'warning' | 'critical'
  totalSuccessRequests: number
  totalClientErrorRequests: number
  totalServerErrorRequests: number
}

export interface UsageSnapshot {
  generatedAtEpoch: number
  windows: WindowUsage[]
  source: 'ccusage' | 'simulated' | string
  note?: string | null
  summary: UsageSummary
  modelDistribution: ModelUsage[]
}

export interface AlertEvent {
  level: 'safe' | 'warning' | 'critical'
  source: 'ccusage-api' | 'local-jsonl' | 'no-data' | 'simulated' | 'proxy' | 'unknown'
  createdAtEpoch: number
}

// 代理相关类型
export interface ProxyStatus {
  running: boolean
  port: number
  uptimeSeconds: number
  totalRequests: number
  successRequests: number
  failedRequests: number
  activeConnections: number
  configTakenOver: boolean
  recordCount: number
  status2xx: number
  status4xx: number
  status5xx: number
}

export interface ProxyWindowUsage {
  window: string
  tokenUsed: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  requestUsed: number
  successRequests: number
  clientErrorRequests: number
  serverErrorRequests: number
}

export interface ProxyUsageSnapshot {
  generatedAtEpoch: number
  windows: ProxyWindowUsage[]
  source: string
}

// Token 生成速率统计
export interface ModelRateStats {
  modelName: string
  requestCount: number
  totalOutputTokens: number
  totalDurationMs: number
  avgTokensPerSecond: number
  minTokensPerSecond: number
  maxTokensPerSecond: number
}

export interface OverallRateStats {
  requestCount: number
  totalOutputTokens: number
  totalDurationMs: number
  avgTokensPerSecond: number
}

export interface WindowRateSummary {
  window: string
  overall: OverallRateStats
  byModel: ModelRateStats[]
  ttft: TtftStats
  ttftByModel: ModelTtftStats[]
}

// TTFT 统计（首 Token 生成时间）
export interface TtftStats {
  requestCount: number
  avgTtftMs: number
  minTtftMs: number
  maxTtftMs: number
}

// 单模型 TTFT 统计
export interface ModelTtftStats {
  modelName: string
  requestCount: number
  avgTtftMs: number
  minTtftMs: number
  maxTtftMs: number
}
