import type { WindowName, WindowUsage } from '../types'

/**
 * 窗口数据回退纯函数（供桌面概览页与单测共用）。
 *
 * 语义与 DesktopOverview 历史实现完全一致：
 * 当前所选窗口有真实用量 → 使用当前窗口；否则按数据覆盖最全的长窗口优先
 * （30d → current_month → 7d → 24h → today → 5h）回退到最近一个有数据的窗口；
 * 全部无数据时仍展示首选窗口（保持结构不空白），窗口列表为空则返回 null。
 */

/** 当前窗口无数据时的回退优先级（优先数据覆盖最全的长窗口）。 */
export const FALLBACK_WINDOW_ORDER = ['30d', 'current_month', '7d', '24h', 'today', '5h'] as const

/** 窗口是否有真实用量（request / token / cost 任一非零）。 */
export function hasAnyUsage(d: WindowUsage | null | undefined): boolean {
  if (!d) return false
  return (
    (d.requestUsed ?? 0) > 0 ||
    (d.tokenUsed ?? 0) > 0 ||
    (d.cost ?? 0) > 0
  )
}

export interface EffectiveWindowPick {
  /** 实际生效的窗口；窗口列表为空时为 null。 */
  window: WindowName | null
  /** 是否处于回退态（首选窗口存在但无数据，实际展示了其他窗口）。 */
  isFallback: boolean
}

/** 从窗口列表中挑选实际生效窗口（含回退）。 */
export function pickEffectiveWindow(
  windows: readonly WindowUsage[],
  preferredWindow: WindowName
): EffectiveWindowPick {
  const preferred = windows.find(w => w.window === preferredWindow) ?? null
  if (hasAnyUsage(preferred)) {
    return { window: preferred!.window as WindowName, isFallback: false }
  }
  // 当前窗口暂无数据：回退到最近一个有数据的窗口，避免概览空白。
  // isFallback 与组件历史实现一致：仅当首选窗口存在（但无数据）被替换时才提示，
  // 首选窗口不在列表中（后端未返回该窗口）时回退不提示。
  for (const name of FALLBACK_WINDOW_ORDER) {
    const candidate = windows.find(w => w.window === name)
    if (candidate && hasAnyUsage(candidate)) {
      return { window: candidate.window as WindowName, isFallback: !!preferred }
    }
  }
  const shown = preferred ?? windows[0] ?? null
  return { window: shown ? (shown.window as WindowName) : null, isFallback: false }
}
