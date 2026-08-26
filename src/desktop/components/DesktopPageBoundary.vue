<script setup lang="ts">
import { onErrorCaptured, ref } from 'vue'
import { TriangleAlert } from 'lucide-vue-next'
import { t } from '../../i18n'
import { useMonitorStore } from '../../stores/monitor'

/**
 * 视图渲染错误边界：捕获后代组件渲染/生命周期错误，显示可诊断的错误卡片
 * 而非空白内容区（此前某视图抛错会让主窗口内容区白屏且难以察觉）。
 *
 * - onErrorCaptured 捕获后返回 false 阻止继续向上传播（避免框架层被波及）；
 * - 错误消息与 info 一并展示（i18n 文案 + 原始消息），便于用户报告与定位；
 * - 「重试」重新挂载插槽内容；错误只发生在渲染期时状态刷新后可能自愈。
 */
const error = ref<Error | null>(null)
const errorInfo = ref('')
const attempt = ref(0)

onErrorCaptured((err, _instance, info) => {
  error.value = err instanceof Error ? err : new Error(String(err))
  errorInfo.value = info
  console.error('[DesktopPageBoundary]', info, err)
  return false
})

function retry(): void {
  error.value = null
  errorInfo.value = ''
  attempt.value += 1
}

const store = useMonitorStore()
const locale = () => store.settings.locale
</script>

<template>
  <div v-if="!error" class="h-full">
    <slot :key="attempt" />
  </div>
  <div
    v-else
    class="flex h-full flex-col items-center justify-center gap-3 px-8 text-center"
    role="alert"
  >
    <TriangleAlert :size="26" class="text-[var(--theme-status-danger-fg)]" aria-hidden="true" />
    <div class="space-y-1">
      <p class="text-sm font-semibold text-[var(--theme-text-primary)]">
        {{ t(locale(), 'desktop.pageRenderError') }}
      </p>
      <p class="mx-auto max-w-md break-words text-xs leading-5 text-[var(--theme-text-tertiary)]">
        {{ error.message }}
      </p>
      <p v-if="errorInfo" class="text-xs text-[var(--theme-text-quaternary)]">
        {{ errorInfo }}
      </p>
    </div>
    <button
      type="button"
      class="rounded-lg border border-[var(--theme-border-default)] px-3 py-1.5 text-xs font-medium text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]"
      @click="retry()"
    >
      {{ t(locale(), 'desktop.pageRenderRetry') }}
    </button>
  </div>
</template>
