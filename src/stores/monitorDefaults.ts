import type {
  AppSettings,
  ClientToolSettings,
  CurrencySettings,
  ModelPricingSettings,
  NetworkProxyConfig,
  SourceAwareSettings,
  SyncSettings,
  ThemeSettings,
  WslScanSettings
} from '../types'

const createDefaultModelPricing = (): ModelPricingSettings => ({ matchMode: 'fuzzy', lastSyncTime: null, pricings: [] })
const createDefaultSourceAware = (): SourceAwareSettings => ({ sources: [], activeSourceFilter: null })
const createDefaultCurrency = (): CurrencySettings => ({
  displayCurrency: 'USD', exchangeRates: { USD: 1.0 }, trackedCurrencies: ['USD'], lastRateUpdate: null
})
const createDefaultSync = (): SyncSettings => ({
  enabled: false, provider: 'webdav', url: '', username: '', password: '', syncPassword: '', deviceId: '',
  intervalMinutes: 15, autoSync: false, includeSessionText: false
})
const createDefaultWslScan = (): WslScanSettings => ({ enabled: false, distros: [], extraRoots: [] })
const createDefaultNetworkProxy = (): NetworkProxyConfig => ({
  enabled: false, scheme: 'http', host: '127.0.0.1', port: 7890, username: undefined, password: undefined
})
const createDefaultTheme = (): ThemeSettings => ({ appearance: 'system', lightPalette: 'cloud', darkPalette: 'midnight' })

const createDefaultClientTools = (): ClientToolSettings => ({
  profiles: [
    { id: 'claude_code', tool: 'claude_code', displayName: 'Claude Code', pathPrefix: 'claude-code', enabled: true, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'claudecode' },
    { id: 'codex', tool: 'codex', displayName: 'Codex', pathPrefix: 'codex', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'codex' },
    { id: 'deepseek_harness', tool: 'deepseek_harness', displayName: 'DeepSeek Harness', pathPrefix: 'deepseek-harness', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'deepseek' },
    { id: 'hermes', tool: 'hermes', displayName: 'Hermes Agent', pathPrefix: 'hermes', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'hermesagent' },
    { id: 'pi', tool: 'pi', displayName: 'Pi Agent', pathPrefix: 'pi', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'pi' },
    { id: 'openclaw', tool: 'openclaw', displayName: 'OpenClaw', pathPrefix: 'openclaw', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'openclaw' },
    { id: 'opencode', tool: 'opencode', displayName: 'OpenCode', pathPrefix: 'opencode', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'opencode' },
    { id: 'qoder_ide', tool: 'qoder_ide', displayName: 'Qoder IDE', pathPrefix: 'qoder', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'qoder' },
    { id: 'reasonix', tool: 'reasonix', displayName: 'Reasonix', pathPrefix: 'reasonix', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'reasonix' },
    { id: 'gemini', tool: 'gemini', displayName: 'Gemini CLI', pathPrefix: 'gemini', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'geminicli' },
    { id: 'copilot', tool: 'copilot', displayName: 'GitHub Copilot CLI', pathPrefix: 'copilot', enabled: false, autoDetected: false, firstSeenMs: 0, lastSeenMs: 0, icon: 'copilot' }
  ],
  activeToolFilter: null
})

export function createDefaultSettings(): AppSettings {
  return {
    locale: 'zh-CN', timezone: 'Asia/Shanghai', refreshIntervalSeconds: 30, summaryWindow: '24h',
    dayBoundaryMode: 'standard', numberFormat: 'international',
    proxy: { enabled: false, port: 18765, autoStart: false, includeErrorRequests: true, requestTimeoutSeconds: 120, streamingIdleTimeoutSeconds: 0 },
    gateway: { profiles: [] }, theme: createDefaultTheme(), modelPricing: createDefaultModelPricing(), autoStart: false,
    sourceAware: createDefaultSourceAware(), clientTools: createDefaultClientTools(), currency: createDefaultCurrency(),
    sync: createDefaultSync(), networkProxy: createDefaultNetworkProxy(), autoCheckUpdate: true, skippedUpdateVersion: '',
    wslScan: createDefaultWslScan(),
    deepseekHarnessSessionRoot: null,
    deepIndexLevel: 'off', deepIndexRetentionDays: 90
  }
}

export const CONFIGURED_SOURCE_SUCCESS_REFRESH_MS = 5 * 60 * 1000
export const CONFIGURED_SOURCE_FAILURE_BASE_MS = 2 * 60 * 1000
export const CONFIGURED_SOURCE_FAILURE_MAX_MS = 30 * 60 * 1000
