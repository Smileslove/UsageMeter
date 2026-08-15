<script setup lang="ts">
/**
 * 左栏：事件类型过滤 + 代理树（设计 9.3 / 9.6：fullTree 才画父子树，
 * rootGrouped/flagOnly 显示分组列表不画伪树）+ 工具汇总。
 * 全部展示逻辑自包含；过滤状态通过 v-model 风格 emits 与父组件同步。
 */
import { computed } from 'vue'
import { Bot, ChevronRight, Wrench, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t } from '../../../i18n'
import {
  KIND_GROUPS, agentStatusKey, agentStatusMeta, shortAgentId, formatDuration
} from './eventMeta'
import type { AgentNodeDto, SessionEventKind, ToolSummaryRow } from '../../../types'

const props = defineProps<{
  agents: AgentNodeDto[]
  relationLevel: string
  toolSummary: ToolSummaryRow[]
  selectedKinds: Set<SessionEventKind>
  activeAgentKey: string | null
  /** 宽屏三栏模式（窄屏 overlay 显示关闭按钮）。 */
  wide: boolean
}>()

const emit = defineEmits<{
  'update:selectedKinds': [kinds: Set<SessionEventKind>]
  'update:activeAgentKey': [key: string | null]
  close: []
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

interface AgentRow { node: AgentNodeDto; depth: number }

const agentRows = computed<AgentRow[]>(() => {
  const list = props.agents
  if (list.length === 0) return []
  if (props.relationLevel === 'fullTree') {
    const byParent = new Map<string | null, AgentNodeDto[]>()
    for (const node of list) {
      const key = node.parentAgentKey ?? null
      const bucket = byParent.get(key) ?? []
      bucket.push(node)
      byParent.set(key, bucket)
    }
    const rows: AgentRow[] = []
    const walk = (parent: string | null, depth: number) => {
      const children = byParent.get(parent) ?? []
      for (const node of children) {
        rows.push({ node, depth })
        walk(node.agentKey, depth + 1)
      }
    }
    walk(null, 0)
    return rows
  }
  // rootGrouped / flagOnly / none：分组列表，不画伪树
  return list.map(node => ({ node, depth: 0 }))
})

const unlinkedAgents = computed(() =>
  props.relationLevel !== 'fullTree' ? props.agents : []
)

const toggleKindGroup = (kinds: SessionEventKind[]) => {
  const next = new Set(props.selectedKinds)
  const allSelected = kinds.every(k => next.has(k))
  for (const kind of kinds) {
    if (allSelected) next.delete(kind)
    else next.add(kind)
  }
  emit('update:selectedKinds', next)
}
const groupChecked = (kinds: SessionEventKind[]) => kinds.every(k => props.selectedKinds.has(k))

const toggleAgent = (key: string | null) => {
  emit('update:activeAgentKey', props.activeAgentKey === key ? null : key)
}
</script>

<template>
  <!-- 左栏：章节与代理树（240px；窄屏 overlay） -->
  <aside
    class="theme-surface flex w-60 shrink-0 flex-col overflow-hidden rounded-xl border"
    :class="wide ? '' : 'absolute inset-y-0 left-0 z-40 w-72 shadow-2xl'"
    :aria-label="t(locale, 'desktop.activity.filtersLabel')"
  >
    <div class="flex items-center justify-between border-b border-[var(--theme-border-subtle)] px-3 py-2">
      <span class="text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.filtersLabel') }}</span>
      <button
        v-if="!wide"
        type="button"
        class="rounded p-1 text-[var(--theme-text-quaternary)] hover:bg-[var(--theme-bg-hover)]"
        :aria-label="t(locale, 'common.close')"
        :title="t(locale, 'common.close')"
        @click="emit('close')"
      >
        <X class="h-3.5 w-3.5" aria-hidden="true" />
      </button>
    </div>
    <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-3">
      <!-- 事件类型过滤（复选） -->
      <div class="space-y-1">
        <span class="px-0.5 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.filtersLabel') }}</span>
        <label
          v-for="group in KIND_GROUPS"
          :key="group.id"
          class="flex cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 transition-colors hover:bg-[var(--theme-bg-hover)]"
        >
          <input
            type="checkbox"
            class="h-3.5 w-3.5 shrink-0 accent-[var(--theme-accent-primary)]"
            :checked="groupChecked(group.kinds)"
            :aria-label="t(locale, group.labelKey)"
            @change="toggleKindGroup(group.kinds)"
          />
          <span class="text-[11.5px] font-medium text-[var(--theme-text-secondary)]">{{ t(locale, group.labelKey) }}</span>
        </label>
      </div>

      <!-- 代理（设计 9.3 / 9.6） -->
      <div class="space-y-1">
        <div class="flex items-center justify-between px-0.5">
          <span class="text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.agentsLabel') }}</span>
          <button
            v-if="activeAgentKey"
            type="button"
            class="rounded px-1 text-[10px] font-semibold text-[var(--theme-accent-primary)] hover:underline"
            @click="emit('update:activeAgentKey', null)"
          >
            {{ t(locale, 'desktop.activity.clearFilter') }}
          </button>
        </div>
        <p v-if="relationLevel !== 'fullTree'" class="px-0.5 text-[10px] leading-relaxed text-[var(--theme-text-quaternary)]">
          {{ t(locale, 'desktop.activity.agentsRelationHint') }}
        </p>
        <div v-if="agentRows.length === 0 && unlinkedAgents.length === 0" class="px-0.5 py-2 text-[11px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.activity.agentsEmpty') }}
        </div>
        <template v-else>
          <!-- fullTree：父子树（缩进层级） -->
          <button
            v-for="row in agentRows"
            :key="row.node.agentKey"
            type="button"
            class="flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1 text-left transition-colors hover:bg-[var(--theme-bg-hover)]"
            :style="{ paddingLeft: `${8 + row.depth * 14}px` }"
            :class="activeAgentKey === row.node.agentKey ? 'bg-[var(--theme-accent-soft)]' : ''"
            @click="toggleAgent(row.node.agentKey)"
          >
            <ChevronRight
              v-if="row.depth === 0"
              class="h-3 w-3 shrink-0 text-[var(--theme-text-quaternary)]"
              aria-hidden="true"
            />
            <Bot class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <span class="min-w-0 flex-1 truncate text-[11px] font-medium text-[var(--theme-text-secondary)]">
              {{ row.node.displayKind || shortAgentId(row.node.agentKey) }}
            </span>
            <span class="shrink-0 rounded px-1 py-px text-[9px] font-bold leading-none" :class="agentStatusMeta(row.node.status)">
              {{ t(locale, agentStatusKey(row.node.status)) }}
            </span>
          </button>
          <!-- 非 fullTree：分组列表（不画伪树） -->
          <button
            v-for="node in unlinkedAgents"
            :key="node.agentKey"
            type="button"
            class="flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1 text-left transition-colors hover:bg-[var(--theme-bg-hover)]"
            :class="activeAgentKey === node.agentKey ? 'bg-[var(--theme-accent-soft)]' : ''"
            @click="toggleAgent(node.agentKey)"
          >
            <Bot class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
            <span class="min-w-0 flex-1 truncate text-[11px] font-medium text-[var(--theme-text-secondary)]">
              {{ node.displayKind || shortAgentId(node.agentKey) }}
            </span>
            <span class="shrink-0 rounded px-1 py-px text-[9px] font-bold leading-none" :class="agentStatusMeta(node.status)">
              {{ t(locale, agentStatusKey(node.status)) }}
            </span>
          </button>
        </template>
      </div>

      <!-- 工具汇总 -->
      <div class="space-y-1">
        <span class="px-0.5 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.activity.toolsLabel') }}</span>
        <div v-if="toolSummary.length === 0" class="px-0.5 py-2 text-[11px] text-[var(--theme-text-tertiary)]">
          {{ t(locale, 'desktop.activity.toolsEmpty') }}
        </div>
        <div v-for="row in toolSummary" :key="row.toolName" class="flex items-center gap-2 rounded-lg px-2 py-1.5">
          <Wrench class="h-3.5 w-3.5 shrink-0 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
          <div class="min-w-0 flex-1">
            <div class="truncate text-[11px] font-medium text-[var(--theme-text-secondary)]" :title="row.toolName">{{ row.toolName }}</div>
            <div class="text-[9.5px] text-[var(--theme-text-quaternary)]">
              {{ t(locale, 'desktop.activity.invocationsCount', { count: row.invocationCount }) }}
              <span v-if="row.errorCount > 0" class="text-rose-500"> · {{ t(locale, 'desktop.activity.failuresCount', { count: row.errorCount }) }}</span>
              <span class="font-mono"> · {{ formatDuration(row.avgDurationMs) }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </aside>
</template>
