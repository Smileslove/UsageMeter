<script setup lang="ts">
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'

const store = useMonitorStore()
</script>
<template>
  <div class="space-y-3 animate-in fade-in zoom-in-95 duration-300 pb-4">
    <!-- Development Notice -->
    <div class="bg-amber-50 dark:bg-amber-900/20 border border-amber-200 dark:border-amber-800/30 rounded-xl p-3 flex items-center gap-2">
      <svg class="w-4 h-4 text-amber-500 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
      </svg>
      <span class="text-[11px] font-medium text-amber-700 dark:text-amber-300">{{ t(store.settings.locale, 'common.underDevelopment') }}</span>
    </div>
    <!-- Single list item accordion -->
    <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden shadow-sm">
      <div class="p-3.5 cursor-pointer hover:bg-gray-50 dark:hover:bg-[#2C2C2E] transition-colors flex justify-between items-center">
        <div class="flex flex-col gap-1 overflow-hidden pr-2">
          <span class="text-sm font-semibold truncate text-gray-800 dark:text-gray-100">重构用户认证模块重构用户认证模块</span>
          <span class="text-[10px] text-blue-500 bg-blue-50 dark:bg-blue-900/20 px-1.5 py-0.5 rounded w-max border border-blue-100 dark:border-blue-900/30">claude-3-5-sonnet</span>
        </div>
        <span class="text-xs font-mono text-gray-500 dark:text-gray-400 whitespace-nowrap">6.9k <span class="text-[10px] text-gray-300 dark:text-gray-500 font-sans">tok</span></span>
      </div>
      <!-- Expanded View -->
      <div class="px-3.5 pb-4 bg-gray-50/50 dark:bg-[#1C1C1E] border-t border-gray-50 dark:border-neutral-800 pt-3">
         <div class="flex justify-between items-end mb-1.5">
           <span class="text-[10px] text-gray-400">输入 / 输出</span>
           <span class="text-[11px] font-mono text-green-600 bg-green-50 dark:bg-green-500/10 px-1.5 rounded-sm">28 tok/s</span>
         </div>
         <div class="w-full flex h-2 bg-gray-200 rounded-full overflow-hidden dark:bg-neutral-800">
           <div class="bg-cyan-400 h-full" style="width: 70%"></div>
           <div class="bg-fuchsia-400 h-full" style="width: 30%"></div>
         </div>
      </div>
    </div>

    <!-- Collapsed View -->
    <div class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 overflow-hidden shadow-sm">
      <div class="p-3.5 cursor-pointer hover:bg-gray-50 dark:hover:bg-[#2C2C2E] transition-colors flex justify-between items-center">
        <div class="flex flex-col gap-1 overflow-hidden pr-2">
          <span class="text-sm font-semibold truncate text-gray-800 dark:text-gray-100">编写单元测试</span>
          <span class="text-[10px] text-blue-500 bg-blue-50 dark:bg-blue-900/20 px-1.5 py-0.5 rounded w-max border border-blue-100 dark:border-blue-900/30">claude-3-5-sonnet</span>
        </div>
        <span class="text-xs font-mono text-gray-500 dark:text-gray-400 whitespace-nowrap">4.3k <span class="text-[10px] text-gray-300 dark:text-gray-500 font-sans">tok</span></span>
      </div>
    </div>
  </div>
</template>
