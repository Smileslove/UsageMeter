import type { StatisticsMetric, StatisticsRangePreset } from '../types'

/**
 * 统计页跨切页的 UI 状态（模块级）。
 *
 * 快速面板以 v-if 切换视图（组件卸载/重挂），统计页的范围/指标/自定义日期
 * 若为组件内 ref 会在切页后重置为默认值（范围回到 today），导致
 * 「切换面板后内容消失、切回仍无内容」。此模块在 WebView 生命周期内
 * 保留状态，供 Statistics.vue 卸载时保存、重挂时恢复。
 */

export interface StatisticsViewState {
  preset: StatisticsRangePreset
  monthMetric: StatisticsMetric
  analysisMetric: StatisticsMetric
  activityView: 'month' | 'year'
  selectedDate: string
  customStart: string
  customEnd: string
}

let persistedState: StatisticsViewState | null = null

export function loadStatisticsViewState(): StatisticsViewState | null {
  return persistedState
}

export function saveStatisticsViewState(view: StatisticsViewState): void {
  persistedState = view
}
