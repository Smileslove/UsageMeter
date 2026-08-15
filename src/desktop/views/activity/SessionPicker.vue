<script setup lang="ts">
/**
 * 会话选择器（活动页自由模式顶部；数据来自 monitor store.sessions）。
 * 搜索/下拉/分页加载自包含；选中会话通过 emit('select') 交由父组件加载。
 */
import { computed, onMounted, ref } from 'vue'
import { ChevronDown, Loader2, Search } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t } from '../../../i18n'
import type { SessionStats } from '../../../types'

const props = defineProps<{
  /** 当前选中会话 id（用于高亮）。 */
  activeKey: string | null
  /** 挂载时若 store.sessions 为空则预加载第一页（自由模式无深链会话时）。 */
  autoLoad?: boolean
}>()

const emit = defineEmits<{
  select: [session: SessionStats]
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

const sessionPickerOpen = ref(false)
const sessionSearch = ref('')
const sessionsLoading = ref(false)
const sessionsAllLoaded = ref(false)
let sessionsOffset = 0

const sessionTitle = (session: SessionStats) =>
  session.topic?.trim()
  || session.sessionName?.trim()
  || session.lastPrompt?.trim()
  || session.projectName?.trim()
  || t(locale.value, 'sessions.untitled')

const pickerSessions = computed(() => {
  const query = sessionSearch.value.trim().toLowerCase()
  if (!query) return store.sessions
  return store.sessions.filter(s =>
    sessionTitle(s).toLowerCase().includes(query)
    || (s.projectName ?? '').toLowerCase().includes(query)
    || (s.cwd ?? '').toLowerCase().includes(query)
  )
})

const loadMoreSessions = async () => {
  if (sessionsLoading.value || sessionsAllLoaded.value) return
  sessionsLoading.value = true
  try {
    const count = await store.fetchSessions(100, sessionsOffset, true)
    sessionsOffset += count
    if (count < 100) sessionsAllLoaded.value = true
  } finally {
    sessionsLoading.value = false
  }
}

const ensureSessions = async () => {
  if (store.sessions.length > 0) return
  const count = await store.fetchSessions(100, 0, false)
  sessionsOffset = count
  if (count < 100) sessionsAllLoaded.value = true
}

const selectSession = (session: SessionStats) => {
  sessionPickerOpen.value = false
  emit('select', session)
}

const currentSession = computed(() =>
  store.sessions.find(s => s.sessionId === props.activeKey) ?? null
)

onMounted(() => {
  if (props.autoLoad !== false) void ensureSessions()
})
</script>

<template>
  <div class="relative min-w-0 flex-1">
    <button
      type="button"
      class="theme-surface flex h-9 w-full max-w-md items-center gap-2 rounded-lg border px-3 text-left text-[12px] font-medium text-[var(--theme-text-primary)]"
      :aria-label="t(locale, 'desktop.activity.selectSessionLabel')"
      :title="t(locale, 'desktop.activity.selectSessionLabel')"
      :aria-expanded="sessionPickerOpen"
      @click="sessionPickerOpen = !sessionPickerOpen"
    >
      <Search class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <span class="min-w-0 flex-1 truncate">
        <template v-if="currentSession">{{ sessionTitle(currentSession) }}</template>
        <template v-else class="text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.selectSessionLabel') }}</template>
      </span>
      <ChevronDown class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
    </button>

    <!-- 下拉：搜索 + 会话列表 -->
    <div
      v-if="sessionPickerOpen"
      class="theme-surface-elevated absolute left-0 top-10 z-50 w-full max-w-md rounded-xl border p-1.5 shadow-lg"
    >
      <div class="flex items-center gap-2 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-1.5">
        <Search class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <input
          v-model="sessionSearch"
          type="text"
          class="min-w-0 flex-1 bg-transparent text-[12px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]"
          :placeholder="t(locale, 'desktop.activity.sessionPickerPlaceholder')"
        />
      </div>
      <div class="mt-1 max-h-64 overflow-y-auto">
        <button
          v-for="session in pickerSessions"
          :key="session.sessionId"
          type="button"
          class="flex w-full flex-col gap-0.5 rounded-lg px-2.5 py-2 text-left transition-colors hover:bg-[var(--theme-bg-hover)]"
          :class="activeKey === session.sessionId ? 'bg-[var(--theme-accent-soft)]' : ''"
          @click="selectSession(session)"
        >
          <span class="truncate text-[12px] font-medium text-[var(--theme-text-primary)]">{{ sessionTitle(session) }}</span>
          <span class="truncate font-mono text-[10px] text-[var(--theme-text-quaternary)]">{{ session.projectName || session.sessionId }}</span>
        </button>
        <div v-if="pickerSessions.length === 0" class="px-2.5 py-6 text-center text-[11px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.activity.sessionPickerEmpty') }}
        </div>
        <button
          v-if="!sessionsAllLoaded"
          type="button"
          class="mt-1 flex w-full items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-semibold text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]"
          :disabled="sessionsLoading"
          @click="loadMoreSessions"
        >
          <Loader2 v-if="sessionsLoading" class="h-3 w-3 animate-spin" aria-hidden="true" />
          {{ t(locale, 'desktop.activity.sessionsLoadMore') }}
        </button>
      </div>
    </div>
  </div>
</template>
