import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))

// 被测模块通过 @tauri-apps/api/core 的 invoke 发起真实 Tauri 调用，
// 测试环境（node）下没有 Tauri 运行时，必须桩化。
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { invokeWithTimeout } from './invokeWithTimeout'

const DEFAULT_TIMEOUT_MS = 120000

describe('invokeWithTimeout', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    invokeMock.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('rejects with ERR_STATISTICS_TIMEOUT after the default 120000ms', async () => {
    // invoke 永不 resolve：只有超时通路能结束 race
    invokeMock.mockReturnValue(new Promise(() => {}))

    const promise = invokeWithTimeout('get_statistics_summary', { query: {} })

    vi.advanceTimersByTime(DEFAULT_TIMEOUT_MS - 1)
    // 超时前一刻仍未 settle
    await expect(Promise.race([promise, Promise.resolve('still-pending')])).resolves.toBe('still-pending')

    vi.advanceTimersByTime(1)
    await expect(promise).rejects.toThrow('ERR_STATISTICS_TIMEOUT')
    expect(invokeMock).toHaveBeenCalledWith('get_statistics_summary', { query: {} })
  })

  it('resolves with the invoke value and clears the pending timer when invoke wins', async () => {
    invokeMock.mockResolvedValue({ ok: true } as never)
    const clearSpy = vi.spyOn(globalThis, 'clearTimeout')

    const promise = invokeWithTimeout('cmd', { a: 1 })

    await expect(promise).resolves.toEqual({ ok: true })
    // finally 清理了未触发的超时 timer
    expect(clearSpy).toHaveBeenCalled()

    // 即便再推进远超默认超时，也不会再 reject（timer 已清理）
    vi.advanceTimersByTime(DEFAULT_TIMEOUT_MS + 1000)
    await expect(promise).resolves.toEqual({ ok: true })
  })

  it('respects a custom timeoutMs instead of the default', async () => {
    invokeMock.mockReturnValue(new Promise(() => {}))

    const promise = invokeWithTimeout('cmd', {}, 1000)

    vi.advanceTimersByTime(999)
    await expect(Promise.race([promise, Promise.resolve('still-pending')])).resolves.toBe('still-pending')

    vi.advanceTimersByTime(1)
    await expect(promise).rejects.toThrow('ERR_STATISTICS_TIMEOUT')
  })

  it('does not override the timeout rejection when invoke resolves late', async () => {
    let resolveInvoke!: (value: { late: boolean }) => void
    invokeMock.mockReturnValue(new Promise(resolve => { resolveInvoke = resolve }))

    const promise = invokeWithTimeout('cmd', {})

    vi.advanceTimersByTime(DEFAULT_TIMEOUT_MS)
    await expect(promise).rejects.toThrow('ERR_STATISTICS_TIMEOUT')

    // 超时后才返回的 invoke 结果无法改变已 reject 的结果
    resolveInvoke({ late: true })
    await expect(promise).rejects.toThrow('ERR_STATISTICS_TIMEOUT')
  })
})
