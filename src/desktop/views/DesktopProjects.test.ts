// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { flushPromises, shallowMount } from '@vue/test-utils'
import { useMonitorStore } from '../../stores/monitor'
import type { ProjectStats, SessionStats } from '../../types'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn().mockResolvedValue(() => {}) }))
vi.mock('../../composables/useSessionViewData', async () => {
  const { ref } = await import('vue')
  return {
    useSessionViewData: () => ({
      selectedTool: ref(null),
      reloadProjectStats: vi.fn(),
      initialize: vi.fn(),
      dispose: vi.fn(),
    }),
  }
})

import DesktopProjects from './DesktopProjects.vue'

function session(fields: Partial<SessionStats>): SessionStats {
  return {
    sessionId: '', tool: 'codex', models: [],
    totalRequests: 0, totalInputTokens: 0, totalOutputTokens: 0,
    totalCacheCreateTokens: 0, totalCacheReadTokens: 0, totalDurationMs: 0,
    avgOutputTokensPerSecond: 0, firstRequestTime: 0, lastRequestTime: 0,
    ...fields,
  }
}

describe('DesktopProjects project membership', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('includes worktrees by project key and excludes another repository with the same name', async () => {
    const store = useMonitorStore()
    store.sessions = [
      session({ sessionId: 'main', projectName: 'repo', projectKey: '/code/repo', cwd: '/code/repo', topic: 'Main session' }),
      session({ sessionId: 'worktree', projectName: 'repo', projectKey: '/code/repo', cwd: '/agent/worktrees/feature', topic: 'Worktree session' }),
      session({ sessionId: 'other', projectName: 'repo', projectKey: '/other/repo', cwd: '/other/repo', topic: 'Other repository session' }),
    ]
    const wrapper = shallowMount(DesktopProjects)
    store.projectStats = [{ name: 'repo', projectKey: '/code/repo', projectPath: '/code/repo', projectIdentity: 'project' }] as ProjectStats[]
    await flushPromises()
    expect(wrapper.text()).toContain('Main session')
    expect(wrapper.text()).toContain('Worktree session')
    expect(wrapper.text()).not.toContain('Other repository session')
    wrapper.unmount()
  })

  it('uses cwd for older session payloads without merging same-name repositories', async () => {
    const store = useMonitorStore()
    store.sessions = [
      session({ sessionId: 'legacy', projectName: 'repo', cwd: '/code/repo', topic: 'Legacy session' }),
      session({ sessionId: 'other', projectName: 'repo', cwd: '/other/repo', topic: 'Other legacy session' }),
    ]
    const wrapper = shallowMount(DesktopProjects)
    store.projectStats = [{ name: 'repo', projectPath: '/code/repo', projectIdentity: 'project' }] as ProjectStats[]
    await flushPromises()
    expect(wrapper.text()).toContain('Legacy session')
    expect(wrapper.text()).not.toContain('Other legacy session')
    wrapper.unmount()
  })
})
