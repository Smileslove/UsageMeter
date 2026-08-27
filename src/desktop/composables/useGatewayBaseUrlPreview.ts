/**
 * 网关 Base URL 预览 composable：防抖拉取预览结果，竞态安全。
 * 从 GatewayProfileEditor 抽取。
 */
import { ref, watch, type Ref } from 'vue'
import { previewGatewayBaseUrl } from '../../api/gatewayApi'
import type { GatewayBaseUrlPreview } from '../../types'
import type { DraftProfile } from '../views/gateway/gatewayTypes'

export function useGatewayBaseUrlPreview(draft: Ref<DraftProfile>) {
  const baseUrlPreview = ref<GatewayBaseUrlPreview | null>(null)
  let previewTimer: ReturnType<typeof setTimeout> | undefined
  let previewSeq = 0

  async function updateBaseUrlPreview() {
    const raw = draft.value.baseUrl.trim()
    const seq = ++previewSeq
    if (!raw) {
      if (seq === previewSeq) baseUrlPreview.value = null
      return
    }
    try {
      new URL(raw)
    } catch {
      if (seq === previewSeq) baseUrlPreview.value = null
      return
    }
    try {
      const preview = await previewGatewayBaseUrl(draft.value.protocol, raw)
      if (seq === previewSeq) baseUrlPreview.value = preview
    } catch {
      if (seq === previewSeq) baseUrlPreview.value = null
    }
  }

  watch(
    [() => draft.value.baseUrl, () => draft.value.protocol],
    () => {
      if (previewTimer) clearTimeout(previewTimer)
      previewTimer = setTimeout(updateBaseUrlPreview, 250)
    },
  )

  function clearBaseUrlPreview() {
    baseUrlPreview.value = null
    if (previewTimer) clearTimeout(previewTimer)
    previewTimer = undefined
    previewSeq++
  }

  function dispose() {
    if (previewTimer) clearTimeout(previewTimer)
  }

  return { baseUrlPreview, clearBaseUrlPreview, dispose }
}
