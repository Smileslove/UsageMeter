/**
 * 复制到剪贴板 + flash 反馈共享逻辑。
 *
 * 消除 SessionWorkspace / DesktopRequests / DesktopActivity 中几乎逐字重复的
 * writeText + label flash + 定时清除实现（原各自约 10 行）；定时器在组件
 * 卸载时自动清理，调用方无需手动处理。
 *
 * 语义与既有实现逐点一致：
 * - copiedValue 保存 label 字符串（空串表示无 flash），模板仅做 truthy 判断展示固定文案，
 *   不直接输出 label 值，因此 label 传任意非空串即可；
 * - 连续复制会重置计时（先 clearTimeout 再重新计时）；
 * - 剪贴板不可用时静默失败（不抛错、不闪 flash）。
 *
 * 既有 flash 时长有 1200ms（Requests）与 1400ms（Workspace/Activity）两种，
 * 通过 duration 参数保留，默认 1200。
 */
import { onUnmounted, ref } from 'vue'

export interface UseClipboardOptions {
  /** flash 自动清除延迟（ms），默认 1200。 */
  duration?: number
}

export function useClipboard(options: UseClipboardOptions = {}) {
  const { duration = 1200 } = options
  const copiedValue = ref('')
  let copiedTimer: ReturnType<typeof setTimeout> | null = null

  const copyText = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value)
      copiedValue.value = label
      if (copiedTimer) clearTimeout(copiedTimer)
      copiedTimer = setTimeout(() => { copiedValue.value = '' }, duration)
    } catch { /* 剪贴板不可用时静默失败 */ }
  }

  onUnmounted(() => {
    if (copiedTimer) clearTimeout(copiedTimer)
  })

  return { copiedValue, copyText }
}
