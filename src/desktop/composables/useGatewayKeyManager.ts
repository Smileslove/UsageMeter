/**
 * 网关密钥管理 composable：封装上游 Key CRUD 与本地 Key 生成/揭示/复制/撤销。
 * 从 GatewayProfileEditor 抽取，消除编辑器的 5 项职责之一。
 */
import { ref, type Ref } from 'vue'
import {
  createGatewayLocalKey,
  createGatewayUpstreamKey,
  deleteGatewayUpstreamKey,
  listGatewayProfiles,
  revealGatewayLocalKey,
  revokeGatewayLocalKey,
  updateGatewayUpstreamKey,
} from '../../api/gatewayApi'
import type { GatewayProfile, GatewayUpstreamKey } from '../../types'
import type { DraftProfile } from '../views/gateway/gatewayTypes'

type NoticeFn = (kind: 'saved' | 'copied' | 'error', errorCode?: string) => void
type ApplyProfileFn = (profile: GatewayProfile) => void

export function useGatewayKeyManager(
  draft: Ref<DraftProfile>,
  applyProfile: ApplyProfileFn,
  notify: NoticeFn,
) {
  const keyBusy = ref(false)
  const upstreamRemark = ref('')
  const upstreamSecret = ref('')
  const localRemark = ref('')
  const generatedLocalKey = ref('')
  const keyPanel = ref<'upstream' | 'local' | null>(null)
  const revealedLocalKeys = ref<Record<string, string>>({})

  function resetKeyState() {
    upstreamRemark.value = ''
    upstreamSecret.value = ''
    localRemark.value = ''
    generatedLocalKey.value = ''
    revealedLocalKeys.value = {}
    keyPanel.value = null
  }

  function onDraftIdChange(id: string | undefined) {
    upstreamSecret.value = ''
    keyPanel.value = null
    if (!id) {
      upstreamRemark.value = ''
      localRemark.value = ''
      generatedLocalKey.value = ''
      revealedLocalKeys.value = {}
    }
  }

  async function addUpstreamKey() {
    if (!draft.value.id || !upstreamSecret.value.trim()) return
    keyBusy.value = true
    try {
      const profile = await createGatewayUpstreamKey(draft.value.id, {
        remark: upstreamRemark.value.trim(),
        secret: upstreamSecret.value.trim(),
        enabled: true,
        weight: 1,
        priority: 0,
      })
      applyProfile(profile)
      upstreamRemark.value = ''
      upstreamSecret.value = ''
      notify('saved')
    } catch (error) {
      notify('error', String(error))
    } finally {
      keyBusy.value = false
    }
  }

  async function deleteUpstreamKey(keyId: string) {
    if (!draft.value.id) return
    try {
      applyProfile(await deleteGatewayUpstreamKey(draft.value.id, keyId))
    } catch (error) {
      notify('error', String(error))
    }
  }

  async function updateUpstreamKey(key: GatewayUpstreamKey) {
    if (!draft.value.id || !Number.isInteger(key.weight) || key.weight < 1 || key.weight > 65535 || !Number.isInteger(key.priority) || key.priority < 0 || key.priority > 65535) {
      notify('error', 'gateway.operationError')
      return
    }
    keyBusy.value = true
    try {
      applyProfile(await updateGatewayUpstreamKey(draft.value.id, key.id, {
        enabled: key.enabled, weight: key.weight, priority: key.priority,
      }))
      notify('saved')
    } catch (error) {
      notify('error', String(error))
    } finally {
      keyBusy.value = false
    }
  }

  async function revokeLocalKey(keyId: string) {
    if (!draft.value.id) return
    try {
      applyProfile(await revokeGatewayLocalKey(draft.value.id, keyId))
    } catch (error) {
      notify('error', String(error))
    }
  }

  async function createReplacementLocalKey() {
    if (!draft.value.id) return
    keyBusy.value = true
    try {
      const generated = await createGatewayLocalKey(draft.value.id, { remark: localRemark.value.trim() })
      generatedLocalKey.value = generated.key
      const updated = (await listGatewayProfiles()).find(profile => profile.id === draft.value.id)
      if (updated) applyProfile(updated)
      localRemark.value = ''
      notify('saved')
    } catch (error) {
      notify('error', String(error))
    } finally {
      keyBusy.value = false
    }
  }

  async function revealLocalKey(keyId: string) {
    if (!draft.value.id) return
    try {
      revealedLocalKeys.value[keyId] = await revealGatewayLocalKey(draft.value.id, keyId)
    } catch (error) {
      notify('error', String(error))
    }
  }

  async function copyStoredLocalKey(keyId: string) {
    await revealLocalKey(keyId)
    const key = revealedLocalKeys.value[keyId]
    if (!key) return
    try {
      await navigator.clipboard.writeText(key)
      notify('copied')
    } catch {
      notify('error', 'gateway.operationError')
    }
  }

  return {
    keyBusy,
    upstreamRemark,
    upstreamSecret,
    localRemark,
    generatedLocalKey,
    keyPanel,
    revealedLocalKeys,
    resetKeyState,
    onDraftIdChange,
    addUpstreamKey,
    deleteUpstreamKey,
    updateUpstreamKey,
    revokeLocalKey,
    createReplacementLocalKey,
    revealLocalKey,
    copyStoredLocalKey,
  }
}
