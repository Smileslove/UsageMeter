<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { formatRequestCount } from '../utils/format'
import { themeColorVar } from '../theme'

import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { PieChart } from 'echarts/charts'
import { TooltipComponent, LegendComponent } from 'echarts/components'
import VChart from 'vue-echarts'

// 注册所需组件库
use([CanvasRenderer, PieChart, TooltipComponent, LegendComponent])

const store = useMonitorStore()

const models = computed(() => store.snapshot?.modelDistribution ?? [])

// 定义明亮的图表色彩（匹配参考图）
const chartColors = [
  themeColorVar('--theme-chart-series-1'),
  themeColorVar('--theme-chart-series-2'),
  themeColorVar('--theme-chart-series-3'),
  themeColorVar('--theme-chart-series-4'),
  themeColorVar('--theme-chart-series-5'),
  themeColorVar('--theme-chart-series-6')
]

// 构建 ECharts 选项的动态配置
const chartOptions = computed(() => {
  // 只展示非零数据
  const data = models.value
    .filter(m => m.percent > 0)
    .map(m => ({
      name: m.modelName,
      value: Number(m.percent.toFixed(1)), // 保留一位小数
      requestCount: m.requestCount
    }))
    // 按值从大到小排序，使得图表更规整
    .sort((a, b) => b.value - a.value)

  return {
    color: chartColors,
    tooltip: {
      trigger: 'item',
      formatter: (params: any) => {
        const countLabel = t(store.settings.locale, 'metrics.requests') || '请求数'
        const percentLabel = t(store.settings.locale, 'metrics.percent') || '占比'
        const textColor = themeColorVar('--theme-chart-tooltip-text')
        const secondaryColor = themeColorVar('--theme-chart-tooltip-subtext')
        const valueColor = themeColorVar('--theme-text-primary')

        return `
          <div style="display:flex;align-items:center;margin-bottom:8px;gap:6px;">
            <span style="display:inline-block;width:8px;height:8px;border-radius:50%;background-color:${params.color};"></span>
            <span style="font-weight:600;font-size:13px;color:${textColor}">${params.name}</span>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:baseline;margin-bottom:4px;gap:16px;">
            <span style="font-size:12px;color:${secondaryColor}">${countLabel}</span>
            <span style="font-size:15px;font-weight:700;font-family:monospace;color:${valueColor}">${formatRequestCount(params.data.requestCount)}</span>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:baseline;gap:16px;">
            <span style="font-size:12px;color:${secondaryColor} ;opacity: 0.8;">${percentLabel}</span>
            <span style="font-size:13px;font-weight:600;font-family:monospace;color:${textColor}">${params.value}%</span>
          </div>
        `
      },
      backgroundColor: themeColorVar('--theme-chart-tooltip-bg'),
      borderColor: themeColorVar('--theme-chart-tooltip-border'),
      textStyle: {
        color: themeColorVar('--theme-chart-tooltip-text'),
        fontSize: 12
      },
      padding: [8, 12],
      borderRadius: 8
    },
    legend: {
      orient: 'vertical',
      right: '0%', // 靠右对齐
      top: 'middle',
      icon: 'circle',
      itemWidth: 8,
      itemHeight: 8,
      itemGap: 12,
      formatter: (name: string) => {
        const item = data.find(d => d.name === name)
        const percent = item ? item.value.toFixed(1) : '0.0'
        return `{name|${name}}  {percent|${percent}%}`
      },
      textStyle: {
        rich: {
          name: {
            color: themeColorVar('--theme-text-secondary'),
            fontSize: 11,
            width: 80 // 固定名字宽度形成对齐
          },
          percent: {
            color: themeColorVar('--theme-text-tertiary'),
            fontSize: 11,
            fontWeight: 'bold',
            align: 'right',
            width: 45,
            fontFamily: 'monospace'
          }
        }
      }
    },
    series: [
      {
        type: 'pie',
        radius: ['55%', '85%'], // 环形比例（相对于被限制后的包围盒）
        center: ['25%', '50%'], // 偏左放置圆环，右侧留给图例
        avoidLabelOverlap: false,
        padAngle: 3, // 较小的扇区间隙
        itemStyle: {
          borderRadius: 4 // 圆角
        },
        label: {
          show: false // 不再在外部显示线条和文字标签
        },
        labelLine: {
          show: false
        },
        data
      }
    ]
  }
})
</script>

<template>
  <div v-if="models.length > 0" class="theme-card rounded-xl border p-4">
    <div class="mb-3 text-xs font-semibold text-[var(--theme-text-secondary)]">
      {{ t(store.settings.locale, 'metrics.modelDistribution') }}
    </div>
    <!-- 使用按需引入的 ECharts 显示模型占比圆环图 -->
    <div class="w-full h-[120px] flex relative">
      <!-- 高度大幅度缩小 -->
      <v-chart class="w-full h-full" :option="chartOptions" autoresize />
      <!-- 中心空心部分可放置说明文本，相对图表位置做偏移 -->
      <div class="absolute left-[25%] top-1/2 -translate-x-1/2 -translate-y-1/2 pointer-events-none">
        <span class="text-[9px] uppercase tracking-widest text-[var(--theme-text-quaternary)] opacity-50">Models</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.theme-card {
  background: var(--theme-surface-gradient);
  border-color: var(--theme-border-default);
  box-shadow: var(--theme-shadow-inline);
}
</style>
