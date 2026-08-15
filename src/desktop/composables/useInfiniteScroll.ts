/**
 * 触底续载（IntersectionObserver + sentinel 触发）共享逻辑。
 *
 * 消除 DesktopActivity / DesktopSessions / DesktopRequests 三页重复的
 * observer 创建、sentinel 观察、hasMore/loading 门控与卸载清理（原各自约 15 行）。
 *
 * 三页差异通过参数化保留（不强行统一）：
 * - trigger 元素 ref 名不同（sentinel / loadMoreTrigger / requestLoadMoreTrigger）→ 传入 trigger；
 * - rootMargin 不同（Activity 240px，Sessions/Requests 160px）→ rootMargin 参数；
 * - 触发门控不同（Activity 额外要求 viewState === 'ready' 且非初始 loading）→ enabled 参数；
 * - observer 创建时机不同（Sessions/Requests 延迟 100ms 等首屏渲染）→ delay 参数；
 * - trigger 重建/列表变化后的重新观察（Activity watch trigger 自身；Sessions/Requests
 *   watch 列表长度 + 60ms 防抖）→ reobserve 源 + reobserveDelay 参数。
 *
 * 生命周期：observer 与内部定时器（创建延迟 / 重观察防抖）在组件卸载时自动
 * disconnect / 清理，调用方无需手动处理。
 */
import { onMounted, onUnmounted, watch } from 'vue'
import type { Ref, WatchSource } from 'vue'

export interface UseInfiniteScrollOptions {
  /** 触底哨兵元素（模板 ref）。 */
  trigger: Ref<HTMLElement | null>
  /** 触底且门控通过时加载下一页（异步函数亦接受，结果 void 透传）。 */
  onLoadMore: () => void | Promise<void>
  /** 是否还有更多数据（false 后不再触发）。 */
  hasMore: () => boolean
  /** 加载中标志（true 时跳过触发，防重入）。 */
  loading: () => boolean
  /** 额外的触发允许判断（如页面 viewState 门控），默认恒允许。 */
  enabled?: () => boolean
  /** observer 创建延迟（ms），等待首屏/列表渲染完成；Sessions/Requests 原为 100。 */
  delay?: number
  /** IntersectionObserver rootMargin，默认 '160px'。 */
  rootMargin?: string
  /** 重新观察源（trigger 自身或列表长度等）：变化后按 reobserveDelay 防抖重新 observe。 */
  reobserve?: WatchSource
  /** 重新观察防抖延迟（ms），默认 0（下一宏任务内执行）。 */
  reobserveDelay?: number
}

export function useInfiniteScroll(options: UseInfiniteScrollOptions): void {
  const {
    trigger,
    onLoadMore,
    hasMore,
    loading,
    enabled,
    delay = 0,
    rootMargin = '160px',
    reobserve,
    reobserveDelay = 0
  } = options

  let observer: IntersectionObserver | null = null
  let observeTimer: ReturnType<typeof setTimeout> | null = null
  let createTimer: ReturnType<typeof setTimeout> | null = null

  /** trigger 已渲染/重建时重新观察（observer 未创建时静默跳过，创建后自会 observe 一次）。 */
  const observeTrigger = () => {
    if (!observer || !trigger.value) return
    observer.observe(trigger.value)
  }
  const scheduleObserve = () => {
    if (observeTimer) clearTimeout(observeTimer)
    observeTimer = setTimeout(() => {
      observeTimer = null
      observeTrigger()
    }, reobserveDelay)
  }

  if (reobserve) watch(reobserve, scheduleObserve)

  onMounted(() => {
    createTimer = setTimeout(() => {
      createTimer = null
      observer = new IntersectionObserver(entries => {
        const entry = entries[0]
        if (!entry?.isIntersecting) return
        if (entry.target !== trigger.value) return
        if (!hasMore() || loading()) return
        if (enabled && !enabled()) return
        void onLoadMore()
      }, { root: null, rootMargin })
      observeTrigger()
    }, delay)
  })

  onUnmounted(() => {
    if (createTimer) clearTimeout(createTimer)
    if (observeTimer) clearTimeout(observeTimer)
    observer?.disconnect()
    observer = null
  })
}
