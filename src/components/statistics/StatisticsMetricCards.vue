<script setup lang="ts">
import { CircleDollarSign, MessageSquare, Sigma } from 'lucide-vue-next'
import { t } from '../../i18n'
import { formatCost, formatRequestCount, formatTokenPair, formatTokenValue } from '../../utils/format'
import type { AppLocale, StatisticsTotals } from '../../types'

const props = defineProps<{
  locale: AppLocale
  totals: StatisticsTotals | null
}>()

function value(key: 'requests' | 'tokens' | 'input' | 'output' | 'cost'): string {
  const totals = props.totals
  if (!totals) return key === 'cost' ? formatCost(0) : '0'
  if (key === 'requests') return formatRequestCount(totals.requestCount)
  if (key === 'tokens') return formatTokenValue(totals.totalTokens)
  // 输入显示总输入（包含缓存读取）
  if (key === 'input') return formatTokenPair(totals.inputTokens + (totals.cacheReadTokens ?? 0), totals.outputTokens).input
  if (key === 'output') return formatTokenPair(totals.inputTokens + (totals.cacheReadTokens ?? 0), totals.outputTokens).output
  if (key === 'cost') return formatCost(totals.cost)
  return '0'
}

function countValue(value: number | null | undefined): string {
  return formatRequestCount(value ?? 0)
}

function splitCost(key: 'input' | 'output'): string {
  const totals = props.totals
  if (!totals || totals.cost <= 0) return formatCost(0)
  const total = totals.inputTokens + totals.outputTokens
  if (total <= 0) return formatCost(0)
  const tokens = key === 'input' ? totals.inputTokens : totals.outputTokens
  return formatCost(totals.cost * (tokens / total))
}
</script>

<template>
  <section class="grid grid-cols-3 gap-1.5">
    <div class="min-w-0 rounded-xl border border-gray-100 bg-gray-50 px-1.5 py-2 dark:border-neutral-700 dark:bg-neutral-800/70">
      <div class="flex min-h-[78px] items-center gap-1.5">
        <div class="flex h-full w-5 shrink-0 flex-col items-center justify-center gap-1 border-r border-gray-200 pr-1 text-gray-500 dark:border-neutral-700 dark:text-gray-300">
          <MessageSquare class="h-3.5 w-3.5 text-emerald-500 dark:text-emerald-300" />
          <p class="writing-vertical text-[10px] font-semibold leading-none">{{ t(locale, 'statistics.requestStats') }}</p>
        </div>
        <div class="min-w-0 flex-1">
          <p class="text-[9px] font-semibold text-gray-500 dark:text-gray-400">{{ t(locale, 'statistics.requests') }}</p>
          <p class="truncate font-mono text-base font-bold text-gray-900 dark:text-gray-100">{{ value('requests') }}</p>
          <div class="mt-1 space-y-0.5 text-[9px] font-medium text-gray-500 dark:text-gray-400">
            <p class="truncate">{{ t(locale, 'statistics.successRequests') }} {{ countValue(totals?.successRequests) }}</p>
            <p class="truncate">{{ t(locale, 'statistics.errorRequests') }} {{ countValue(totals?.errorRequests) }}</p>
          </div>
        </div>
      </div>
    </div>

    <div class="min-w-0 rounded-xl border border-gray-100 bg-gray-50 px-1.5 py-2 dark:border-neutral-700 dark:bg-neutral-800/70">
      <div class="flex min-h-[78px] items-center gap-1.5">
        <div class="flex h-full w-5 shrink-0 flex-col items-center justify-center gap-1 border-r border-gray-200 pr-1 text-gray-500 dark:border-neutral-700 dark:text-gray-300">
          <Sigma class="h-3.5 w-3.5 text-blue-500 dark:text-blue-300" />
          <p class="writing-vertical text-[10px] font-semibold leading-none">{{ t(locale, 'statistics.consumptionStats') }}</p>
        </div>
        <div class="min-w-0 flex-1">
          <p class="text-[9px] font-semibold text-gray-500 dark:text-gray-400">{{ t(locale, 'statistics.consumedTokens') }}</p>
          <p class="truncate font-mono text-base font-bold text-gray-900 dark:text-gray-100">{{ value('tokens') }}</p>
          <div class="mt-1 space-y-0.5 text-[9px] font-medium text-gray-500 dark:text-gray-400">
            <p class="truncate">{{ t(locale, 'statistics.input') }} {{ value('input') }}</p>
            <p class="truncate">{{ t(locale, 'statistics.output') }} {{ value('output') }}</p>
          </div>
        </div>
      </div>
    </div>

    <div class="min-w-0 rounded-xl border border-gray-100 bg-gray-50 px-1.5 py-2 dark:border-neutral-700 dark:bg-neutral-800/70">
      <div class="flex min-h-[78px] items-center gap-1.5">
        <div class="flex h-full w-5 shrink-0 flex-col items-center justify-center gap-1 border-r border-gray-200 pr-1 text-gray-500 dark:border-neutral-700 dark:text-gray-300">
          <CircleDollarSign class="h-3.5 w-3.5 text-amber-500 dark:text-amber-300" />
          <p class="writing-vertical text-[10px] font-semibold leading-none">{{ t(locale, 'statistics.consumedCost') }}</p>
        </div>
        <div class="min-w-0 flex-1">
          <p class="text-[9px] font-semibold text-gray-500 dark:text-gray-400">{{ t(locale, 'statistics.cost') }}</p>
          <p class="truncate font-mono text-[15px] font-bold leading-5 text-gray-900 dark:text-gray-100">{{ value('cost') }}</p>
          <div class="mt-1 space-y-0.5 text-[9px] font-medium text-gray-500 dark:text-gray-400">
            <p class="truncate">{{ t(locale, 'statistics.inputCost') }} {{ splitCost('input') }}</p>
            <p class="truncate">{{ t(locale, 'statistics.outputCost') }} {{ splitCost('output') }}</p>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.writing-vertical {
  writing-mode: vertical-rl;
  text-orientation: mixed;
}
</style>
