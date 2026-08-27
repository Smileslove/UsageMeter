import { computed } from 'vue'
import type { useMonitorStore } from '../../stores/monitor'

/**
 * 代理覆盖性能数据的可用性判断（设计 7.7）。
 * PerformanceView 与 BreakdownView 共享同一口径：capability 标记 + 实际性能样本 > 0。
 */
export function usePerformanceAvailability(store: ReturnType<typeof useMonitorStore>) {
  const performance = computed(() => store.statisticsSummary?.performance ?? null)
  const hasPerformance = computed(() =>
    !!store.statisticsSummary?.capability.hasPerformance &&
    !!performance.value &&
    performance.value.requestCount > 0,
  )
  return { performance, hasPerformance }
}
