import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'

export interface UpdateInfo {
  version: string
  currentVersion: string
  body: string | null
  date: string | null
}

export const useUpdaterStore = defineStore('updater', {
  state: () => ({
    status: 'idle' as 'idle' | 'checking' | 'available' | 'downloading' | 'error',
    updateInfo: null as UpdateInfo | null,
    downloadedBytes: 0,
    totalBytes: null as number | null,
    errorMessage: null as string | null,
    isExpanded: false,
  }),

  getters: {
    hasUpdate: (state): boolean =>
      state.status === 'available' || state.status === 'downloading' ||
      (state.status === 'error' && state.updateInfo != null),

    downloadProgress: (state): number => {
      if (!state.totalBytes || state.totalBytes === 0) return 0
      return Math.min(100, Math.round((state.downloadedBytes / state.totalBytes) * 100))
    },

    formattedDownloaded: (state): string => formatBytes(state.downloadedBytes),

    formattedTotal: (state): string =>
      state.totalBytes != null ? formatBytes(state.totalBytes) : '',
  },

  actions: {
    async checkForUpdate(): Promise<void> {
      this.status = 'checking'
      this.errorMessage = null
      try {
        const result = await invoke<UpdateInfo | null>('check_for_update')
        if (result) {
          this.updateInfo = result
          this.status = 'available'
        } else {
          this.status = 'idle'
        }
      } catch {
        this.status = 'error'
        this.errorMessage = 'checkFailed'
      }
    },

    async downloadAndInstall(): Promise<void> {
      this.status = 'downloading'
      this.downloadedBytes = 0
      this.totalBytes = null
      this.errorMessage = null
      try {
        await invoke('download_and_install_update')
        // app.restart() 在 Rust 侧调用，前端不会执行到此处
      } catch {
        this.status = 'error'
        this.errorMessage = 'downloadFailed'
      }
    },

    async skipVersion(): Promise<void> {
      if (!this.updateInfo) return
      try {
        await invoke('skip_update_version', { version: this.updateInfo.version })
        this.reset()
      } catch {
        this.errorMessage = 'checkFailed'
      }
    },

    onUpdateAvailable(info: UpdateInfo): void {
      this.updateInfo = info
      this.status = 'available'
    },

    onDownloadProgress(downloadedBytes: number, totalBytes: number | null): void {
      this.downloadedBytes = downloadedBytes
      if (totalBytes != null) {
        this.totalBytes = totalBytes
      }
    },

    toggleExpanded(): void {
      this.isExpanded = !this.isExpanded
    },

    reset(): void {
      this.status = 'idle'
      this.updateInfo = null
      this.downloadedBytes = 0
      this.totalBytes = null
      this.errorMessage = null
      this.isExpanded = false
    },
  },
})

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}
