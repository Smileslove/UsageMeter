<script setup lang="ts">
import { ref, computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import SummaryPanel from '../components/SummaryPanel.vue'
import WindowCard from '../components/WindowCard.vue'
import { WINDOW_ORDER, type WindowName } from '../types'

const store = useMonitorStore()
const displayMode = ref<'auto' | 'exact'>('auto')

// 获取启用的窗口数据（按照固定顺序排列）
const enabledWindows = computed(() => {
  const enabled = store.settings.quotas
    .filter(q => q.enabled)
    .map(q => store.windows.find(w => w.window === q.window))
    .filter((w): w is NonNullable<typeof w> => w !== undefined)

  // 按照固定顺序排序
  return enabled.sort((a, b) => {
    const indexA = WINDOW_ORDER.indexOf(a.window as WindowName)
    const indexB = WINDOW_ORDER.indexOf(b.window as WindowName)
    return indexA - indexB
  })
})

// 动态计算网格列数：所有模式都使用双列布局（因为现在使用嵌套圆环，空间足够）
const gridCols = computed(() => 'grid-cols-2')

// 切换显示模式
const toggleDisplayMode = () => {
  displayMode.value = displayMode.value === 'auto' ? 'exact' : 'auto'
}
</script>

<template>
  <div class="space-y-2 animate-in fade-in zoom-in-95 duration-300 pb-1">
    <!-- 顶部汇总面板 -->
    <SummaryPanel />

    <!-- 时间窗口卡片列表 -->
    <div :class="gridCols" class="grid gap-2">
      <WindowCard v-for="window in enabledWindows" :key="window.window" :window="window" :display-mode="displayMode" @click="toggleDisplayMode" />
    </div>
  </div>
</template>
