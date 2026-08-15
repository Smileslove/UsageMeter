/**
 * 弹层焦点陷阱（设计文档 17.1 / 17.2）。
 *
 * 供抽屉 / 对话框 / 弹层使用，集中实现键盘焦点管理，消除各弹层
 * 重复的 keydown 监听、可聚焦元素收集与 Tab 循环逻辑（原各处
 * 仅有零散的 Escape 监听，Tab 循环与焦点归还完全缺失）：
 * - 打开时自动聚焦容器内第一个可聚焦元素（rAF 等待渲染完成；
 *   容器内无可聚焦元素时临时给容器设 tabindex="-1" 接收焦点，
 *   防止焦点逃逸到页面其余部分）；
 * - Tab / Shift+Tab 在容器内循环，焦点始终困在容器内；
 * - Esc 触发 onClose（关闭动作由调用方翻转 open 完成）；
 * - 关闭后焦点回到触发器（打开前的 activeElement，可通过
 *   restoreFocus 自定义，如触发器为 v-if 重建时传 ref 重新聚焦）；
 * - 卸载时自动移除监听、取消挂起的聚焦帧并恢复焦点，
 *   调用方无需手动处理。
 *
 * 限制：同一时刻只支持单个激活实例——嵌套弹层需由调用方保证
 * 同时只打开一个（内层关闭后再开外层），否则多个实例的
 * Tab 循环会互相覆盖焦点。
 */
import { onUnmounted, watch } from 'vue'
import type { Ref } from 'vue'

export interface UseFocusTrapOptions {
  /** 是否打开：true 激活陷阱并聚焦首个可聚焦元素，false 释放并恢复焦点。 */
  open: Ref<boolean>
  /** 陷阱容器元素（模板 ref）。 */
  container: Ref<HTMLElement | null>
  /** Esc 关闭回调（由调用方翻转 open 完成关闭）。 */
  onClose: () => void
  /** 自定义焦点恢复回调；缺省时自动把焦点还给打开前的元素（触发器）。 */
  restoreFocus?: () => void
}

/** 可聚焦元素选择器（WAI-ARIA 对话框模式推荐列表）。 */
const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
  '[contenteditable]:not([contenteditable="false"])'
].join(',')

export function useFocusTrap(options: UseFocusTrapOptions): void {
  const { open, container, onClose, restoreFocus } = options

  let trapActive = false
  let previouslyFocused: HTMLElement | null = null
  let focusFrame: number | null = null
  let containerHadTabIndex = false
  let containerTabIndex = -1

  /** 收集容器内可见的可聚焦元素（隐藏元素排除在循环外）。 */
  const getFocusable = (): HTMLElement[] => {
    const root = container.value
    if (!root) return []
    return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(el => {
      if (typeof el.getClientRects === 'function' && el.getClientRects().length === 0) return false
      return getComputedStyle(el).visibility !== 'hidden'
    })
  }

  /** 容器内无可聚焦元素时：临时让容器可聚焦以接收焦点，防止焦点逃逸。 */
  const focusContainer = () => {
    const el = container.value
    if (!el) return
    containerHadTabIndex = el.hasAttribute('tabindex')
    containerTabIndex = el.tabIndex
    el.setAttribute('tabindex', '-1')
    el.focus()
  }

  const restoreContainerTabIndex = () => {
    const el = container.value
    if (!el) return
    if (containerHadTabIndex) el.tabIndex = containerTabIndex
    else el.removeAttribute('tabindex')
    containerHadTabIndex = false
  }

  const handleKeydown = (event: KeyboardEvent) => {
    if (!trapActive) return
    if (event.key === 'Escape') {
      event.preventDefault()
      onClose()
      return
    }
    if (event.key !== 'Tab') return

    const focusable = getFocusable()
    if (focusable.length === 0) {
      // 容器内无可聚焦元素：拦截 Tab，避免焦点逃出弹层
      event.preventDefault()
      return
    }
    const first = focusable[0]
    const last = focusable[focusable.length - 1]
    const active = document.activeElement
    const inside = container.value?.contains(active) ?? false

    if (!inside) {
      // 焦点在容器外（理论上只发生在打开瞬间）：Tab 进入容器，Shift+Tab 从末尾进入
      event.preventDefault()
      const target = event.shiftKey ? last : first
      target.focus()
      return
    }
    if (event.shiftKey && active === first) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && active === last) {
      event.preventDefault()
      first.focus()
    }
  }

  const activate = () => {
    trapActive = true
    previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null
    focusFrame = requestAnimationFrame(() => {
      focusFrame = null
      if (!trapActive) return
      const firstFocusable = getFocusable()[0]
      if (firstFocusable) firstFocusable.focus()
      else focusContainer()
    })
  }

  const deactivate = () => {
    if (focusFrame !== null) {
      cancelAnimationFrame(focusFrame)
      focusFrame = null
    }
    if (!trapActive) return
    trapActive = false
    restoreContainerTabIndex()
    if (restoreFocus) {
      restoreFocus()
    } else if (previouslyFocused && previouslyFocused.isConnected) {
      previouslyFocused.focus()
    }
    previouslyFocused = null
  }

  watch(
    open,
    isOpen => {
      if (isOpen) activate()
      else deactivate()
    },
    { immediate: true }
  )

  window.addEventListener('keydown', handleKeydown)
  onUnmounted(() => {
    deactivate()
    window.removeEventListener('keydown', handleKeydown)
  })
}
