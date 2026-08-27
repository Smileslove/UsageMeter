/**
 * ECharts canvas 渲染器无法解析 var(--x) 字符串，需要把主题 CSS 变量解析为具体颜色。
 * 页面内把 theme 设置（appearance/palette）作为响应式依赖 touch 后重算，图表即可跟随主题。
 */
export function resolveChartTheme(): {
  requests: string
  tokens: string
  cost: string
  series3: string
  axis: string
  grid: string
  elevated: string
  tooltipBg: string
  tooltipBorder: string
  tooltipText: string
  tooltipSub: string
} {
  function cssVar(name: string): string {
    const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
    return value || '#7c8aa0'
  }
  return {
    requests: cssVar('--theme-chart-requests'),
    tokens: cssVar('--theme-chart-tokens'),
    cost: cssVar('--theme-chart-cost'),
    series3: cssVar('--theme-chart-series-3'),
    axis: cssVar('--theme-chart-axis'),
    grid: cssVar('--theme-chart-grid'),
    elevated: cssVar('--theme-bg-elevated'),
    tooltipBg: cssVar('--theme-chart-tooltip-bg'),
    tooltipBorder: cssVar('--theme-chart-tooltip-border'),
    tooltipText: cssVar('--theme-chart-tooltip-text'),
    tooltipSub: cssVar('--theme-chart-tooltip-subtext')
  }
}

/** 给 6 位 hex 颜色追加透明度（如 #10b981 + '26' → #10b98126），非法颜色原样返回避免图表渲染报错。 */
export function withAlpha(color: string, alphaHex: string): string {
  return /^#[0-9a-fA-F]{6}$/.test(color) ? `${color}${alphaHex}` : color
}
