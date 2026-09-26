// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { flushPromises, shallowMount } from '@vue/test-utils'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue('') }))

import { useMonitorStore } from '../../stores/monitor'
import DesktopSettings from './DesktopSettings.vue'

describe('DesktopSettings privacy level', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('rolls the UI and store back when saving the privacy level fails', async () => {
    const store = useMonitorStore()
    store.settings.deepIndexLevel = 'off'
    store.saveSettings = vi.fn().mockRejectedValue(new Error('save failed'))
    const wrapper = shallowMount(DesktopSettings)

    const privacyNav = wrapper.findAll('nav button').find(button => button.text() === '隐私')
    await privacyNav!.trigger('click')
    const fulltextRadio = wrapper.find('input[aria-label="全文索引"]')
    await fulltextRadio.trigger('change')
    await flushPromises()

    expect(store.settings.deepIndexLevel).toBe('off')
    expect((wrapper.find('input[aria-label="关闭"]').element as HTMLInputElement).checked).toBe(true)
    expect((fulltextRadio.element as HTMLInputElement).checked).toBe(false)
  })
})
