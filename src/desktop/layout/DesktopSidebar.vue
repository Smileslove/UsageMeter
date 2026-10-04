<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { Activity, ChartSpline, ChevronsLeft, ChevronsRight, FolderKanban, LayoutDashboard, List, ListChecks, Network, Settings } from 'lucide-vue-next'
import type { Component } from 'vue'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import { formatRelativeTime } from '../../utils/format'
import type { DesktopPage } from '../../types'

interface NavItem {
  id: DesktopPage
  icon: Component
  key: string
}

const navItems: NavItem[] = [
  { id: 'overview', icon: LayoutDashboard, key: 'desktop.nav.overview' },
  { id: 'analytics', icon: ChartSpline, key: 'desktop.nav.analytics' },
  { id: 'requests', icon: ListChecks, key: 'desktop.nav.requests' },
  { id: 'sessions', icon: List, key: 'desktop.nav.sessions' },
  { id: 'projects', icon: FolderKanban, key: 'desktop.nav.projects' },
  { id: 'activity', icon: Activity, key: 'desktop.nav.activity' },
  { id: 'gateway', icon: Network, key: 'desktop.nav.gateway' },
  { id: 'settings', icon: Settings, key: 'desktop.nav.settings' }
]

const nav = useDesktopNavigationStore()
const monitor = useMonitorStore()

const locale = computed(() => monitor.settings.locale)
const collapsed = computed(() => nav.sidebarCollapsed)

/** 用户手动调整过折叠状态后，不再跟随窗口宽度自动折叠。 */
const userAdjusted = ref(false)

function applyDefaultCollapse() {
  if (!userAdjusted.value) {
    nav.sidebarCollapsed = window.innerWidth < 1040
  }
}

function handleResize() {
  applyDefaultCollapse()
}

function toggleCollapsed() {
  userAdjusted.value = true
  nav.sidebarCollapsed = !nav.sidebarCollapsed
}

const updatedLabel = ref('')
let updateTimer: ReturnType<typeof setInterval> | null = null

function refreshUpdatedLabel() {
  updatedLabel.value = formatRelativeTime(monitor.lastUpdatedEpoch, locale.value)
}

onMounted(() => {
  applyDefaultCollapse()
  window.addEventListener('resize', handleResize)
  refreshUpdatedLabel()
  updateTimer = setInterval(refreshUpdatedLabel, 60_000)
})

onUnmounted(() => {
  window.removeEventListener('resize', handleResize)
  if (updateTimer) {
    clearInterval(updateTimer)
    updateTimer = null
  }
})
</script>

<template>
  <aside
    class="relative flex h-full shrink-0 flex-col overflow-hidden border-r border-[var(--theme-border-default)] transition-[width] duration-200 ease-out"
    :class="collapsed ? 'w-16' : 'w-[216px]'"
    style="background: var(--theme-bg-sidebar)"
  >
    <!-- 品牌区（顶部为 macOS traffic lights 留出安全区） -->
    <div data-tauri-drag-region class="flex shrink-0 items-center px-3 pb-4 pt-8 [&_*]:pointer-events-none">
      <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg" style="background: var(--theme-accent-soft)">
        <span class="relative flex h-2.5 w-2.5 items-center justify-center">
          <span class="h-1.5 w-1.5 rounded-full bg-emerald-500"></span>
          <span class="absolute inset-0 rounded-full bg-emerald-400/25"></span>
        </span>
      </div>
      <span
        v-if="!collapsed"
        class="ml-2.5 truncate text-[15px] font-semibold tracking-tight text-[var(--theme-text-primary)]"
      >
        {{ t(locale, 'app.name') }}
      </span>
    </div>

    <!-- 主导航 -->
    <nav class="flex flex-col gap-0.5 px-2" :aria-label="t(locale, 'app.name')">
      <button
        v-for="item in navItems"
        :key="item.id"
        type="button"
        class="relative flex h-10 shrink-0 items-center gap-3 rounded-lg px-3 text-[13px] font-medium transition-colors duration-150"
        :class="
          nav.currentPage === item.id
            ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-primary)]'
            : 'text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]'
        "
        :aria-label="t(locale, item.key)"
        :aria-current="nav.currentPage === item.id ? 'page' : undefined"
        :title="t(locale, item.key)"
        @click="nav.navigate(item.id)"
      >
        <span
          v-if="nav.currentPage === item.id"
          class="absolute left-0 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-r-full"
          style="background: var(--theme-accent-primary)"
        ></span>
        <component :is="item.icon" :size="18" :stroke-width="1.75" class="shrink-0" aria-hidden="true" />
        <span v-if="!collapsed" class="truncate">{{ t(locale, item.key) }}</span>
      </button>
    </nav>

    <div class="flex-1"></div>

    <!-- 数据状态区：最后更新时间 -->
    <div v-if="!collapsed" class="shrink-0 px-2 pb-1">
      <div class="flex flex-col gap-0.5 rounded-lg px-3 py-2">
        <span class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.updatedAt') }}</span>
        <span class="text-xs font-medium tabular-nums text-[var(--theme-text-secondary)]">{{ updatedLabel }}</span>
      </div>
    </div>

    <!-- 折叠切换 -->
    <div class="shrink-0 border-t border-[var(--theme-border-subtle)] p-2">
      <button
        type="button"
        class="flex h-8 w-full items-center justify-center rounded-lg text-[var(--theme-text-tertiary)] transition-colors duration-150 hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)]"
        :aria-label="collapsed ? t(locale, 'desktop.expandSidebar') : t(locale, 'desktop.collapseSidebar')"
        :title="collapsed ? t(locale, 'desktop.expandSidebar') : t(locale, 'desktop.collapseSidebar')"
        @click="toggleCollapsed"
      >
        <ChevronsLeft v-if="!collapsed" :size="16" aria-hidden="true" />
        <ChevronsRight v-else :size="16" aria-hidden="true" />
      </button>
    </div>
  </aside>
</template>
