import { ref } from 'vue'
import { defineStore } from 'pinia'
import { useMonitorStore } from '../../stores/monitor'
import type { StatisticsMetric, WindowName } from '../../types'

// 纯函数已抽取至 utils 层，此处 re-export 保持向后兼容（避免破坏现有 import 方）。
export { bucketForWindow, startOfBusinessDay, addDays, rangeQueryForWindow, previousRangeQuery } from '../../utils/analyticsRange'
export { formatCountdownSeconds } from '../../utils/format'
export { resolveChartTheme, withAlpha } from '../../utils/chartTheme'

/**
 * 主窗口的窗口局部状态（设计文档 3.5：主窗口持久化的分析范围、分析指标等）。
 *
 * M1 先存于内存（跨页面切换保持），后续版本可对接后端 storage 做持久化。
 * 时间范围字段供 DesktopPageHeader 顶栏读写：概览页 → overviewWindow，
 * 分析页 → analyticsWindow，其余页面（会话/活动占位页）沿用概览范围。
 */

export type AnalyticsView = 'trend' | 'composition' | 'activity' | 'performance'
export type AnalyticsGranularity = 'auto' | 'hour' | 'day'
export type AnalyticsCompare = 'none' | 'previous'

export const useDesktopAnalyticsStore = defineStore('desktopAnalytics', () => {
  const monitor = useMonitorStore()

  /** 概览页时间范围；首次进入继承快速面板 summaryWindow（设计 6.2），之后主窗口记忆自己的值。 */
  const overviewWindow = ref<WindowName>('24h')
  /** 分析页时间范围。 */
  const analyticsWindow = ref<WindowName>('24h')
  /** 概览/分析主指标：费用 / 请求 / Token（设计 6.4、7.3）。 */
  const analyticsMetric = ref<StatisticsMetric>('cost')
  /** 分析页二级视图（设计 4.2：同一页面内切换，不加入侧栏）。 */
  const analyticsView = ref<AnalyticsView>('trend')
  /** 趋势粒度：自动 / 小时 / 天（设计 7.3）。 */
  const analyticsGranularity = ref<AnalyticsGranularity>('auto')
  /** 比较方式：无 / 上一等长周期（设计 7.3）。 */
  const analyticsCompare = ref<AnalyticsCompare>('none')
  /** 分析页组合筛选：模型多选（空数组 = 全部；设计 7.3 筛选摘要）。 */
  const analyticsModelFilter = ref<string[]>([])
  /** 分析页组合筛选：项目多选（空数组 = 全部；设计 7.3 筛选摘要）。 */
  const analyticsProjectFilter = ref<string[]>([])

  /** 是否已从快速面板继承过 summaryWindow（只继承一次，避免覆盖用户后续手动选择）。 */
  const overviewWindowInherited = ref(false)

  /**
   * 首次进入概览页时，以快速面板 summaryWindow 初始化概览范围。
   * 调用方需保证 settings 已从后端加载（例如 monitor.snapshot 非空时调用）。
   */
  function inheritSummaryWindowOnce() {
    if (overviewWindowInherited.value) return
    overviewWindowInherited.value = true
    overviewWindow.value = monitor.settings.summaryWindow
  }

  return {
    overviewWindow,
    analyticsWindow,
    analyticsMetric,
    analyticsView,
    analyticsGranularity,
    analyticsCompare,
    analyticsModelFilter,
    analyticsProjectFilter,
    inheritSummaryWindowOnce
  }
})
