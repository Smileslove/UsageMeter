<script setup lang="ts">
/**
 * 跨会话全文搜索面板（9.8：入口按钮在会话选择器旁，仅 fulltext 档返回命中；
 * 结果点击通过 emit('jump') 交由父组件跳转该会话活动页）。
 * 搜索状态、分页加载、错误重试全部自包含。
 */
import { computed, ref } from 'vue'
import { ChevronRight, Globe, Loader2, RefreshCw, Search, Wrench } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t, backendErrorLabel } from '../../../i18n'
import { searchActivityGlobal } from '../../../api/activityApi'
import { hitKind, KIND_META, formatRelTime } from './eventMeta'
import type { GlobalSearchHit } from '../../../types'

const emit = defineEmits<{
  /** 点击命中：跳转到该会话的活动页。 */
  jump: [hit: GlobalSearchHit]
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

const globalSearchOpen = ref(false)
const globalQuery = ref('')
const globalResults = ref<GlobalSearchHit[]>([])
const globalTotal = ref(0)
const globalHasMore = ref(false)
const globalLoading = ref(false)
const globalError = ref('')
let globalOffset = 0
let globalGeneration = 0

const runGlobalSearch = async (reset: boolean) => {
  const query = globalQuery.value.trim()
  if (!query) return
  const generation = ++globalGeneration
  if (reset) {
    globalOffset = 0
    globalResults.value = []
    globalTotal.value = 0
    globalHasMore.value = false
    globalError.value = ''
  }
  globalLoading.value = true
  try {
    const page = await searchActivityGlobal(store.settings, query, globalOffset, 50)
    if (generation !== globalGeneration) return
    globalResults.value = reset ? page.items : [...globalResults.value, ...page.items]
    globalTotal.value = page.total
    globalHasMore.value = page.hasMore
    globalOffset += page.items.length
  } catch (e) {
    if (generation !== globalGeneration) return
    globalError.value = e instanceof Error ? e.message : String(e)
  } finally {
    if (generation === globalGeneration) globalLoading.value = false
  }
}

const triggerGlobalSearch = () => {
  if (globalQuery.value.trim()) void runGlobalSearch(true)
}

/** 点击命中：关闭面板并跳转（父组件 nav.openActivity 消费）。 */
const jumpToSession = (hit: GlobalSearchHit) => {
  globalSearchOpen.value = false
  emit('jump', hit)
}
</script>

<template>
  <div class="relative shrink-0">
    <button
      type="button"
      class="theme-button-secondary inline-flex h-9 items-center gap-1.5 rounded-lg px-3 text-[12px] font-semibold"
      :aria-label="t(locale, 'desktop.activity.globalSearch')"
      :title="t(locale, 'desktop.activity.globalSearch')"
      :aria-expanded="globalSearchOpen"
      @click="globalSearchOpen = !globalSearchOpen"
    >
      <Globe class="h-3.5 w-3.5" aria-hidden="true" />
      <span class="hidden md:inline">{{ t(locale, 'desktop.activity.globalSearch') }}</span>
    </button>

    <div
      v-if="globalSearchOpen"
      class="theme-surface-elevated absolute right-0 top-11 z-50 w-[min(560px,90vw)] rounded-xl border p-2 shadow-lg"
      :aria-label="t(locale, 'desktop.activity.globalSearch')"
    >
      <div class="flex items-center gap-2 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-1.5">
        <Search class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
        <input
          v-model="globalQuery"
          type="text"
          class="min-w-0 flex-1 bg-transparent text-[12px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]"
          :placeholder="t(locale, 'desktop.activity.globalSearchPlaceholder')"
          @keydown.enter="triggerGlobalSearch"
        />
        <button
          type="button"
          class="rounded p-1 text-[var(--theme-text-tertiary)] transition-colors hover:bg-[var(--theme-bg-hover)]"
          :aria-label="t(locale, 'desktop.activity.globalSearch')"
          :disabled="globalLoading"
          @click="triggerGlobalSearch"
        >
          <Loader2 v-if="globalLoading" class="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
          <Search v-else class="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      </div>
      <div class="mt-1.5 max-h-72 overflow-y-auto">
        <div v-if="globalLoading && globalResults.length === 0" class="py-6 text-center text-[11px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.activity.timelineLoading') }}
        </div>
        <div v-else-if="globalError" class="px-2.5 py-4 text-center">
          <p class="break-all text-[10.5px] text-rose-500">{{ backendErrorLabel(locale, globalError) }}</p>
          <button
            type="button"
            class="theme-button-secondary mt-2 inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[11px] font-semibold"
            @click="triggerGlobalSearch"
          >
            <RefreshCw class="h-3 w-3" aria-hidden="true" />
            {{ t(locale, 'desktop.activity.retry') }}
          </button>
        </div>
        <div v-else-if="globalResults.length === 0 && globalQuery.trim()" class="px-2.5 py-6 text-center text-[11px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.activity.globalSearchEmpty') }}
        </div>
        <template v-else>
          <div v-if="globalTotal > 0" class="px-2 pt-1 text-[10px] font-semibold text-[var(--theme-accent-primary)]">
            {{ t(locale, 'desktop.activity.searchHitCount', { count: globalTotal }) }}
          </div>
          <button
            v-for="hit in globalResults"
            :key="hit.eventKey"
            type="button"
            class="flex w-full flex-col gap-1 rounded-lg px-2.5 py-2 text-left transition-colors hover:bg-[var(--theme-bg-hover)]"
            @click="jumpToSession(hit)"
          >
            <span class="flex flex-wrap items-center gap-x-2 gap-y-0.5">
              <span class="min-w-0 flex-1 truncate text-[11px] font-semibold text-[var(--theme-text-primary)]">{{ hit.sessionTitle || t(locale, 'sessions.untitled') }}</span>
              <span class="shrink-0 rounded px-1.5 py-px text-[9px] font-bold leading-none" :class="KIND_META[hitKind(hit.kind)].cls">
                {{ t(locale, KIND_META[hitKind(hit.kind)].labelKey) }}
              </span>
              <span v-if="hit.toolName" class="shrink-0 inline-flex items-center gap-1 rounded bg-[var(--theme-border-subtle)] px-1.5 py-px font-mono text-[9px] text-[var(--theme-text-tertiary)]">
                <Wrench class="h-2.5 w-2.5" aria-hidden="true" />
                {{ hit.toolName }}
              </span>
              <span v-if="hit.timestampMs != null" class="ml-auto shrink-0 text-[9.5px] text-[var(--theme-text-quaternary)]">{{ formatRelTime(hit.timestampMs, locale) }}</span>
            </span>
            <span v-if="hit.summary" class="line-clamp-2 break-words text-[11px] leading-relaxed text-[var(--theme-text-secondary)]">{{ hit.summary }}</span>
            <span class="inline-flex items-center gap-0.5 text-[9.5px] font-semibold text-[var(--theme-accent-primary)]">
              {{ t(locale, 'desktop.activity.globalSearchJump') }}
              <ChevronRight class="h-3 w-3" aria-hidden="true" />
            </span>
          </button>
          <button
            v-if="globalHasMore"
            type="button"
            class="mt-1 flex w-full items-center justify-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[10.5px] font-semibold text-[var(--theme-text-tertiary)] hover:bg-[var(--theme-bg-hover)]"
            :disabled="globalLoading"
            @click="runGlobalSearch(false)"
          >
            <Loader2 v-if="globalLoading" class="h-3 w-3 animate-spin" aria-hidden="true" />
            {{ t(locale, 'desktop.activity.loadMore') }}
          </button>
        </template>
      </div>
    </div>
  </div>
</template>
