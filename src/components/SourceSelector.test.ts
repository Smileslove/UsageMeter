// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import SourceSelector from './SourceSelector.vue'
import { DEEPSEEK_HARNESS_ACCOUNT_SOURCE_ID } from '../types'
import { t } from '../i18n'

const store = vi.hoisted(() => ({
  settings: {
    locale: 'zh-CN',
    clientTools: { profiles: [{ tool: 'deepseek_harness', enabled: true }] },
    sourceAware: { sources: [], activeSourceFilter: null as string | null },
  },
  hasChatGptOAuth: false,
  hasGeminiOAuth: false,
  hasClaudeOAuth: false,
  setActiveSourceFilter: vi.fn(async () => {}),
  refreshSources: vi.fn(async () => {}),
}))
vi.mock('../stores/monitor', () => ({ useMonitorStore: () => store }))

afterEach(() => vi.clearAllMocks())

describe('Harness account source selection', () => {
  it.each(['zh-CN', 'zh-TW', 'en-US'])('selects and renders the account source in %s', async (locale) => {
    store.settings.locale = locale
    store.settings.sourceAware.activeSourceFilter = null
    const wrapper = mount(SourceSelector, { global: { stubs: { LobeIcon: true } } })
    await wrapper.find('button').trigger('click')
    expect(store.refreshSources).toHaveBeenCalledOnce()
    const account = wrapper.findAll('button').find(button => button.text() === t(locale, 'sources.deepseekHarnessAccount'))
    expect(account).toBeDefined()
    await account!.trigger('click')
    expect(store.setActiveSourceFilter).toHaveBeenCalledWith(DEEPSEEK_HARNESS_ACCOUNT_SOURCE_ID)
    wrapper.unmount()
    store.settings.sourceAware.activeSourceFilter = DEEPSEEK_HARNESS_ACCOUNT_SOURCE_ID
    const selected = mount(SourceSelector, { global: { stubs: { LobeIcon: true } } })
    expect(selected.find('button').attributes('title')).toBe(t(locale, 'sources.deepseekHarnessAccount'))
    selected.unmount()
  })
})
