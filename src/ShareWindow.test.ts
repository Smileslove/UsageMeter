// @vitest-environment happy-dom
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import ShareWindow from './ShareWindow.vue'
import { createDefaultSettings } from './stores/monitorDefaults'
import { SHARE_THEMES } from './components/statistics/shareThemes'
import type { StatisticsSummary } from './types'

const { invokeMock, toBlobMock, toJpegMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(), toBlobMock: vi.fn(), toJpegMock: vi.fn()
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('html-to-image', () => ({ toBlob: toBlobMock, toJpeg: toJpegMock, toPng: vi.fn() }))

const summary: StatisticsSummary = {
  generatedAtEpoch: 1791000000,
  source: 'local-files',
  capability: { hasBasicUsage: true, hasPerformance: false, hasStatusCodes: false },
  range: { startEpoch: 1790964000, endEpoch: 1791000000, timezone: 'Asia/Shanghai', bucket: 'hour' },
  totals: {
    totalTokens: 15238536, inputTokens: 15186196, outputTokens: 52340, requestCount: 111,
    cacheReadTokens: 0, cacheCreateTokens: 0, cost: 2.8269, modelCount: 0, localRequestCount: 111, proxyRequestCount: 0
  },
  trend: [], models: []
}

let wrapper: ReturnType<typeof mount> | undefined
beforeEach(() => {
  vi.clearAllMocks()
  setActivePinia(createPinia())
  invokeMock.mockImplementation(async (command: string) => command === 'load_settings' ? createDefaultSettings() : summary)
  toBlobMock.mockResolvedValue(new Blob(['png'], { type: 'image/png' }))
  toJpegMock.mockResolvedValue('data:image/jpeg;base64,anBlZw==')
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { callback(0); return 0 })
})
afterEach(() => {
  wrapper?.unmount()
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

async function mountShare() {
  wrapper = mount(ShareWindow)
  await flushPromises()
  return wrapper
}

describe('share poster controls and export', () => {
  it('ends the current-month calendar on the last day rather than including the next month', async () => {
    vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout'] })
    vi.setSystemTime(new Date(2026, 9, 3, 12))
    const view = await mountShare()
    await view.findAll('.share-window__range-grid button')[5].trigger('click')
    await vi.advanceTimersByTimeAsync(200)
    await flushPromises()
    const dates = view.findAll('.share-window__preview .share-card__calendar time').map(cell => cell.attributes('datetime'))
    expect(dates).toHaveLength(31)
    expect(dates.at(-1)).toBe('2026-10-31')
    expect(dates).not.toContain('2026-11-01')
  })

  it('applies content choices to both the visible preview and the exported poster', async () => {
    const view = await mountShare()
    expect(view.findAll('.share-card__proof-grid')).toHaveLength(2)
    expect(view.findAll('.share-card__panel--trend')).toHaveLength(2)
    expect(view.findAll('.share-card__panel--models')).toHaveLength(2)
    for (const toggle of view.findAll('input[role="switch"]')) await toggle.setValue(false)
    expect(view.findAll('.share-card__proof-grid')).toHaveLength(0)
    expect(view.findAll('.share-card__panel')).toHaveLength(0)
    expect(view.findAll('.share-card__title-row h1').map(node => node.text())).toEqual(['15,238,536', '15,238,536'])
  })

  it('keeps preview and export colors in sync when choosing a light poster', async () => {
    const view = await mountShare()
    const appearanceButtons = view.findAll('.share-window__theme-appearances button')
    await appearanceButtons[1].trigger('click')
    const lightThemes = SHARE_THEMES.filter(theme => theme.appearance === 'light')
    expect(view.findAll('.share-window__swatch')).toHaveLength(lightThemes.length)
    for (const [index, theme] of lightThemes.entries()) {
      await view.findAll('.share-window__swatch')[index].trigger('click')
      for (const poster of view.findAll('.share-card')) {
        expect((poster.element as HTMLElement).style.getPropertyValue('--share-background')).toBe(theme.background)
        expect((poster.element as HTMLElement).style.getPropertyValue('--share-paper')).toBe(theme.foreground)
      }
    }
    await appearanceButtons[1].trigger('click')
    expect(view.findAll('.share-window__swatch').at(-1)?.attributes('aria-pressed')).toBe('true')
    await view.findAll('.share-window__actions > button')[1].trigger('click')
    await flushPromises()
    expect(toBlobMock).toHaveBeenCalledWith(view.get('.share-window__capture .share-card').element, expect.objectContaining({
      backgroundColor: lightThemes.at(-1)?.background
    }))
    await appearanceButtons[0].trigger('click')
    expect(view.get('.share-window__preview .share-card').attributes('style')).toContain(SHARE_THEMES[0].background)
  })

  it('exports JPEG with the right extension while copying PNG regardless of the chosen format', async () => {
    const createUrl = vi.fn((_blob: Blob | MediaSource) => 'blob:share-test')
    const clipboardWrite = vi.fn().mockResolvedValue(undefined)
    vi.spyOn(URL, 'createObjectURL').mockImplementation(createUrl)
    vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {})
    const downloads: string[] = []
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) { downloads.push(this.download) })
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ blob: async () => new Blob(['jpeg'], { type: 'image/jpeg' }) }))
    vi.stubGlobal('navigator', { clipboard: { write: clipboardWrite } })
    vi.stubGlobal('ClipboardItem', class { constructor(public items: Record<string, Blob>) {} })
    const view = await mountShare()
    await view.findAll('.share-window__format-grid button')[1].trigger('click')
    await view.get('.share-window__primary').trigger('click')
    await flushPromises()
    expect(toJpegMock).toHaveBeenCalledWith(view.get('.share-window__capture .share-card').element, expect.objectContaining({ pixelRatio: 2, width: 1200, height: 1760 }))
    expect((createUrl.mock.calls[0][0] as Blob).type).toBe('image/jpeg')
    expect(downloads[0]).toMatch(/\.jpg$/)
    await view.findAll('.share-window__actions > button')[1].trigger('click')
    await flushPromises()
    expect(toBlobMock).toHaveBeenCalledOnce()
    expect(clipboardWrite.mock.calls[0][0][0].items['image/png'].type).toBe('image/png')
    expect(view.get('[role="status"]').text()).toContain('图片已复制')
  })

  it('disables export after a loading failure and allows retrying', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'load_settings') return createDefaultSettings()
      throw new Error('statistics unavailable')
    })
    const view = await mountShare()
    expect(view.get('.share-window__primary').attributes('disabled')).toBeDefined()
    expect(view.get('[role="status"]').text()).toContain('用量数据加载失败')
    invokeMock.mockResolvedValue(summary)
    await view.get('.share-window__retry').trigger('click')
    await flushPromises()
    expect(view.get('.share-window__primary').attributes('disabled')).toBeUndefined()
    expect(view.get('.share-window__preview .share-card__title-row h1').text()).toBe('15,238,536')
  })
})
