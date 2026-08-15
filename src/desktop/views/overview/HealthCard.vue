<script lang="ts">
/**
 * 健康检查条目的数据契约：由父页面根据 store 状态组装后经 props 传入。
 * 普通 script 块用于导出类型（script setup 无法导出）。
 */
import type { Component } from 'vue'

export interface HealthItem {
  key: string
  icon: Component
  tone: 'ok' | 'warning' | 'danger'
  text: string
  actionKey?: string
  action?: () => void
}
</script>

<script setup lang="ts">
/**
 * 概览页"数据覆盖与异常"卡片（设计 6.8）。
 * 条目（含 action 闭包）由父页面组装传入；组件仅负责渲染。
 */
import type { PropType } from 'vue'
import { CircleHelp } from 'lucide-vue-next'
import { t } from '../../../i18n'

defineProps({
  items: { type: Array as PropType<HealthItem[]>, required: true },
  locale: { type: String, required: true }
})
</script>

<template>
  <div class="rounded-lg border border-[var(--theme-border-default)] p-4" style="background: var(--theme-surface-gradient)">
    <h3 class="mb-2 flex items-center gap-1.5 text-[13px] font-semibold text-[var(--theme-text-secondary)]">
      <CircleHelp :size="14" class="shrink-0" aria-hidden="true" />
      {{ t(locale, 'desktop.overview.healthTitle') }}
    </h3>
    <ul class="flex flex-col gap-1.5">
      <li
        v-for="item in items"
        :key="item.key"
        class="flex items-start justify-between gap-2 rounded-lg border border-[var(--theme-border-subtle)] px-3 py-2"
      >
        <span class="flex min-w-0 items-start gap-2 text-xs leading-5">
          <component
            :is="item.icon"
            :size="14"
            class="mt-0.5 shrink-0"
            :class="
              item.tone === 'ok'
                ? 'text-emerald-500 dark:text-emerald-400'
                : item.tone === 'danger'
                  ? 'text-red-500 dark:text-red-400'
                  : 'text-amber-500 dark:text-amber-400'
            "
            aria-hidden="true"
          />
          <span class="min-w-0 text-[var(--theme-text-secondary)]">{{ item.text }}</span>
        </span>
        <button
          v-if="item.actionKey && item.action"
          type="button"
          class="shrink-0 text-xs font-medium text-[var(--theme-accent-primary)] transition-colors duration-150 hover:underline"
          @click="item.action()"
        >
          {{ t(locale, item.actionKey) }}
        </button>
      </li>
    </ul>
  </div>
</template>
