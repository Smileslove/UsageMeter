import { loadSessionBlockData } from 'ccusage/data-loader'

// 计算 token：返回总 Token = input + cacheRead + output（不包含 cacheCreate）
function entryTokens(entry) {
  const usage = entry?.usage ?? {}
  const input = Number(usage.inputTokens ?? 0)
  const output = Number(usage.outputTokens ?? 0)
  const cacheRead = Number(usage.cacheReadInputTokens ?? 0)
  return input + output + cacheRead
}

// 返回四种 token 的详细分类
function entryTokenBreakdown(entry) {
  const usage = entry?.usage ?? {}
  return {
    input: Number(usage.inputTokens ?? 0),
    output: Number(usage.outputTokens ?? 0),
    cacheCreate: Number(usage.cacheCreationInputTokens ?? 0),
    cacheRead: Number(usage.cacheReadInputTokens ?? 0)
  }
}

function entryCost(entry) {
  return Number(entry?.costUSD ?? 0)
}

// 计算本月开始时间戳（本月1日 00:00:00）
function getCurrentMonthStartMs() {
  const now = new Date()
  return new Date(now.getFullYear(), now.getMonth(), 1, 0, 0, 0, 0).getTime()
}

function buildSnapshot(blocks) {
  const now = Date.now()
  const currentMonthStartMs = getCurrentMonthStartMs()

  // 每个窗口的模型分布统计
  const windowModelStats = {
    '5h': { stats: new Map(), maxAgeMs: 5 * 60 * 60 * 1000 },
    '1d': { stats: new Map(), maxAgeMs: 24 * 60 * 60 * 1000 },
    '7d': { stats: new Map(), maxAgeMs: 7 * 24 * 60 * 60 * 1000 },
    '30d': { stats: new Map(), maxAgeMs: 30 * 24 * 60 * 60 * 1000 },
    'current_month': { stats: new Map(), startMs: currentMonthStartMs },
  }

  const windows = {
    '5h': { tokenUsed: 0, inputTokens: 0, outputTokens: 0, cacheCreateTokens: 0, cacheReadTokens: 0, requestUsed: 0, cost: 0, maxAgeMs: 5 * 60 * 60 * 1000 },
    '1d': { tokenUsed: 0, inputTokens: 0, outputTokens: 0, cacheCreateTokens: 0, cacheReadTokens: 0, requestUsed: 0, cost: 0, maxAgeMs: 24 * 60 * 60 * 1000 },
    '7d': { tokenUsed: 0, inputTokens: 0, outputTokens: 0, cacheCreateTokens: 0, cacheReadTokens: 0, requestUsed: 0, cost: 0, maxAgeMs: 7 * 24 * 60 * 60 * 1000 },
    '30d': { tokenUsed: 0, inputTokens: 0, outputTokens: 0, cacheCreateTokens: 0, cacheReadTokens: 0, requestUsed: 0, cost: 0, maxAgeMs: 30 * 24 * 60 * 60 * 1000 },
    'current_month': { tokenUsed: 0, inputTokens: 0, outputTokens: 0, cacheCreateTokens: 0, cacheReadTokens: 0, requestUsed: 0, cost: 0, startMs: currentMonthStartMs },
  }

  // 新增: 模型统计
  const modelStats = new Map()

  // 新增: 总成本
  let totalCost = 0

  // ccusage 数据没有 messageId，按 timestamp + model 组合去重
  const seenKeys = new Set()

  for (const block of blocks ?? []) {
    for (const entry of block?.entries ?? []) {
      const ts = new Date(entry?.timestamp ?? 0).getTime()
      if (!Number.isFinite(ts) || ts <= 0) {
        continue
      }

      const tokens = entryTokens(entry)
      if (tokens <= 0) {
        continue
      }

      // 使用 timestamp 作为唯一标识去重（同一秒内同模型的记录）
      const model = entry?.model ?? 'unknown'
      const key = `${ts}-${model}`
      if (seenKeys.has(key)) {
        continue
      }
      seenKeys.add(key)

      const breakdown = entryTokenBreakdown(entry)
      const cost = entryCost(entry)
      const age = now - ts

      // 滚动时间窗口统计（5h, 1d, 7d, 30d）
      for (const [windowName, info] of Object.entries(windows)) {
        if (windowName === 'current_month') continue
        if (age <= info.maxAgeMs) {
          info.tokenUsed += tokens
          info.inputTokens += breakdown.input
          info.outputTokens += breakdown.output
          info.cacheCreateTokens += breakdown.cacheCreate
          info.cacheReadTokens += breakdown.cacheRead
          info.requestUsed += 1
          info.cost += cost

          // 窗口模型分布统计
          const windowStats = windowModelStats[windowName].stats
          if (!windowStats.has(model)) {
            windowStats.set(model, { inputTokens: 0, outputTokens: 0, cacheCreateTokens: 0, cacheReadTokens: 0 })
          }
          const ms = windowStats.get(model)
          ms.inputTokens += breakdown.input
          ms.outputTokens += breakdown.output
          ms.cacheCreateTokens += breakdown.cacheCreate
          ms.cacheReadTokens += breakdown.cacheRead
        }
      }

      // 本月统计（自然月）
      if (ts >= windows['current_month'].startMs) {
        windows['current_month'].tokenUsed += tokens
        windows['current_month'].inputTokens += breakdown.input
        windows['current_month'].outputTokens += breakdown.output
        windows['current_month'].cacheCreateTokens += breakdown.cacheCreate
        windows['current_month'].cacheReadTokens += breakdown.cacheRead
        windows['current_month'].requestUsed += 1
        windows['current_month'].cost += cost

        // 窗口模型分布统计
        const windowStats = windowModelStats['current_month'].stats
        if (!windowStats.has(model)) {
          windowStats.set(model, { inputTokens: 0, outputTokens: 0, cacheCreateTokens: 0, cacheReadTokens: 0 })
        }
        const ms = windowStats.get(model)
        ms.inputTokens += breakdown.input
        ms.outputTokens += breakdown.output
        ms.cacheCreateTokens += breakdown.cacheCreate
        ms.cacheReadTokens += breakdown.cacheRead
      }

      // 模型统计（仅统计30天内的数据）
      if (age <= 30 * 24 * 60 * 60 * 1000) {
        if (!modelStats.has(model)) {
          modelStats.set(model, {
            tokenUsed: 0,
            inputTokens: 0,
            outputTokens: 0,
            cacheCreateTokens: 0,
            cacheReadTokens: 0,
            requestCount: 0,
            cost: 0
          })
        }
        const stats = modelStats.get(model)
        stats.tokenUsed += tokens
        stats.inputTokens += breakdown.input
        stats.outputTokens += breakdown.output
        stats.cacheCreateTokens += breakdown.cacheCreate
        stats.cacheReadTokens += breakdown.cacheRead
        stats.requestCount += 1
        stats.cost += cost
      }

      // 累加总成本（仅统计30天内的数据）
      if (age <= 30 * 24 * 60 * 60 * 1000) {
        totalCost += cost
      }
    }
  }

  // 计算模型占比
  const totalTokens = Array.from(modelStats.values())
    .reduce((sum, s) => sum + s.tokenUsed, 0)

  const modelDistribution = Array.from(modelStats.entries())
    .map(([modelName, stats]) => ({
      modelName,
      tokenUsed: stats.tokenUsed,
      inputTokens: stats.inputTokens,
      outputTokens: stats.outputTokens,
      cacheCreateTokens: stats.cacheCreateTokens,
      cacheReadTokens: stats.cacheReadTokens,
      requestCount: stats.requestCount,
      percent: totalTokens > 0 ? (stats.tokenUsed / totalTokens * 100) : 0,
      // ccusage 模式无状态码信息
      statusCodes: [],
    }))
    .sort((a, b) => b.tokenUsed - a.tokenUsed)
    .slice(0, 5)  // Top 5

  // 转换窗口模型分布为数组格式
  const windowModelDistribution = {}
  for (const [windowName, { stats }] of Object.entries(windowModelStats)) {
    windowModelDistribution[windowName] = Array.from(stats.entries())
      .map(([modelName, ms]) => ({
        modelName,
        inputTokens: ms.inputTokens,
        outputTokens: ms.outputTokens,
        cacheCreateTokens: ms.cacheCreateTokens,
        cacheReadTokens: ms.cacheReadTokens,
      }))
      .sort((a, b) => (b.inputTokens + b.outputTokens) - (a.inputTokens + a.outputTokens))
  }

  return {
    source: 'ccusage-api',
    windows: Object.entries(windows).map(([window, info]) => ({
      window,
      tokenUsed: info.tokenUsed,
      inputTokens: info.inputTokens,
      outputTokens: info.outputTokens,
      cacheCreateTokens: info.cacheCreateTokens,
      cacheReadTokens: info.cacheReadTokens,
      requestUsed: info.requestUsed,
      cost: info.cost,  // 添加 ccusage 计算的费用
    })),
    totalCost,
    modelDistribution,
    windowModelDistribution,
  }
}

async function main() {
  const blocks = await loadSessionBlockData({
    sessionDurationHours: 5,
    mode: 'calculate',
    offline: false,  // 使用在线模式从 LiteLLM 获取价格数据
  })

  const snapshot = buildSnapshot(blocks)
  process.stdout.write(JSON.stringify(snapshot))
}

main().catch((error) => {
  process.stderr.write(String(error?.stack ?? error))
  process.exit(1)
})
