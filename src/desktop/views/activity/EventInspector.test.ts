// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { flushPromises, mount } from '@vue/test-utils'
import type { RedactedPayloadPage, SessionEventListItem } from '../../../types'

const { getSessionEventPayloadMock } = vi.hoisted(() => ({
  getSessionEventPayloadMock: vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('../../../api/activityApi', () => ({
  getSessionEventPayload: getSessionEventPayloadMock
}))

import EventInspector from './EventInspector.vue'

function event(eventKey: string, tool = false): SessionEventListItem {
  return {
    eventKey,
    sessionKey: 'session-1',
    sequence: 1,
    timestampMs: 1,
    kind: tool ? 'toolInvocation' : 'userMessage',
    status: 'success',
    actorAgentKey: null,
    parentEventKey: null,
    summary: null,
    contentState: 'available',
    tool: tool ? {
      invocationKey: `invocation-${eventKey}`,
      rawName: 'Read',
      normalizedName: 'Read',
      family: 'filesystem',
      durationMs: 1,
      inputBytes: 1,
      outputBytes: 1,
      inputKeys: [],
      resultKind: null,
    } : null,
    requestLinks: [],
    sourceRef: { sourceFileId: null, sourceFilePath: '~/.test', sourceOffset: null, fingerprint: null },
  }
}

function page(content: string, nextCursor: string | null = null): RedactedPayloadPage {
  return { content, nextCursor, truncated: nextCursor !== null, contentState: 'available' }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>(done => { resolve = done })
  return { promise, resolve }
}

describe('EventInspector payload races', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getSessionEventPayloadMock.mockReset()
  })

  it('does not let a previous event response overwrite the selected event', async () => {
    const first = deferred<RedactedPayloadPage>()
    const second = deferred<RedactedPayloadPage>()
    getSessionEventPayloadMock
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const wrapper = mount(EventInspector, { props: { event: event('old'), wide: true } })

    await wrapper.setProps({ event: event('new') })
    second.resolve(page('new payload'))
    await flushPromises()
    first.resolve(page('old payload'))
    await flushPromises()

    expect(wrapper.text()).toContain('new payload')
    expect(wrapper.text()).not.toContain('old payload')
  })

  it('does not append an old page after switching payload sections', async () => {
    const loadMore = deferred<RedactedPayloadPage>()
    const output = deferred<RedactedPayloadPage>()
    getSessionEventPayloadMock
      .mockResolvedValueOnce(page('input page 1', 'cursor-2'))
      .mockReturnValueOnce(loadMore.promise)
      .mockReturnValueOnce(output.promise)
    const wrapper = mount(EventInspector, { props: { event: event('tool', true), wide: true } })
    await flushPromises()

    const loadMoreButton = wrapper.findAll('button').find(button => button.text().includes('加载更多'))
    await loadMoreButton!.trigger('click')
    const outputButton = wrapper.findAll('button').find(button => button.text() === '输出')
    await outputButton!.trigger('click')
    output.resolve(page('output payload'))
    await flushPromises()
    loadMore.resolve(page('stale input page 2'))
    await flushPromises()

    expect(wrapper.text()).toContain('output payload')
    expect(wrapper.text()).not.toContain('stale input page 2')
  })
})
