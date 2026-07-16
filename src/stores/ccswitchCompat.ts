import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type { CcSwitchCleanReport, CcSwitchCompatStatus } from '../types'

/**
 * cc-switch 兼容层状态。
 *
 * 只保存结构化状态与错误码，不含任何自然语言文案（文案由组件通过 i18n key 渲染）。
 */
export const useCcSwitchCompatStore = defineStore('ccswitchCompat', {
  state: () => ({
    installed: false,
    running: false,
    dbExists: false,
    yieldedTools: [] as string[],
    lastCleanAtMs: null as number | null,
    lastReport: null as CcSwitchCleanReport | null,
    pendingClean: false,
    /** 后端持久化的最近一次自动清理失败错误码（如 ccswitchSchemaMismatch） */
    lastErrorCode: null as string | null,
    loading: false,
    cleaning: false,
    /** 后端返回的错误码（如 ccswitchRunning），由组件映射到 i18n key */
    cleanErrorCode: null as string | null,
    initialized: false
  }),
  actions: {
    async refresh() {
      if (this.loading) return
      this.loading = true
      try {
        const status = await invoke<CcSwitchCompatStatus>('get_ccswitch_compat_status')
        this.installed = status.installed
        this.running = status.running
        this.dbExists = status.dbExists
        this.yieldedTools = status.yieldedTools
        this.lastCleanAtMs = status.lastCleanAtMs ?? null
        this.lastReport = status.lastReport ?? null
        this.pendingClean = status.pendingClean
        this.lastErrorCode = status.lastErrorCode ?? null
        this.initialized = true
      } catch (e) {
        console.error('Failed to fetch cc-switch compat status:', e)
      } finally {
        this.loading = false
      }
    },
    async runClean() {
      if (this.cleaning) return
      this.cleaning = true
      this.cleanErrorCode = null
      try {
        const report = await invoke<CcSwitchCleanReport>('run_ccswitch_db_clean')
        this.lastReport = report
        this.lastCleanAtMs = report.cleanedAtMs
        this.pendingClean = false
      } catch (e) {
        this.cleanErrorCode = String(e)
      } finally {
        this.cleaning = false
        await this.refresh()
      }
    }
  }
})
