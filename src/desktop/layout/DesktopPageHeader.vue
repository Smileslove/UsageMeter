<script setup lang="ts">
import { computed } from 'vue'
import { RefreshCw, Share2 } from 'lucide-vue-next'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { useMonitorStore } from '../../stores/monitor'
import { sourceLabel, t } from '../../i18n'
import {
  OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID,
  OFFICIAL_OPENAI_OAUTH_SOURCE_ID
} from '../../types'
import { formatToolDisplayName } from '../../utils/toolDisplay'
import { openShareWindow } from '../../api/appApi'
import type { DesktopPage } from '../../types'
import SourceSelector from '../../components/SourceSelector.vue'
import ToolSelector from '../../components/ToolSelector.vue'
import ThemeSelector from '../../components/ThemeSelector.vue'

/** 全局筛选摘要仅作用于数据受筛选影响的页面（设计文档 4.5）。 */
const PAGES_WITH_FILTER_SUMMARY: DesktopPage[] = ['overview', 'analytics', 'sessions', 'projects', 'requests']

const nav = useDesktopNavigationStore()
const monitor = useMonitorStore()

const locale = computed(() => monitor.settings.locale)
const currentPage = computed(() => nav.currentPage)
const pageTitle = computed(() => t(locale.value, `desktop.nav.${currentPage.value}`))
const showFilterSummary = computed(() => PAGES_WITH_FILTER_SUMMARY.includes(currentPage.value))

// 全局筛选状态摘要（M1：仅单选语义，与现有 SourceSelector/ToolSelector 行为一致）
const sourceFilterLabel = computed(() => {
  const id = monitor.settings.sourceAware.activeSourceFilter
  if (!id || id === '__unknown__') {
    return t(locale.value, 'desktop.allSources')
  }
  if (id === OFFICIAL_OPENAI_OAUTH_SOURCE_ID) return sourceLabel(locale.value, id)
  if (id === OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID) return sourceLabel(locale.value, id)
  const source = monitor.settings.sourceAware.sources.find(s => s.id === id)
  if (!source) {
    return t(locale.value, 'desktop.allSources')
  }
  return source.displayName || source.baseUrl || t(locale.value, 'desktop.allSources')
})

const toolFilterLabel = computed(() => {
  const tool = monitor.settings.clientTools.activeToolFilter
  if (!tool) {
    return t(locale.value, 'desktop.allTools')
  }
  return formatToolDisplayName(tool, locale.value, monitor.settings.clientTools.profiles)
})

const filterSummary = computed(() => `${sourceFilterLabel.value} · ${toolFilterLabel.value}`)

async function handleOpenShareWindow() {
  try {
    await openShareWindow()
  } catch (error) {
    console.error('[DesktopPageHeader] 打开分享窗口失败:', error)
  }
}
</script>

<template>
  <header
    class="flex h-16 shrink-0 items-center justify-between gap-4 border-b border-[var(--theme-border-subtle)] px-6"
    style="background: color-mix(in srgb, var(--theme-bg-chrome) 90%, var(--theme-bg-app) 10%)"
  >
    <!-- 左侧：页面标题 + 可选副信息 -->
    <div class="flex min-w-0 flex-col justify-center">
      <h1 class="truncate text-[20px] font-semibold leading-7 tracking-normal text-[var(--theme-text-primary)]">
        {{ pageTitle }}
      </h1>
      <p v-if="showFilterSummary" class="truncate text-xs text-[var(--theme-text-tertiary)]">
        {{ filterSummary }}
      </p>
    </div>

    <!-- 右侧：筛选范围与窗口操作分组，时间范围仍位于各页面内部工具栏。 -->
    <div class="flex shrink-0 items-center gap-2">
      <div class="flex items-center gap-0.5 rounded-full border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)]/55 p-0.5">
        <SourceSelector />
        <ToolSelector />
      </div>

      <div class="flex items-center gap-0.5 rounded-full border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)]/55 p-0.5">
        <button
          type="button"
          class="theme-icon-button flex h-7 w-7 items-center justify-center rounded-full transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-primary)]"
          :title="t(locale, 'desktop.refresh')"
          :aria-label="t(locale, 'desktop.refresh')"
          @click="monitor.refreshUsageAndSessionViews()"
        >
          <RefreshCw :size="15" :class="{ 'animate-spin': monitor.loading }" aria-hidden="true" />
        </button>

        <ThemeSelector />

        <button
          type="button"
          class="theme-icon-button flex h-7 w-7 items-center justify-center rounded-full transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-primary)]"
          :title="t(locale, 'desktop.shareWindow')"
          :aria-label="t(locale, 'desktop.shareWindow')"
          @click="handleOpenShareWindow"
        >
          <Share2 :size="15" aria-hidden="true" />
        </button>
      </div>
    </div>
  </header>
</template>
