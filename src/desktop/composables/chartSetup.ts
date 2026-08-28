/**
 * ECharts 图表注册与主题 composable。
 * - registerChartComponents：echarts use() 注册（幂等）。
 * - useChartTheme：响应式主题色 computed（touch theme 设置触发重算）。
 */
import { computed } from 'vue'
import type { ComputedRef } from 'vue'
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { LineChart } from 'echarts/charts'
import { GridComponent, TooltipComponent } from 'echarts/components'
import { useMonitorStore } from '../../stores/monitor'
import { resolveChartTheme } from '../../utils/chartTheme'

export type TrendChartTheme = ReturnType<typeof resolveChartTheme>

export function registerChartComponents(): void {
  use([CanvasRenderer, LineChart, GridComponent, TooltipComponent])
}

export function useChartTheme(): ComputedRef<TrendChartTheme> {
  return computed(() => {
    const store = useMonitorStore()
    void store.settings.theme.appearance
    void store.settings.theme.lightPalette
    void store.settings.theme.darkPalette
    return resolveChartTheme()
  })
}
