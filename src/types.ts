export type AppLocale = 'zh-CN' | 'zh-TW' | 'en-US'

/** Stable pseudo-source used for Codex requests authenticated by official ChatGPT OAuth. */
export const OFFICIAL_OPENAI_OAUTH_SOURCE_ID = '__openai_official_oauth__'
/** Presentation-only marker emitted after the official quota endpoint confirms a plan. */
export const OPENAI_OAUTH_PLAN_LABEL_PREFIX = '__openai_oauth_plan:'
/** Stable pseudo-source used for Gemini CLI requests authenticated by official Google OAuth. */
export const OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID = '__google_official_gemini_oauth__'
/** Presentation-only marker emitted after the Gemini quota endpoint confirms a plan. */
export const GEMINI_OAUTH_PLAN_LABEL_PREFIX = '__gemini_oauth_plan:'
/** Stable pseudo-source used for Claude Code requests authenticated by Anthropic OAuth. */
export const OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID = '__anthropic_official_claude_oauth__'

export type WindowName = '5h' | '24h' | 'today' | '7d' | '30d' | 'current_month'

export const WINDOW_ORDER: WindowName[] = ['5h', '24h', 'today', '7d', '30d', 'current_month']

export type ThemeAppearance = 'light' | 'dark' | 'system'
export type ThemeLightPalette = 'dawn' | 'cloud' | 'mist' | 'moss' | 'parchment' | 'rose'
export type ThemeDarkPalette = 'midnight' | 'graphite' | 'forest'
export type ThemePalette = ThemeLightPalette | ThemeDarkPalette

export interface ThemePaletteOption {
  id: ThemePalette
  key: string
  preview: string[]
  family: 'light' | 'nature' | 'warm' | 'dark'
}

export interface ThemeSettings {
  appearance: ThemeAppearance
  lightPalette: ThemeLightPalette
  darkPalette: ThemeDarkPalette
}

export interface ProxyConfig {
  enabled: boolean
  port: number
  autoStart: boolean
  includeErrorRequests: boolean  // 在请求数统计中是否包含错误请求（4xx/5xx）
  requestTimeoutSeconds: number
  streamingIdleTimeoutSeconds: number
}

export type GatewayProtocol =
  | 'open_ai_chat_completions'
  | 'open_ai_responses'
  | 'anthropic_messages'
  | 'gemini_generate_content'

export type GatewayDispatchStrategy = 'round_robin' | 'random' | 'weighted' | 'priority_failover'

export interface GatewayUpstreamKey {
  id: string
  remark: string
  enabled: boolean
  weight: number
  priority: number
  lastUsedAtMs?: number | null
}

export interface GatewayLocalKey {
  id: string
  remark: string
  enabled: boolean
  createdAtMs: number
  lastUsedAtMs?: number | null
}

export interface GatewayCredentialRecovery {
  upstreamKeyRequired: boolean
  localKeyRotationRecommended: boolean
}

export interface GatewayProfile {
  id: string
  name: string
  protocol: GatewayProtocol
  baseUrl: string
  enabled: boolean
  clientLabel: string
  authMode: 'client_passthrough' | 'managed_keys'
  dispatchStrategy: GatewayDispatchStrategy
  upstreamKeys: GatewayUpstreamKey[]
  localKeys: GatewayLocalKey[]
  upstreamModels: GatewayUpstreamModel[]
  credentialRecovery: GatewayCredentialRecovery
}

export interface GatewaySettings {
  profiles: GatewayProfile[]
}

export interface GatewayStatus {
  proxyRunning: boolean
  routingActive: boolean
  enabledProfileCount: number
  listenerAddress?: string | null
}

export interface GatewayUpstreamModel {
  id: string
  name?: string
  ownedBy?: string
}

export interface GatewayUpstreamModelsResult {
  ok: boolean
  models: GatewayUpstreamModel[]
  latencyMs?: number
  errorKind?: string
  errorDetail?: string
}

export interface GatewayModelTestResult {
  ok: boolean
  modelId: string
  latencyMs?: number
  httpStatus?: number
  errorKind?: string
  errorDetail?: string
}

export interface GatewayBaseUrlPreview {
  basePath: string
  effectiveBaseUrl: string
  basePathAdded: boolean
  sampleRequestUrl: string
}

