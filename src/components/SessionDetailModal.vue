<script setup lang="ts">
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { computed } from 'vue'
import type { SessionStats } from '../types'
import { formatCost as formatCostUtil } from '../utils/format'

const props = defineProps<{
  visible: boolean
  session: SessionStats | null
}>()

const emit = defineEmits<{
  close: []
}>()

const store = useMonitorStore()

// 格式化时间
const formatTime = (epoch: number) => {
  if (!epoch) return '-'
  return new Date(epoch * 1000).toLocaleString(store.settings.locale.replace('-', '_'), {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit'
  })
}

// 格式化耗时
const formatDuration = (ms: number) => {
  if (!ms) return '-'
  if (ms < 1000) return `${ms}ms`
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`
  const minutes = Math.floor(ms / 60000)
  const seconds = Math.round((ms % 60000) / 1000)
  return `${minutes}m ${seconds}s`
}

// 格式化 Token 数量（1000以下显示整数，以上保留2位小数）
const formatTokens = (tokens: number) => {
  if (!tokens) return '0'
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(2)}M`
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(2)}K`
  return Math.round(tokens).toString()
}

// 格式化费用（统一4位小数，支持多货币）
const formatCost = (cost: number | undefined) => {
  if (cost === undefined || cost === null) return '-'
  return formatCostUtil(cost, store.settings.currency, 4)
}

// 计算输入输出比例
const inputOutputRatio = computed(() => {
  if (!props.session) return { input: 50, output: 50 }
  const total = props.session.totalInputTokens + props.session.totalOutputTokens
  if (total === 0) return { input: 50, output: 50 }
  return {
    input: (props.session.totalInputTokens / total) * 100,
    output: (props.session.totalOutputTokens / total) * 100
  }
})
</script>

<template>
  <Teleport to="body">
    <div
      v-if="visible && session"
      class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50 backdrop-blur-sm"
      @click.self="emit('close')"
    >
      <div class="bg-white dark:bg-[#1C1C1E] rounded-2xl w-full max-w-md max-h-[80vh] overflow-hidden shadow-2xl">
        <!-- 头部 -->
        <div class="p-4 border-b border-gray-100 dark:border-neutral-800 flex justify-between items-start">
          <div class="flex flex-col gap-1 overflow-hidden pr-2 flex-1">
            <!-- 项目名标签 -->
            <div v-if="session.projectName" class="flex items-center gap-1">
              <span class="inline-flex items-center text-[10px] text-purple-600 dark:text-purple-400 bg-purple-50 dark:bg-purple-900/20 px-1.5 py-0.5 rounded">
                <svg class="w-2.5 h-2.5 mr-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
                </svg>
                {{ session.projectName }}
              </span>
            </div>
            <!-- 话题标题 -->
            <h3 class="text-base font-semibold text-gray-800 dark:text-gray-100 truncate">
              {{ session.topic || session.sessionName || session.sessionId.split('::').pop() }}
            </h3>
            <p class="text-[10px] text-gray-400 truncate">
              {{ session.models.join(', ') }}
            </p>
          </div>
          <button
            @click="emit('close')"
            class="p-1.5 hover:bg-gray-100 dark:hover:bg-neutral-800 rounded-lg transition-colors shrink-0"
          >
            <svg class="w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <!-- 内容 -->
        <div class="p-4 overflow-y-auto max-h-[calc(80vh-60px)] space-y-4">
          <!-- 元信息 -->
          <div v-if="session.cwd || session.lastPrompt" class="bg-gray-50 dark:bg-neutral-800/50 rounded-xl p-3">
            <div v-if="session.cwd" class="mb-2">
              <div class="text-[10px] text-gray-400 mb-0.5">{{ t(store.settings.locale, 'settings.cwd') || '工作目录' }}</div>
              <div class="text-xs text-gray-600 dark:text-gray-300 truncate">{{ session.cwd }}</div>
            </div>
            <div v-if="session.lastPrompt">
              <div class="text-[10px] text-gray-400 mb-0.5">{{ t(store.settings.locale, 'sessions.lastPrompt') || '最后提示' }}</div>
              <div class="text-xs text-gray-600 dark:text-gray-300 line-clamp-2">{{ session.lastPrompt }}</div>
            </div>
          </div>

          <!-- 概览统计 -->
          <div class="grid grid-cols-4 gap-3">
            <div class="bg-gray-50 dark:bg-neutral-800/50 rounded-xl p-2.5 text-center">
              <div class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'sessions.totalTokens') }}</div>
              <div class="text-sm font-mono font-semibold text-gray-800 dark:text-gray-100">
                {{ formatTokens(session.totalInputTokens + session.totalOutputTokens) }}
              </div>
            </div>
            <div class="bg-gray-50 dark:bg-neutral-800/50 rounded-xl p-2.5 text-center">
              <div class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'sessions.estimatedCost') }}</div>
              <div class="text-sm font-mono font-semibold text-green-600">
                {{ formatCost(session.estimatedCost) }}
              </div>
            </div>
            <div class="bg-gray-50 dark:bg-neutral-800/50 rounded-xl p-2.5 text-center">
              <div class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'sessions.avgRate') }}</div>
              <div class="text-sm font-mono font-semibold text-blue-600">
                {{ session.avgOutputTokensPerSecond.toFixed(1) }}
              </div>
            </div>
            <div class="bg-gray-50 dark:bg-neutral-800/50 rounded-xl p-2.5 text-center">
              <div class="text-[10px] text-gray-400">{{ t(store.settings.locale, 'sessions.duration') }}</div>
              <div class="text-sm font-mono font-semibold text-gray-800 dark:text-gray-100">
                {{ formatDuration(session.totalDurationMs) }}
              </div>
            </div>
          </div>

          <!-- 详细统计 -->
          <div class="grid grid-cols-2 gap-3">
            <div class="flex justify-between items-center py-1.5">
              <span class="text-xs text-gray-500">{{ t(store.settings.locale, 'sessions.requests') }}</span>
              <span class="text-xs font-mono text-gray-700 dark:text-gray-300">{{ session.totalRequests }}</span>
            </div>
            <div class="flex justify-between items-center py-1.5">
              <span class="text-xs text-gray-500">{{ t(store.settings.locale, 'sessions.ttft') }}</span>
              <span class="text-xs font-mono text-gray-700 dark:text-gray-300">{{ session.avgTtftMs?.toFixed(0) || '-' }}ms</span>
            </div>
            <div class="flex justify-between items-center py-1.5">
              <span class="text-xs text-gray-500">{{ t(store.settings.locale, 'common.success') }}</span>
              <span class="text-xs font-mono text-green-600">{{ session.successRequests || 0 }}</span>
            </div>
            <div class="flex justify-between items-center py-1.5">
              <span class="text-xs text-gray-500">{{ t(store.settings.locale, 'common.error') || '错误' }}</span>
              <span class="text-xs font-mono text-red-500">{{ session.errorRequests || 0 }}</span>
            </div>
          </div>

          <!-- Token 分布 -->
          <div>
            <div class="text-[10px] text-gray-400 mb-1.5">{{ t(store.settings.locale, 'sessions.inputOutput') }}</div>
            <div class="w-full flex h-2.5 bg-gray-200 rounded-full overflow-hidden dark:bg-neutral-800">
              <div
                class="bg-cyan-400 h-full transition-all"
                :style="{ width: `${inputOutputRatio.input}%` }"
              ></div>
              <div
                class="bg-fuchsia-400 h-full transition-all"
                :style="{ width: `${inputOutputRatio.output}%` }"
              ></div>
            </div>
            <div class="flex justify-between mt-1 text-[10px] text-gray-400">
              <span>{{ t(store.settings.locale, 'common.inputTokens') }}: {{ formatTokens(session.totalInputTokens) }}</span>
              <span>{{ t(store.settings.locale, 'common.outputTokens') }}: {{ formatTokens(session.totalOutputTokens) }}</span>
            </div>
          </div>

          <!-- 时间信息 -->
          <div class="text-[10px] text-gray-400 space-y-1">
            <div class="flex justify-between">
              <span>{{ t(store.settings.locale, 'sessions.startTime') || '开始时间' }}</span>
              <span>{{ formatTime(session.firstRequestTime) }}</span>
            </div>
            <div class="flex justify-between">
              <span>{{ t(store.settings.locale, 'sessions.endTime') || '结束时间' }}</span>
              <span>{{ formatTime(session.lastRequestTime) }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>
