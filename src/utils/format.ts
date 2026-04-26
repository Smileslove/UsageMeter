export function formatCost(value: number): string {
  return `$${Number.isFinite(value) ? value.toFixed(4) : '0.0000'}`
}

export function formatRequestCount(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`
  if (value >= 1_000) return `${(value / 1_000).toFixed(2)}K`
  return String(Math.round(value))
}

export function formatTokenValue(value: number, unitBase?: number): string {
  const base = unitBase ?? value
  if (base >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`
  if (base >= 1_000) return `${(value / 1_000).toFixed(2)}K`
  return value.toFixed(2)
}

export function formatTokenPair(input: number, output: number): { input: string; output: string } {
  const base = Math.max(input, output)
  return {
    input: formatTokenValue(input, base),
    output: formatTokenValue(output, base)
  }
}

export function formatDurationMs(value: number): string {
  if (!value || value < 0) return '-'
  if (value < 1000) return `${value.toFixed(0)}ms`
  return `${(value / 1000).toFixed(2)}s`
}

export function formatRate(value: number): string {
  if (!value || value < 0) return '0'
  if (value >= 100) return value.toFixed(0)
  return value.toFixed(1)
}
