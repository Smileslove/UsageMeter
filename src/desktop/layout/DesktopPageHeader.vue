<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Ellipsis, RefreshCw } from 'lucide-vue-next'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import { formatToolDisplayName } from '../../utils/toolDisplay'
import type { DesktopPage } from '../../types'
import SourceSelector from '../../components/SourceSelector.vue'
import ToolSelector from '../../components/ToolSelector.vue'

/** 全局筛选摘要仅作用于概览/分析/会话（设计文档 4.5）。 */
const PAGES_WITH_FILTER_SUMMARY: DesktopPage[] = ['overview', 'analytics', 'sessions']

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

// 更多菜单（占位）
const moreOpen = ref(false)
const moreMenuRef = ref<HTMLElement | null>(null)

function handleClickOutside(event: MouseEvent) {
  if (moreMenuRef.value && !moreMenuRef.value.contains(event.target as Node)) {
    moreOpen.value = false
  }
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside)
})

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside)
})
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

    <!-- 右侧：来源 → 工具 → 刷新 → 更多菜单（时间范围选择位于各页面内部工具栏） -->
    <div class="flex shrink-0 items-center gap-2">
      <SourceSelector />
      <ToolSelector />

      <!-- 刷新 -->
      <button
        type="button"
        class="flex h-8 w-8 items-center justify-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
        :title="t(locale, 'desktop.refresh')"
        :aria-label="t(locale, 'desktop.refresh')"
        @click="monitor.refreshUsageAndSessionViews()"
      >
        <RefreshCw :size="16" :class="{ 'animate-spin': monitor.loading }" aria-hidden="true" />
      </button>

      <!-- 更多菜单（占位） -->
      <div ref="moreMenuRef" class="relative">
        <button
          type="button"
          class="flex h-8 w-8 items-center justify-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
          :title="t(locale, 'desktop.moreMenu')"
          :aria-label="t(locale, 'desktop.moreMenu')"
          :aria-expanded="moreOpen"
          @click="moreOpen = !moreOpen"
        >
          <Ellipsis :size="16" aria-hidden="true" />
        </button>
        <Transition
          enter-active-class="transition ease-out duration-100"
          enter-from-class="transform opacity-0 scale-95"
          enter-to-class="transform opacity-100 scale-100"
          leave-active-class="transition ease-in duration-75"
          leave-from-class="transform opacity-100 scale-100"
          leave-to-class="transform opacity-0 scale-95"
        >
          <div
            v-if="moreOpen"
            class="absolute right-0 top-full z-50 mt-1 w-48 rounded-lg border border-[var(--theme-border-default)] py-1 shadow-[var(--theme-shadow-inline)]"
            style="background: var(--theme-bg-overlay)"
          >
            <div class="px-3 py-2 text-xs text-[var(--theme-text-tertiary)]">
              {{ t(locale, 'desktop.moreMenuPlaceholder') }}
            </div>
          </div>
        </Transition>
      </div>
    </div>
  </header>
</template>
