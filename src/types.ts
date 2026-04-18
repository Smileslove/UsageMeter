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

// 模型价格配置
export interface ModelPricingConfig {
  modelId: string           // 模型ID，如 "claude-3-sonnet-20240229" 或 "minimax-m2-5"
  displayName?: string      // 显示名称（可选）
  inputPrice: number        // 输入价格 $/M tokens
  outputPrice: number       // 输出价格 $/M tokens
  cacheWritePrice?: number  // 缓存写入价格 $/M（可选）
  cacheReadPrice?: number   // 缓存读取价格 $/M（可选）
  source: 'api' | 'custom'  // 来源：API获取或用户自定义
  lastUpdated: number       // 最后更新时间戳
}

// 模型价格设置
export interface ModelPricingSettings {
  matchMode: 'fuzzy' | 'exact'        // 匹配方式：模糊或精确
  lastSyncTime: number | null         // 最后同步时间
  pricings: ModelPricingConfig[]      // 价格配置列表
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
  modelPricing: ModelPricingSettings  // 模型价格设置
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

// 会话统计
export interface SessionStats {
  sessionId: string
  totalRequests: number
  totalInputTokens: number
  totalOutputTokens: number
  totalCacheCreateTokens: number
  totalCacheReadTokens: number
  totalDurationMs: number
  avgOutputTokensPerSecond: number
  firstRequestTime: number
  lastRequestTime: number
  models: string[]
  // 扩展字段（Phase 2 添加）
  avgTtftMs?: number
  successRequests?: number
  errorRequests?: number
  estimatedCost?: number
  isCostEstimated?: boolean
  // JSONL 元信息（Phase 4 添加）
  cwd?: string
  projectName?: string  // 项目名称（从 cwd 提取）
  topic?: string        // 首个有意义用户消息
  lastPrompt?: string
  sessionName?: string  // 自定义会话名（customTitle 或 slug）
}

// 项目统计（聚合多个会话）
export interface ProjectStats {
  name: string
  sessionCount: number
  totalInputTokens: number
  totalOutputTokens: number
  totalCost: number
  lastActive: number
}

// 会话请求记录
export interface SessionRequest {
  timestamp: number
  messageId: string
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  model: string
  durationMs: number
  outputTokensPerSecond: number | null
  ttftMs: number | null
  statusCode: number
}
