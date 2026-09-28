// @vitest-environment happy-dom
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import { describe, expect, it, vi } from 'vitest'
import SegmentedControl from './SegmentedControl.vue'

describe('SegmentedControl', () => {
  it('measures the selected button position and shape', async () => {
    const wrapper = mount(SegmentedControl, {
      props: { activeIndex: 1 },
      slots: {
        default: '<button style="border-radius: 6px">One</button><button style="border-radius: 14px">Longer</button>'
      }
    })
    const root = wrapper.element as HTMLElement
    const [first, second] = [...root.querySelectorAll('button')]
    const indicator = root.querySelector('.segmented-control__indicator') as HTMLElement

    vi.spyOn(root, 'getBoundingClientRect').mockReturnValue({ left: 20, top: 30, width: 200, height: 80 } as DOMRect)
    vi.spyOn(root, 'offsetWidth', 'get').mockReturnValue(100)
    vi.spyOn(root, 'offsetHeight', 'get').mockReturnValue(40)
    vi.spyOn(root, 'clientLeft', 'get').mockReturnValue(2)
    vi.spyOn(root, 'clientTop', 'get').mockReturnValue(1)
    vi.spyOn(first, 'getBoundingClientRect').mockReturnValue({ left: 24, top: 34, width: 36, height: 22 } as DOMRect)
    vi.spyOn(second, 'getBoundingClientRect').mockReturnValue({ left: 62, top: 34, width: 58, height: 22 } as DOMRect)
    vi.spyOn(window, 'getComputedStyle').mockReturnValue({ borderRadius: '14px' } as CSSStyleDeclaration)
    await nextTick()
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))

    expect(indicator.style.width).toBe('29px')
    expect(indicator.style.height).toBe('11px')
    expect(indicator.style.transform).toBe('translate(19px, 1px)')
    expect(indicator.style.borderRadius).toBe('14px')
    expect(root.dataset.ready).toBe('true')
    wrapper.unmount()
  })
})
