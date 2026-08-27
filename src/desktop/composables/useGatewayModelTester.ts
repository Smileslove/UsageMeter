/**
 * 网关模型连通性测试 composable：封装模型列表拉取、持久化、单模型测试。
 * 从 GatewayProfileEditor 抽取。
 */
import { computed, ref, type Ref } from 'vue'
import {
  listGatewayUpstreamModels,
  saveGatewayUpstreamModels,
  testGatewayUpstreamModel,
} from '../../api/gatewayApi'
import { t } from '../../i18n'
import type { GatewayModelTestResult, GatewayUpstreamModelsResult } from '../../types'
import type { DraftProfile } from '../views/gateway/gatewayTypes'

type NoticeFn = (kind: 'saved' | 'copied' | 'error', errorCode?: string) => void
type UpdateDraftFn = (partial: Partial<DraftProfile>) => void

export function useGatewayModelTester(
  draft: Ref<DraftProfile>,
  locale: Ref<string>,
  _notify: NoticeFn,
  updateDraft: UpdateDraftFn,
) {
  const modelsLoading = ref(false)
  const modelsResult = ref<GatewayUpstreamModelsResult | null>(null)
  const modelsPersisted = ref(false)
  const selectedModelId = ref('')
  const testingModel = ref(false)
  const testResult = ref<GatewayModelTestResult | null>(null)

  const testErrorMessage = computed(() => {
    if (!testResult.value || testResult.value.ok) return ''
    const kind = testResult.value.errorKind
    if (!kind) return t(locale.value, 'gateway.testFailed')
    const camelKind = kind.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase())
    return t(locale.value, `gateway.modelError.${camelKind}`, { status: testResult.value.httpStatus ?? '' })
  })

  function onDraftIdChange(id: string | undefined) {
    testResult.value = null
    modelsLoading.value = false
    testingModel.value = false
    if (id) {
      modelsResult.value = draft.value.upstreamModels.length > 0
        ? { ok: true, models: draft.value.upstreamModels }
        : null
      modelsPersisted.value = draft.value.upstreamModels.length > 0
      selectedModelId.value = draft.value.upstreamModels[0]?.id ?? ''
    } else {
      modelsResult.value = null
      modelsPersisted.value = false
      selectedModelId.value = ''
    }
  }

  function resetModelState() {
    modelsResult.value = null
    modelsPersisted.value = false
    selectedModelId.value = ''
    testResult.value = null
    modelsLoading.value = false
    testingModel.value = false
  }

  async function refreshModels() {
    if (!draft.value.id || modelsLoading.value) return
    const profileId = draft.value.id
    modelsLoading.value = true
    testResult.value = null
    try {
      const result = await listGatewayUpstreamModels(profileId)
      if (draft.value.id !== profileId) return
      modelsResult.value = result
      selectedModelId.value = result.models[0]?.id ?? ''
      if (result.ok) {
        if (result.models.length > 0) {
          try {
            const saved = await saveGatewayUpstreamModels(profileId, result.models)
            if (draft.value.id !== profileId) return
            modelsPersisted.value = true
            updateDraft({ upstreamModels: saved.upstreamModels })
          } catch {
            modelsPersisted.value = false
          }
        } else {
          modelsPersisted.value = false
        }
      } else {
        modelsPersisted.value = false
      }
    } catch (error) {
      if (draft.value.id !== profileId) return
      modelsResult.value = { ok: false, models: [], errorKind: 'transport_error', errorDetail: String(error) }
      selectedModelId.value = ''
      modelsPersisted.value = false
    } finally {
      if (draft.value.id === profileId) modelsLoading.value = false
    }
  }

  async function testSelectedModel() {
    if (!draft.value.id || !selectedModelId.value || testingModel.value) return
    const profileId = draft.value.id
    const modelId = selectedModelId.value
    testingModel.value = true
    try {
      testResult.value = await testGatewayUpstreamModel(profileId, modelId)
    } catch (error) {
      if (draft.value.id !== profileId) return
      testResult.value = { ok: false, modelId, errorKind: 'transport_error', errorDetail: String(error) }
    } finally {
      if (draft.value.id === profileId) testingModel.value = false
    }
  }

  return {
    modelsLoading,
    modelsResult,
    modelsPersisted,
    selectedModelId,
    testingModel,
    testResult,
    testErrorMessage,
    onDraftIdChange,
    resetModelState,
    refreshModels,
    testSelectedModel,
  }
}
