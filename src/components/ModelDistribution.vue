<script setup lang="ts">
import { computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

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
  '#F45B69', // 珊瑚红
  '#2DD4BF', // 薄荷绿
  '#818CF8', // 紫罗兰
  '#FBBF24', // 琥珀橙
  '#60A5FA', // 天空蓝
  '#A78BFA'
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
        const textColor = store.settings.theme === 'dark' ? '#E5E7EB' : '#374151'
        const secondaryColor = store.settings.theme === 'dark' ? '#9CA3AF' : '#6B7280'
        const valueColor = store.settings.theme === 'dark' ? '#F3F4F6' : '#111827'

        return `
          <div style="display:flex;align-items:center;margin-bottom:8px;gap:6px;">
            <span style="display:inline-block;width:8px;height:8px;border-radius:50%;background-color:${params.color};"></span>
            <span style="font-weight:600;font-size:13px;color:${textColor}">${params.name}</span>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:baseline;margin-bottom:4px;gap:16px;">
            <span style="font-size:12px;color:${secondaryColor}">${countLabel}</span>
            <span style="font-size:15px;font-weight:700;font-family:monospace;color:${valueColor}">${params.data.requestCount}</span>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:baseline;gap:16px;">
            <span style="font-size:12px;color:${secondaryColor} ;opacity: 0.8;">${percentLabel}</span>
            <span style="font-size:13px;font-weight:600;font-family:monospace;color:${textColor}">${params.value}%</span>
          </div>
        `
      },
      backgroundColor: store.settings.theme === 'dark' ? '#2C2C2E' : '#FFFFFF',
      borderColor: store.settings.theme === 'dark' ? '#3A3A3C' : '#E5E7EB',
      textStyle: {
        color: store.settings.theme === 'dark' ? '#E5E7EB' : '#374151',
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
            color: store.settings.theme === 'dark' ? '#D1D5DB' : '#4B5563', // gray-300 / gray-600
            fontSize: 11,
            width: 80 // 固定名字宽度形成对齐
          },
          percent: {
            color: store.settings.theme === 'dark' ? '#9CA3AF' : '#6B7280', // gray-400 / gray-500
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
  <div v-if="models.length > 0" class="bg-white dark:bg-[#1C1C1E] rounded-xl p-4 shadow-[0_2px_10px_rgba(0,0,0,0.02)] border border-gray-50 dark:border-neutral-800">
    <div class="text-xs font-semibold text-gray-600 dark:text-gray-300 mb-3">
      {{ t(store.settings.locale, 'metrics.modelDistribution') }}
    </div>
    <!-- 使用按需引入的 ECharts 显示模型占比圆环图 -->
    <div class="w-full h-[120px] flex relative">
      <!-- 高度大幅度缩小 -->
      <v-chart class="w-full h-full" :option="chartOptions" autoresize />
      <!-- 中心空心部分可放置说明文本，相对图表位置做偏移 -->
      <div class="absolute left-[25%] top-1/2 -translate-x-1/2 -translate-y-1/2 pointer-events-none">
        <span class="text-[9px] text-gray-400 dark:text-gray-500 uppercase tracking-widest opacity-50">Models</span>
      </div>
    </div>
  </div>
</template>
