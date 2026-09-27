/**
 * 性能视图（设计 7.7）纯计算逻辑：
 * 分位统计（P50/P95）、慢请求计数、状态码分组、最慢模型/来源排行、
 * 统计卡数据派生（perf）、代理覆盖可用性（hasPerformance）、最慢请求子表。
 *
 * 输入：请求记录 ref + 共享 store + 会话展示工具；输出响应式统计结果。
 * 不包含图表配置（见 usePerformanceCharts）与数据拉取（由组件层负责）。
 */
import { computed } from 'vue'
import type { Ref } from 'vue'
import type { RequestRecord } from '../../types'
import type { useMonitorStore } from '../../stores/monitor'
import type { useSessionDisplay } from '../../composables/useSessionDisplay'
import { usePerformanceAvailability } from './usePerformanceAvailability'

/** 线性插值分位：空数组返回 null，单元素直接返回。 */
export function percentile(sorted: number[], p: number): number | null {
  if (sorted.length === 0) return null
  if (sorted.length === 1) return sorted[0]
  const idx = (sorted.length - 1) * p
  const lo = Math.floor(idx)
  const hi = Math.ceil(idx)
  return lo === hi ? sorted[lo] : sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo)
}

/** 状态码分组（2xx / 3xx / 4xx / 5xx）。 */
export interface StatusGroup {
  key: string
  color: string
  count: number
}

/** 排行行（最慢模型/来源 top 5）。 */
export interface RankRow {
  label: string
  avgMs: number
  count: number
}

export function usePerformanceStats(
  records: Ref<RequestRecord[]>,
  store: ReturnType<typeof useMonitorStore>,
  display: ReturnType<typeof useSessionDisplay>,
) {
  const { performance, hasPerformance } = usePerformanceAvailability(store)

  /** 模板安全取值（模板表达式不支持非空断言；渲染前提是 hasPerformance）。 */
  const perf = computed(() => ({
    avgTtftMs: performance.value?.avgTtftMs ?? 0,
    avgTokensPerSecond: performance.value?.avgTokensPerSecond ?? 0,
    slowestModel: performance.value?.slowestModel ?? null,
    fastestModel: performance.value?.fastestModel ?? null,
  }))

  const statusBreakdown = computed(() => store.statisticsSummary?.status ?? null)

  // ---- 分位统计（A）：线性插值分位；慢请求 = 耗时 > P95 的条数 ----

  /** 有效样本（> 0 视为有性能数据），升序排列。 */
  const ttftSamples = computed(() =>
    records.value.map(r => r.ttftMs).filter((v): v is number => !!v && v > 0).sort((a, b) => a - b),
  )
  const durationSamples = computed(() =>
    records.value.map(r => r.durationMs).filter((v): v is number => !!v && v > 0).sort((a, b) => a - b),
  )

  const ttftP50 = computed(() => percentile(ttftSamples.value, 0.5))
  const ttftP95 = computed(() => percentile(ttftSamples.value, 0.95))
  const durationP50 = computed(() => percentile(durationSamples.value, 0.5))
  const durationP95 = computed(() => percentile(durationSamples.value, 0.95))
  /** 慢请求数：耗时严格大于 P95 的条数；无样本时为 null（模板显示 —）。 */
  const slowRequestsCount = computed(() => {
    const p95 = durationP95.value
    if (p95 == null) return null
    return durationSamples.value.filter(v => v > p95).length
  })

  // ---- 状态码分组（C）：2xx / 3xx / 4xx / 5xx ----

  const statusGroups = computed(() => {
    const groups: StatusGroup[] = [
      { key: '2xx', color: 'var(--theme-chart-requests)', count: 0 },
      { key: '3xx', color: 'var(--theme-chart-tokens)', count: 0 },
      { key: '4xx', color: 'var(--theme-chart-cost)', count: 0 },
      { key: '5xx', color: 'var(--theme-chart-series-3)', count: 0 },
    ]
    let total = 0
    for (const r of records.value) {
      const code = r.statusCode
      if (!code || code < 100 || code >= 600) continue
      total++
      const idx = code < 300 ? 0 : code < 400 ? 1 : code < 500 ? 2 : 3
      groups[idx].count++
    }
    return { groups, total }
  })

  // ---- 最慢模型/来源排行（D）：按平均耗时降序 top 5，带请求数 ----

  function rankByDuration(pick: (r: RequestRecord) => { key: string; label: string } | null): RankRow[] {
    const acc = new Map<string, { label: string; total: number; count: number }>()
    for (const r of records.value) {
      const picked = pick(r)
      if (!picked || !r.durationMs || r.durationMs <= 0) continue
      const cur = acc.get(picked.key) ?? { label: picked.label, total: 0, count: 0 }
      cur.total += r.durationMs
      cur.count += 1
      acc.set(picked.key, cur)
    }
    return [...acc.entries()]
      .map(([, v]) => ({ label: v.label, avgMs: v.total / v.count, count: v.count }))
      .sort((a, b) => b.avgMs - a.avgMs)
      .slice(0, 5)
  }

  const slowestModels = computed(() =>
    rankByDuration(r => {
      const model = r.model?.trim()
      return model ? { key: model, label: display.requestModelLabel(r) } : null
    }),
  )

  const slowestSources = computed(() =>
    rankByDuration(r => {
      const source = r.sourceLabel?.trim() || r.requestBaseUrl?.trim() || ''
      return source ? { key: source, label: display.requestSourceLabel(r) } : null
    }),
  )

  // ---- 最慢 20 条请求子表（E）：耗时降序 ----

  const slowestRequests = computed(() =>
    records.value
      .filter(r => !!r.durationMs && r.durationMs > 0)
      .sort((a, b) => (b.durationMs ?? 0) - (a.durationMs ?? 0))
      .slice(0, 20),
  )

  return {
    hasPerformance,
    perf,
    statusBreakdown,
    percentile,
    ttftP50,
    ttftP95,
    durationP50,
    durationP95,
    slowRequestsCount,
    statusGroups,
    rankByDuration,
    slowestModels,
    slowestSources,
    slowestRequests,
  }
}