export type NetworkProxyScheme = 'http' | 'https' | 'socks5'

// 全局出站网络代理（与 ProxyConfig 即"本地接管"完全不同）
export interface NetworkProxyConfig {
  enabled: boolean        // false 时跟随系统代理；true 时强制走下方配置
  scheme: NetworkProxyScheme
  host: string
  port: number
  username?: string
  password?: string
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

export interface CurrencySettings {
  displayCurrency: string          // "USD"
  exchangeRates: Record<string, number>  // {"CNY": 7.25, "EUR": 0.92}
  trackedCurrencies: string[]      // ["USD", "CNY"]
  lastRateUpdate: number | null    // 最后汇率更新时间
}

export interface SyncSettings {
  enabled: boolean
  provider: 'webdav'
  url: string
  username: string
  password: string
  syncPassword: string
  deviceId: string
  intervalMinutes: number
  autoSync: boolean
  includeSessionText: boolean
}

export interface SyncStatus {
  enabled: boolean
  lastSyncAt: number | null
  lastStatus: string
  lastError?: string | null
  uploadedRequests: number
  importedRequests: number
  localRequestCount: number
  totalRequestCount: number
}

export interface RemoteSyncDevice {
  deviceId: string
  lastSeenAt: number | null
  lastExportSeq: number
  syncStatus: string
  updatedAt: number
}

export interface AppSettings {
  locale: AppLocale
  timezone: string
  refreshIntervalSeconds: number
  summaryWindow: WindowName  // 概览面板汇总展示区显示的窗口
  dayBoundaryMode: 'standard' | 'night_owl'
  numberFormat: NumberFormatMode  // 数值单位显示：国际单位 K/M 或中文单位 万/亿
  proxy: ProxyConfig         // 代理配置
  gateway: GatewaySettings   // 通用 API 网关配置
  theme: ThemeSettings       // 主题设置：外观模式 + 色板
  modelPricing: ModelPricingSettings  // 模型价格设置
  autoStart: boolean         // 开机自动启动
  sourceAware: SourceAwareSettings    // 来源识别设置
  clientTools: ClientToolSettings      // 客户端工具识别设置
  currency: CurrencySettings           // 多货币设置
  sync: SyncSettings                    // WebDAV 多端同步
  networkProxy: NetworkProxyConfig      // 全局出站网络代理
  autoCheckUpdate: boolean              // 启动时自动检查更新
  skippedUpdateVersion: string          // 已跳过的版本号（空字符串表示不跳过）
  wslScan: WslScanSettings              // WSL 被动扫描设置
  deepIndexLevel: string                // 深度索引级别：'off' | 'structured' | 'fulltext' | 'ondemand'
  deepIndexRetentionDays: number        // 深度索引保留期限（天，默认 90）
}

export type DayBoundaryMode = AppSettings['dayBoundaryMode']

export type NumberFormatMode = 'international' | 'chinese'

// WSL 被动扫描设置（仅 Windows 生效）
export interface WslScanSettings {
  enabled: boolean       // 是否启用
  distros: string[]      // 手动指定的发行版列表；空则自动枚举
  extraRoots: string[]   // 手动 UNC 根，兜底
}

// API 来源配置
export interface ApiSource {
  id: string
  displayName?: string
  baseUrl?: string
  apiKeyPrefixes: string[]
  apiKeyNotes?: Record<string, string>
  color: string
  icon?: string
  autoDetected: boolean
  quotaQuery?: SourceQuotaBindingConfig
  firstSeenMs: number
  lastSeenMs: number
}

export type SourceQueryProfileId =
  | 'generic_balance_v1_usage'
  | 'new_api_user_self'
  | 'official_deep_seek_balance'
  | 'official_step_fun_balance'
  | 'official_silicon_flow_balance_cn'
  | 'official_silicon_flow_balance_en'
  | 'official_open_router_balance'
  | 'official_novita_balance'
  | 'kimi_coding_plan'
  | 'zhipu_coding_plan'
  | 'mini_max_coding_plan'
  | 'zen_mux_coding_plan'

export type SourceCredentialStrategy =
  | 'tool_live_api_key'
  | 'manual_api_key'
  | 'tool_live_api_key_then_manual_api_key'
  | 'manual_access_token_user_id'

export interface SourceQuotaBindingConfig {
  enabled: boolean
  queryProfileId: SourceQueryProfileId
  credentialStrategy: SourceCredentialStrategy
  manualApiKey?: string
  manualAccessToken?: string
  manualUserId?: string
}

export interface SourceQuotaBindingTestResult {
  success: boolean
  attemptedProfileId?: SourceQueryProfileId
  recommendedProfileId?: SourceQueryProfileId
  credentialStrategy?: SourceCredentialStrategy
  sourceTool?: string
  summary?: string
  quota?: SubscriptionQuota
  error?: string
}

export type DetectionConfidence = 'high' | 'medium' | 'low'

export interface SourceQuotaBindingRuntimeState {
  sourceId: string
  recommendedProfileId?: SourceQueryProfileId
  detectionConfidence?: DetectionConfidence
  lastProbeAt?: number
  lastProbeError?: string
  lastVerifiedAt?: number
  lastTestSuccess?: boolean
  lastTestSummary?: string
  lastTestError?: string
  lastTestedProfileId?: SourceQueryProfileId
  lastTestedStrategy?: SourceCredentialStrategy
  sourceTool?: string
}

export type SourceQuotaProfileCategory =
  | 'genericBalance'
  | 'newApiBalance'
  | 'officialBalance'
  | 'codingPlan'

export type SourceQuotaExecutorKind =
  | 'genericBalanceV1Usage'
  | 'newApiUserSelf'
  | 'relayProvider'

export type SourceQuotaProbeKind =
  | 'genericBalanceV1Usage'
  | 'newApiUserSelf'

export interface SourceQuotaProfileDescriptor {
  profileId: SourceQueryProfileId
  labelKey: string
  category: SourceQuotaProfileCategory
  executorKind: SourceQuotaExecutorKind
  probeKind?: SourceQuotaProbeKind
  defaultCredentialStrategy: SourceCredentialStrategy
  supportedCredentialStrategies: SourceCredentialStrategy[]
}

// 来源感知设置
export interface SourceAwareSettings {
  sources: ApiSource[]
  activeSourceFilter: string | null  // null = 全部, "__unknown__" = 未归因, 也可为 OpenAI OAuth 伪来源
}

// 客户端工具接入配置
export interface ClientToolProfile {
  id: string
  tool: string
  displayName?: string
  pathPrefix: string
  targetBaseUrl?: string
  enabled: boolean
  autoDetected: boolean
  firstSeenMs: number
  lastSeenMs: number
  icon?: string
}

export interface ClientToolSettings {
  profiles: ClientToolProfile[]
  activeToolFilter: string | null
}

export interface ToolTakeoverStatus {
  tool: string
  enabled: boolean
  takeoverActive: boolean
  conflictPaused: boolean
  configPath?: string
  authPath?: string
  authMode?: 'api_key' | 'chat_gpt'
  officialProvider: boolean
  activeSourceId?: string
  managedProviderIds?: string[]
  conflictExternalBaseUrl?: string
  /** 当前礼让的外部配置管理器标识（如 "cc-switch"） */
  yieldedTo?: string
  scopeWarningKey?: string
  lastError?: string
}

/** cc-switch 供应商库单次清理的结果报告 */
export interface CcSwitchCleanReport {
  scanned: number
  cleaned: number
  unresolved: number
  skippedParseErrors: number
  backupPath?: string | null
  cleanedAtMs: number
}

/** cc-switch 兼容层状态快照 */
export interface CcSwitchCompatStatus {
  installed: boolean
  running: boolean
  dbExists: boolean
  yieldedTools: string[]
  lastCleanAtMs?: number | null
  lastReport?: CcSwitchCleanReport | null
  pendingClean: boolean
  /** 最近一次清理失败的错误码（如 ccswitchSchemaMismatch），成功后为空 */
  lastErrorCode?: string | null
}

export interface WindowUsage {
  window: string
  tokenUsed: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  requestUsed: number
  /** 该窗口的费用（美元） */
  cost: number
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
  totalSuccessRequests: number
  totalClientErrorRequests: number
  totalServerErrorRequests: number
}

export interface UsageSnapshot {
  generatedAtEpoch: number
  windows: WindowUsage[]
  source: 'local-files' | 'simulated' | string
  note?: string | null
  summary: UsageSummary
  modelDistribution: ModelUsage[]
}

export interface OverviewBreakdownCapability {
  hasSource: boolean
  hasTool: boolean
  hasCost: boolean
  hasStatus: boolean
  hasPerformance: boolean
}

export interface OverviewBreakdownItem {
  id: string
  label: string
  kind: 'source' | 'tool' | 'model'
  color?: string | null
  icon?: string | null
  requestCount: number
  totalTokens: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  cost: number
  percent: number
  successRequests?: number | null
  errorRequests?: number | null
  avgTokensPerSecond?: number | null
  avgTtftMs?: number | null
  lastSeenMs?: number | null
}

export interface OverviewBreakdown {
  window: string
  generatedAtEpoch: number
  sourceRanking: OverviewBreakdownItem[]
  toolRanking: OverviewBreakdownItem[]
  modelRanking: OverviewBreakdownItem[]
  capability: OverviewBreakdownCapability
}

export interface UsageRefreshBundle {
  generatedAtEpoch: number
  snapshot: UsageSnapshot
  limitSurvival: LimitSurvivalSnapshot
}

export interface OverviewDeferredBundle {
  window: string
  generatedAtEpoch: number
  windowUsage: WindowUsage
  usageSummary: UsageSummary
  modelDistribution: ModelUsage[]
  rateSummary: WindowRateSummary
  overviewBreakdown: OverviewBreakdown
}

// ============ Limit Survival Types ============

export type SurvivalConfidence = 'high' | 'medium' | 'low'
export type SurvivalSourceKind = 'baseline' | 'none'

/** 会话锚定的当前 5h 块 */
export interface AnchoredBlock {
  startEpoch: number
  resetsAtEpoch: number
  usedTokens: number
  usedRequests: number
  elapsedSeconds: number
  remainingSeconds: number
  projectedEndTokens: number
}

export interface BurnRate {
  tokensPerHour: number
  requestsPerHour: number
  sampleSeconds: number
  sampleRequests: number
  confidence: SurvivalConfidence
}

export interface SurvivalBaseline {
  avgTokensPerHour: number
  relativeToBaseline: number | null
}

/** 限额生存层本地信号（真实配额 T1 由前端用 subscriptionQuota 叠加） */
export interface LimitSurvivalSnapshot {
  generatedAtEpoch: number
  sourceKind: SurvivalSourceKind
  block: AnchoredBlock | null
  burn: BurnRate
  baseline: SurvivalBaseline | null
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
  tool: string
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
  usageFullyCovered?: boolean
  coveredRequests?: number
  uncoveredRequests?: number
  // JSONL 元信息（Phase 4 添加）
  cwd?: string
  projectName?: string  // 项目名称（从 cwd 提取）
  projectIdentity?: 'project' | 'global' | 'unknown'
  topic?: string        // 首个有意义用户消息
  lastPrompt?: string
  sessionName?: string  // 自定义会话名（customTitle 或 slug）
  scope?: 'project' | 'global' | string | null
  wslDistro?: string | null  // 来源 WSL 发行版名（非 WSL 会话为空）
}

// 项目统计（聚合多个会话）
export interface ProjectStats {
  name: string
  projectKey?: string | null
  projectIdentity?: 'project' | 'global' | 'unknown'
  projectPath?: string | null
  requestCount: number
  sessionCount: number
  totalInputTokens: number
  totalOutputTokens: number
  totalCacheCreateTokens: number
  totalCacheReadTokens: number
  totalCost: number
  lastActive: number
  usageFullyCovered?: boolean
  coveredRequests?: number
  uncoveredRequests?: number
  toolBreakdown: ProjectToolStats[]
  wslDistro?: string | null  // 来源 WSL 发行版名（项目内有 WSL 会话时，兼容字段）
  wslDistros?: string[]      // 项目内涉及的所有 WSL 发行版名
}

export interface ProjectToolStats {
  tool: string
  requestCount: number
  sessionCount: number
  totalInputTokens: number
  totalOutputTokens: number
  totalCacheCreateTokens: number
  totalCacheReadTokens: number
  totalCost: number
  lastActive: number
  usageFullyCovered?: boolean
  coveredRequests?: number
  uncoveredRequests?: number
}

export type RequestCoverageOrigin =
  | 'proxy_only'
  | 'local_only'
  | 'merged_proxy_preferred'
  | 'merged_fuzzy_matched'

export type RequestAttributionMethod =
  | 'unattributed'
  | 'config_inferred'
  | 'manual'

// 最近请求记录
export interface RequestRecord {
  requestKey: string
  sessionId: string
  projectName?: string | null
  projectPath?: string | null
  sourceLabel?: string | null
  attributionSourceId?: string | null
  attributionMethod: RequestAttributionMethod
  apiKeyPrefix?: string | null
  requestBaseUrl?: string | null
  tool: string
  timestampSec: number
  timestampMs: number
  model: string
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  totalTokens: number
  estimatedCost: number
  coverageOrigin: RequestCoverageOrigin
  statusCode?: number | null
  durationMs?: number | null
  outputTokensPerSecond: number | null
  ttftMs: number | null
}

export type RequestSortField = 'timestamp' | 'input' | 'output' | 'totalTokens' | 'cost' | 'duration' | 'rate' | 'ttft'
export type RequestSortDir = 'asc' | 'desc'

export interface RequestQueryParams {
  limit: number
  offset: number
  search?: string | null
  status?: string | null
  coverage?: string | null
  performance?: string | null
  sortField?: RequestSortField | null
  sortDir?: RequestSortDir | null
}

export interface RequestRecordsPage {
  items: RequestRecord[]
  total: number
  hasMore: boolean
}

// 统计面板
export type StatisticsMetric = 'cost' | 'requests' | 'tokens'

export type StatisticsBucket = 'hour' | 'day'

export type StatisticsRangePreset = '5h' | 'today' | '1d' | '7d' | '30d' | 'current_month' | 'custom'

export interface StatisticsQuery {
  startEpoch: number
  endEpoch: number
  timezone: string
  bucket: StatisticsBucket
}

export interface StatisticsRange {
  startEpoch: number
  endEpoch: number
  timezone: string
  bucket: StatisticsBucket
}

export interface StatisticsCapability {
  hasBasicUsage: boolean
  hasPerformance: boolean
  hasStatusCodes: boolean
}

export interface StatisticsTotals {
  requestCount: number
  totalTokens: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  cost: number
  modelCount: number
  localRequestCount: number
  proxyRequestCount: number
  successRequests?: number | null
  errorRequests?: number | null
}

export interface StatisticsTrendPoint {
  startEpoch: number
  label: string
  requestCount: number
  totalTokens: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  cost: number
  avgTokensPerSecond?: number | null
}

export interface StatisticsModelBreakdown {
  modelName: string
  requestCount: number
  localRequestCount: number
  estimatedRequestCount: number
  totalTokens: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  cost: number
  percent: number
  avgTokensPerSecond?: number | null
  avgTtftMs?: number | null
  errorRequests?: number | null
  successRequests?: number | null
  clientErrorRequests?: number | null
  serverErrorRequests?: number | null
  statusCodes: StatusCodeCount[]
  trend: StatisticsTrendPoint[]
}

export interface StatisticsPerformance {
  requestCount: number
  avgTokensPerSecond: number
  avgTtftMs: number
  slowestModel?: string | null
  fastestModel?: string | null
}

export interface StatisticsStatusBreakdown {
  successRequests: number
  clientErrorRequests: number
  serverErrorRequests: number
  successRate: number
}

export interface StatisticsSummary {
  generatedAtEpoch: number
  source: string
  capability: StatisticsCapability
  range: StatisticsRange
  totals: StatisticsTotals
  trend: StatisticsTrendPoint[]
  models: StatisticsModelBreakdown[]
  performance?: StatisticsPerformance | null
  status?: StatisticsStatusBreakdown | null
}

export interface DayActivity {
  date: string
  requestCount: number
  totalTokens: number
  inputTokens: number
  outputTokens: number
  cacheCreateTokens: number
  cacheReadTokens: number
  cost: number
  modelCount: number
  successRequests?: number | null
  errorRequests?: number | null
}

export interface MonthActivity {
  year: number
  month: number
  timezone: string
  metric: StatisticsMetric
  days: DayActivity[]
}

export interface YearActivity {
  year: number
  timezone: string
  metric: StatisticsMetric
  days: DayActivity[]
}

// 后端后台物化完成事件 activity_materialization_done 的 payload
// month 为后端 Option<u8>，序列化为 null（year 视图时恒为 null），故不能用可选字段省略。
export interface ActivityMaterializationDone {
  kind: 'month' | 'year'
  year: number
  month: number | null
  metric: StatisticsMetric
  ok: boolean
}

// ============ Subscription Types ============

export type QuotaKind = 'window' | 'balance'

/// Quota tier for a time window (5h or 7d) or a balance
export interface QuotaTier {
  name: string           // "five_hour" / "seven_day" / 余额币种
  kind?: QuotaKind       // 默认 window
  utilization: number    // Usage percentage 0-100（余额型可为 0）
  resetsAt?: string      // ISO 8601 format reset time
  remainingValue?: number | null  // 余额型：剩余额度
  maxValue?: number | null         // 余额型/套餐：上限
  currency?: string | null         // USD / CNY / credits
  limitReached?: boolean | null
}

/// Subscription quota data for different providers
export interface SubscriptionQuota {
  provider: string
  tool: string           // 官方："codex"/"claude_oauth"/"copilot"；中转：供应商 id（如 "deepseek"）
  sourceTool?: string     // 来源工具："claude-code" / "codex" / "opencode"（仅已配置来源额度查询填充）
  credentialStatus: string
  credentialMessage?: string
  success: boolean
  tiers: QuotaTier[]
  updatedAt: number
  fromCache: boolean
  error?: string
  /** Plan / tier label (Gemini), e.g. "Free" / "Pro" */
  planLabel?: string
  /** Account label such as email or project id (Gemini) */
  accountLabel?: string
}

/// Credential status for subscription queries
export type CredentialStatus =
  | 'notConfigured'
  | 'valid'
  | 'expired'
  | { refreshFailed: { error: string } }
  | { queryFailed: { error: string } }

/// Result of a subscription query
export interface SubscriptionQueryResult {
  success: boolean
  quota?: SubscriptionQuota
  credentialStatus: CredentialStatus
  error?: string
  queriedAt: number
}

export interface ConfiguredSourceQuotaQueryResult {
  quotas: SubscriptionQuota[]
  attemptedCount: number
  successCount: number
  failedCount: number
  errors: string[]
  queriedAt: number
}

export interface GitHubAccount {
  id: string
  login: string
  avatarUrl?: string | null
  authenticatedAt: number
  githubDomain: string
}

export interface GitHubDeviceCodeResponse {
  deviceCode: string
  userCode: string
  verificationUri: string
  expiresIn: number
  interval: number
}

export interface CopilotAuthStatus {
  accounts: GitHubAccount[]
  defaultAccountId?: string | null
  authenticated: boolean
  username?: string | null
  migrationError?: string | null
}

export type DesktopPage = 'overview' | 'analytics' | 'sessions' | 'projects' | 'requests' | 'activity' | 'gateway' | 'settings'

/** 时间桶过滤范围（秒级 epoch，半开区间 [startEpoch, endEpoch)，与统计趋势点/会话 lastRequestTime 口径一致）。 */
export interface DesktopTimeRange {
  startEpoch: number
  endEpoch: number
}

/** 主窗口深链导航目标（camelCase 字段与 Rust serde 对齐）。 */
export interface DesktopNavigationTarget {
  page: DesktopPage
  window?: string
  sourceId?: string
  tool?: string
  sessionKey?: string
  metric?: string
  view?: string
  /** 趋势下钻携带的时间桶范围（可选；Rust 侧 serde 忽略未知字段，深链载荷不传此字段）。 */
  timeRange?: DesktopTimeRange
}

// ============ M2 深度会话活动 DTO（字段 camelCase，与 src-tauri/src/activity/model.rs 对齐） ============

export type ActivityCapabilityLevel = 'none' | 'metadata' | 'structured' | 'fullContent'

export type AgentRelationLevel = 'none' | 'flagOnly' | 'rootGrouped' | 'fullTree'

export interface SessionActivityCapability {
  level: ActivityCapabilityLevel
  messages: boolean
  toolInvocations: boolean
  toolResults: boolean
  requestLinks: boolean
  agentRelations: AgentRelationLevel
  contentSearch: boolean
  sourceContentAvailable: boolean
  parserId: string
  parserVersion: number
}

export type SessionEventKind =
  | 'userMessage'
  | 'assistantMessage'
  | 'toolInvocation'
  | 'toolResult'
  | 'agentStarted'
  | 'agentFinished'
  | 'systemEvent'
  | 'compaction'
  | 'error'
  | 'unknown'

export type EventStatus = 'pending' | 'running' | 'success' | 'error' | 'cancelled' | 'unknown'

export type ContentState = 'none' | 'available' | 'redacted' | 'truncated' | 'unavailable'

export type RequestLinkStrength = 'exact' | 'sourceExplicit' | 'timeWindow'

export interface RequestEventLink {
  requestKey: string
  strength: RequestLinkStrength
}

export interface ToolInvocationSummary {
  invocationKey: string
  rawName: string
  normalizedName: string
  family: string
  durationMs: number | null
  inputBytes: number | null
  outputBytes: number | null
  inputKeys: string[]
  resultKind: string | null
}

/** 安全来源引用（sourceFilePath 已由后端脱敏：home → ~）。 */
export interface SafeSourceRef {
  sourceFileId: number | null
  sourceFilePath: string
  sourceOffset: number | null
  fingerprint: string | null
}

export interface SessionEventListItem {
  eventKey: string
  sessionKey: string
  sequence: number
  timestampMs: number | null
  kind: SessionEventKind
  status: EventStatus | null
  actorAgentKey: string | null
  parentEventKey: string | null
  summary: string | null
  contentState: ContentState
  tool: ToolInvocationSummary | null
  requestLinks: RequestEventLink[]
  sourceRef: SafeSourceRef
}

export interface AgentNodeDto {
  agentKey: string
  sessionKey: string
  parentAgentKey: string | null
  relationLevel: AgentRelationLevel
  displayKind: string | null
  taskSummary: string | null
  startedAtMs: number | null
  endedAtMs: number | null
  status: EventStatus
  requestCount: number | null
  totalTokens: number | null
  estimatedCost: number | null
  childCount: number
}

export interface SessionActivitySummary {
  sessionKey: string
  capability: SessionActivityCapability
  eventCounts: Record<string, number>
  agentCounts: Record<string, number>
  toolCounts: Array<[string, number]>
  coverage: string
}

export interface SessionEventFilter {
  kinds?: SessionEventKind[] | null
  toolNames?: string[] | null
  agents?: string[] | null
  statuses?: EventStatus[] | null
  search?: string | null
  minSequence?: number | null
  maxSequence?: number | null
}

export interface EventsPage {
  items: SessionEventListItem[]
  total: number
  hasMore: boolean
}

export interface ToolSummaryRow {
  toolName: string
  family: string
  invocationCount: number
  successCount: number
  errorCount: number
  totalDurationMs: number
  avgDurationMs: number
  inputBytesTotal: number
  outputBytesTotal: number
}

export interface RedactedPayloadPage {
  content: string
  truncated: boolean
  nextCursor: string | null
  contentState: ContentState
}

export interface RebuildResult {
  sessionsIndexed: number
  sessionsFailed: number
  eventsWritten: number
  errors: string[]
}

// ============ M3 深度上下文 DTO（字段 camelCase，与 src-tauri/src/commands/activity.rs 对齐） ============

/** 会话活动导出选项（后端 serde(default) 兼容缺省字段；payload 默认不包含，前端双保险）。 */
export interface ExportOptions {
  format: 'json' | 'csv'
  includeSummaries: boolean
  includeToolSummaries: boolean
  includeRequestLinks: boolean
  includePayloads: boolean
}

/** 会话活动导出结果。 */
export interface ExportResult {
  filePath: string
  rowCount: number
  payloadIncluded: boolean
  truncated: boolean
}

/** 跨会话全文搜索单条命中（内容均为后端脱敏字段）。 */
export interface GlobalSearchHit {
  sessionKey: string
  sessionTitle?: string | null
  eventKey: string
  kind: string
  status?: string | null
  summary?: string | null
  timestampMs?: number | null
  toolName?: string | null
}

/** 跨会话全文搜索分页结果。 */
export interface GlobalSearchPage {
  items: GlobalSearchHit[]
  total: number
  hasMore: boolean
}
