<script setup lang="ts">
import CursorUsageNotice from '../../components/CursorUsageNotice.vue'
import DesktopSidebar from './DesktopSidebar.vue'
import DesktopPageHeader from './DesktopPageHeader.vue'
</script>

<template>
  <div
    class="flex h-full w-full overflow-hidden antialiased"
    style="background: var(--theme-bg-workspace)"
  >
    <DesktopSidebar />
    <div class="flex min-w-0 flex-1 flex-col">
      <DesktopPageHeader />
      <!-- 单一主滚动区：页面内避免嵌套滚动（设计文档 4.6） -->
      <!-- h-full 透传高度，让表格型页面可撑满视口做表体内部滚动 -->
      <main class="min-h-0 flex-1 overflow-y-auto overscroll-contain">
        <div class="mx-auto flex h-full w-full max-w-[1440px] flex-col px-6 py-6">
          <CursorUsageNotice class="shrink-0" />
          <div class="min-h-0 flex-1"><slot /></div>
        </div>
      </main>
    </div>
  </div>
</template>
