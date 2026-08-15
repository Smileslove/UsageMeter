<script setup lang="ts">
/**
 * 桌面网关页右侧「详情 / 编辑表单」（内嵌，非 modal）：
 * 基本信息（名称/协议/地址/启用）、密钥管理（上游 Key 增删改、本地 Key 生成/查看/复制/撤销）、
 * 模型连通性测试。draft 由父组件持有（props 传入，表单直接编辑其字段）；
 * 提交、取消、密钥 CRUD、复制地址等副作用一律通过事件上抛由父组件编排
 * （profiles 更新、feedback/errorCode、status 刷新、confirm 等）。
 */
import { computed, onUnmounted, ref, watch } from 'vue'
import { Check, Copy, Eye, KeyRound, Plus, RefreshCw, Save, Trash2, X } from 'lucide-vue-next'
import {
  createGatewayLocalKey,
  createGatewayUpstreamKey,
  deleteGatewayUpstreamKey,
  listGatewayProfiles,
  listGatewayUpstreamModels,
  previewGatewayBaseUrl,
  revealGatewayLocalKey,
  revokeGatewayLocalKey,
  saveGatewayUpstreamModels,
  testGatewayUpstreamModel,
  updateGatewayUpstreamKey
} from '../../../api/gatewayApi'
import { useMonitorStore } from '../../../stores/monitor'
import { t } from '../../../i18n'
import { gatewayProtocolOptions } from './protocolOptions'
import type { GatewayBaseUrlPreview, GatewayCredentialRecovery, GatewayDispatchStrategy, GatewayLocalKey, GatewayModelTestResult, GatewayProfile, GatewayProtocol, GatewayUpstreamKey, GatewayUpstreamModel, GatewayUpstreamModelsResult } from '../../../types'

interface DraftProfile {
  id?: string
  name: string
  protocol: GatewayProtocol
  baseUrl: string
  enabled: boolean
  clientLabel: string
  dispatchStrategy: GatewayDispatchStrategy
  upstreamKeys: GatewayUpstreamKey[]
  localKeys: GatewayLocalKey[]
  upstreamModels: GatewayUpstreamModel[]
  credentialRecovery: GatewayCredentialRecovery
}

interface SubmitPayload {
  name: string
  protocol: GatewayProtocol
  baseUrl: string
  enabled: boolean
  clientLabel: string
  dispatchStrategy: GatewayDispatchStrategy
  upstreamSecret: string
}

const props = defineProps<{
  draft: DraftProfile
  selectedAddress: string
  selectedProtocolLabel: string
  saving: boolean
  feedback: 'copied' | 'saved' | 'error' | null
  feedbackMessage: string
  resetKey: number
}>()

const emit = defineEmits<{
  (e: 'submit', payload: SubmitPayload): void
  (e: 'cancel'): void
  (e: 'applyProfile', profile: GatewayProfile): void
  (e: 'updateDraft', partial: Partial<DraftProfile>): void
  (e: 'notice', payload: { kind: 'saved' | 'copied' | 'error'; errorCode?: string }): void
  (e: 'copyAddress'): void
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

const keyBusy = ref(false)
const upstreamRemark = ref('')
const upstreamSecret = ref('')
const localRemark = ref('')
const generatedLocalKey = ref('')
const keyPanel = ref<'upstream' | 'local' | null>(null)
const revealedLocalKeys = ref<Record<string, string>>({})
const modelsLoading = ref(false)
const modelsResult = ref<GatewayUpstreamModelsResult | null>(null)
const modelsPersisted = ref(false)
const selectedModelId = ref('')
const testingModel = ref(false)
const testResult = ref<GatewayModelTestResult | null>(null)
const baseUrlPreview = ref<GatewayBaseUrlPreview | null>(null)
let previewTimer: ReturnType<typeof setTimeout> | undefined
let previewSeq = 0

const upstreamKeyRecoveryRequired = computed(() => props.draft.credentialRecovery.upstreamKeyRequired)
const localKeyRotationRecommended = computed(() => props.draft.credentialRecovery.localKeyRotationRecommended)

const testErrorMessage = computed(() => {
  if (!testResult.value || testResult.value.ok) return ''
  const kind = testResult.value.errorKind
  if (!kind) return t(locale.value, 'gateway.testFailed')
  const camelKind = kind.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase())
  return t(locale.value, `gateway.modelError.${camelKind}`, { status: testResult.value.httpStatus ?? '' })
})

async function updateBaseUrlPreview() {
  const raw = props.draft.baseUrl.trim()
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
    const preview = await previewGatewayBaseUrl(props.draft.protocol, raw)
    if (seq === previewSeq) baseUrlPreview.value = preview
  } catch {
    if (seq === previewSeq) baseUrlPreview.value = null
  }
}

watch(
  [() => props.draft.baseUrl, () => props.draft.protocol],
  () => {
    if (previewTimer) clearTimeout(previewTimer)
    previewTimer = setTimeout(updateBaseUrlPreview, 250)
  }
)

function clearBaseUrlPreview() {
  baseUrlPreview.value = null
  if (previewTimer) clearTimeout(previewTimer)
  previewTimer = undefined
  previewSeq++
}

// 全部表单内部状态重置（等价于原 resetDraft）
function resetFormState() {
  upstreamRemark.value = ''
  upstreamSecret.value = ''
  localRemark.value = ''
  generatedLocalKey.value = ''
  revealedLocalKeys.value = {}
  keyPanel.value = null
  modelsResult.value = null
  modelsPersisted.value = false
  selectedModelId.value = ''
  testResult.value = null
  modelsLoading.value = false
  testingModel.value = false
  clearBaseUrlPreview()
}

// draft.id 变化 → 选中语义（等价于原 selectProfile）或 取消/新建语义（等价于原 resetDraft）
watch(
  () => props.draft.id,
  id => {
    upstreamSecret.value = ''
    keyPanel.value = null
    testResult.value = null
    modelsLoading.value = false
    testingModel.value = false
    clearBaseUrlPreview()
    if (id) {
      modelsResult.value = props.draft.upstreamModels.length > 0 ? { ok: true, models: props.draft.upstreamModels } : null
      modelsPersisted.value = props.draft.upstreamModels.length > 0
      selectedModelId.value = props.draft.upstreamModels[0]?.id ?? ''
    } else {
      upstreamRemark.value = ''
      localRemark.value = ''
      generatedLocalKey.value = ''
      revealedLocalKeys.value = {}
      modelsResult.value = null
      modelsPersisted.value = false
      selectedModelId.value = ''
    }
  }
)

// resetKey 变化（父组件 startNewProfile / cancelEditing）→ 全量重置兜底
watch(
  () => props.resetKey,
  () => resetFormState()
)

function notify(kind: 'saved' | 'copied' | 'error', errorCode?: string) {
  emit('notice', { kind, errorCode })
}

function onSubmit() {
  emit('submit', {
    name: props.draft.name,
    protocol: props.draft.protocol,
    baseUrl: props.draft.baseUrl,
    enabled: props.draft.enabled,
    clientLabel: props.draft.clientLabel,
    dispatchStrategy: props.draft.dispatchStrategy,
    upstreamSecret: upstreamSecret.value
  })
}

async function addUpstreamKey() {
  if (!props.draft.id || !upstreamSecret.value.trim()) return
  keyBusy.value = true
  try {
    const profile = await createGatewayUpstreamKey(props.draft.id, {
      remark: upstreamRemark.value.trim(), secret: upstreamSecret.value.trim(), enabled: true, weight: 1, priority: 0
    })
    emit('applyProfile', profile)
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
  if (!props.draft.id) return
  try {
    emit('applyProfile', await deleteGatewayUpstreamKey(props.draft.id, keyId))
  } catch (error) {
    notify('error', String(error))
  }
}

async function updateUpstreamKey(key: GatewayUpstreamKey) {
  if (!props.draft.id || !Number.isInteger(key.weight) || key.weight < 1 || key.weight > 65535 || !Number.isInteger(key.priority) || key.priority < 0 || key.priority > 65535) {
    notify('error', 'gateway.operationError')
    return
  }
  keyBusy.value = true
  try {
    emit('applyProfile', await updateGatewayUpstreamKey(props.draft.id, key.id, {
      enabled: key.enabled, weight: key.weight, priority: key.priority
    }))
    notify('saved')
  } catch (error) {
    notify('error', String(error))
  } finally {
    keyBusy.value = false
  }
}

async function revokeLocalKey(keyId: string) {
  if (!props.draft.id) return
  try {
    emit('applyProfile', await revokeGatewayLocalKey(props.draft.id, keyId))
  } catch (error) {
    notify('error', String(error))
  }
}

async function createReplacementLocalKey() {
  if (!props.draft.id) return
  keyBusy.value = true
  try {
    const generated = await createGatewayLocalKey(props.draft.id, { remark: localRemark.value.trim() })
    generatedLocalKey.value = generated.key
    const updated = (await listGatewayProfiles()).find(profile => profile.id === props.draft.id)
    if (updated) emit('applyProfile', updated)
    localRemark.value = ''
    notify('saved')
  } catch (error) {
    notify('error', String(error))
  } finally {
    keyBusy.value = false
  }
}

async function revealLocalKey(keyId: string) {
  if (!props.draft.id) return
  try {
    revealedLocalKeys.value[keyId] = await revealGatewayLocalKey(props.draft.id, keyId)
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

async function refreshModels() {
  if (!props.draft.id || modelsLoading.value) return
  const profileId = props.draft.id
  modelsLoading.value = true
  testResult.value = null
  try {
    const result = await listGatewayUpstreamModels(profileId)
    // 用户可能已切换 profile，过期响应不得写入错误表单
    if (props.draft.id !== profileId) return
    modelsResult.value = result
    selectedModelId.value = result.models[0]?.id ?? ''
    if (result.ok) {
      if (result.models.length > 0) {
        try {
          const saved = await saveGatewayUpstreamModels(profileId, result.models)
          if (props.draft.id !== profileId) return
          modelsPersisted.value = true
          emit('updateDraft', { upstreamModels: saved.upstreamModels })
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
    if (props.draft.id !== profileId) return
    modelsResult.value = { ok: false, models: [], errorKind: 'transport_error', errorDetail: String(error) }
    selectedModelId.value = ''
    modelsPersisted.value = false
  } finally {
    if (props.draft.id === profileId) modelsLoading.value = false
  }
}

async function testSelectedModel() {
  if (!props.draft.id || !selectedModelId.value || testingModel.value) return
  const profileId = props.draft.id
  const modelId = selectedModelId.value
  testingModel.value = true
  try {
    testResult.value = await testGatewayUpstreamModel(profileId, modelId)
  } catch (error) {
    if (props.draft.id !== profileId) return
    testResult.value = { ok: false, modelId, errorKind: 'transport_error', errorDetail: String(error) }
  } finally {
    if (props.draft.id === profileId) testingModel.value = false
  }
}

onUnmounted(() => {
  if (previewTimer) clearTimeout(previewTimer)
})
</script>

<template>
  <form class="theme-surface overflow-hidden rounded-xl border" role="dialog" aria-modal="false" @submit.prevent="onSubmit">
    <!-- 表单头部 -->
    <header class="flex items-start justify-between border-b border-[var(--theme-border-default)] px-4 py-3">
      <div class="min-w-0">
        <h2 class="text-[14px] font-bold text-[var(--theme-text-primary)]">{{ draft.id ? t(locale, 'gateway.editProfile') : t(locale, 'gateway.newProfile') }}</h2>
        <p class="mt-0.5 truncate text-[10px] text-[var(--theme-text-tertiary)]">{{ selectedProtocolLabel }}</p>
      </div>
      <button type="button" class="theme-icon-button -mr-1 -mt-1 rounded-lg p-2" :title="t(locale, 'gateway.cancel')" :aria-label="t(locale, 'gateway.cancel')" @click="emit('cancel')"><X class="h-4 w-4" aria-hidden="true" /></button>
    </header>

    <div class="space-y-2.5 p-4">
      <!-- 基本信息 -->
      <section v-if="!keyPanel" class="theme-surface-muted overflow-hidden rounded-xl border">
        <label class="flex h-11 items-center gap-3 px-3">
          <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.name') }}</span>
          <input v-model="draft.name" class="min-w-0 flex-1 bg-transparent text-right text-[12px] font-semibold text-[var(--theme-text-primary)] outline-none placeholder:font-normal placeholder:text-[var(--theme-text-quaternary)]" :placeholder="t(locale, 'gateway.namePlaceholder')" />
        </label>
        <label class="flex h-11 items-center gap-3 border-t border-[var(--theme-border-default)] px-3">
          <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.protocol') }}</span>
          <select v-model="draft.protocol" class="min-w-0 flex-1 appearance-none bg-transparent py-0.5 text-right text-[11.5px] font-semibold text-[var(--theme-text-primary)] outline-none">
            <option v-for="option in gatewayProtocolOptions" :key="option.value" :value="option.value">{{ t(locale, option.labelKey) }}</option>
          </select>
        </label>
        <label class="flex h-11 items-center gap-3 border-t border-[var(--theme-border-default)] px-3">
          <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.baseUrl') }}</span>
          <input v-model="draft.baseUrl" class="min-w-0 flex-1 bg-transparent text-right font-mono text-[10.5px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]" :placeholder="t(locale, 'gateway.baseUrlPlaceholder')" inputmode="url" />
        </label>
        <div v-if="baseUrlPreview" class="border-t border-[var(--theme-border-default)] px-3 py-2">
          <p class="break-all font-mono text-[10px] leading-relaxed text-[var(--theme-text-primary)]">POST {{ baseUrlPreview.sampleRequestUrl }}</p>
          <p v-if="!baseUrlPreview.basePathAdded" class="mt-1 text-right text-[9px] leading-none text-emerald-600/90 dark:text-emerald-400/90">
            {{ t(locale, 'gateway.basePathAlreadyIncluded', { path: baseUrlPreview.basePath }) }}
          </p>
        </div>
        <label class="flex h-11 items-center gap-3 border-t border-[var(--theme-border-default)] px-3">
          <span class="w-[66px] shrink-0 text-[10px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.upstreamKey') }}</span>
          <!-- 上游 key 密码框：创建后不再回显 -->
          <input v-model="upstreamSecret" type="password" class="min-w-0 flex-1 bg-transparent text-right font-mono text-[10.5px] text-[var(--theme-text-primary)] outline-none placeholder:text-[var(--theme-text-quaternary)]" :placeholder="upstreamKeyRecoveryRequired ? t(locale, 'gateway.upstreamKeyRecoveryPlaceholder') : (draft.id ? t(locale, 'gateway.upstreamKeyKeepHint') : t(locale, 'gateway.upstreamKeyPlaceholder'))" autocomplete="off" />
        </label>
        <label class="flex h-11 cursor-pointer items-center justify-between border-t border-[var(--theme-border-default)] px-3">
          <span class="text-[10.5px] font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.enabled') }}</span>
          <input v-model="draft.enabled" type="checkbox" class="peer sr-only" />
          <span class="relative h-5 w-9 rounded-full bg-[var(--theme-border-strong)] transition-colors peer-checked:bg-[var(--theme-accent-primary)] after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:shadow-sm after:transition-transform peer-checked:after:translate-x-4 peer-focus-visible:ring-2 peer-focus-visible:ring-[var(--theme-accent-primary)] peer-focus-visible:ring-offset-2"></span>
        </label>
      </section>

      <!-- 凭据 / 地址 / 模型测试 -->
      <template v-if="!keyPanel">
        <div v-if="upstreamKeyRecoveryRequired || localKeyRotationRecommended" class="rounded-xl border border-amber-500/25 bg-amber-500/8 px-3 py-2 text-[9.5px] leading-snug text-amber-700 dark:text-amber-200">
          <p v-if="upstreamKeyRecoveryRequired">{{ t(locale, 'gateway.upstreamKeyRecoveryNotice') }}</p>
          <p v-if="localKeyRotationRecommended" :class="{ 'mt-1': upstreamKeyRecoveryRequired }">{{ t(locale, 'gateway.localKeyRecoveryNotice') }}</p>
        </div>
        <p class="px-1 text-[9px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.managedKeyHint') }}</p>
        <div v-if="draft.id" class="theme-surface-muted rounded-xl border px-3 py-2.5">
          <div class="flex items-center justify-between gap-2"><span class="text-[9.5px] font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.address') }}</span><div class="flex items-center gap-1"><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.localKeys')" :aria-label="t(locale, 'gateway.localKeys')" @click="keyPanel = 'local'"><KeyRound class="h-3.5 w-3.5" aria-hidden="true" /></button><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click="emit('copyAddress')"><Copy class="h-3.5 w-3.5" aria-hidden="true" /></button></div></div>
          <p class="mt-1 break-all font-mono text-[9px] leading-snug text-[var(--theme-text-primary)]">{{ selectedAddress }}</p>
        </div>
        <div v-if="draft.id" class="theme-surface-muted rounded-xl border px-3 py-2.5">
          <div class="flex items-center justify-between gap-2"><span class="text-[9.5px] font-semibold text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelTest') }}</span><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.refreshModels')" :aria-label="t(locale, 'gateway.refreshModels')" :disabled="modelsLoading" @click="refreshModels"><RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': modelsLoading }" aria-hidden="true" /></button></div>
          <p v-if="modelsLoading" class="mt-1 text-[9px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsLoading') }}</p>
          <p v-else-if="modelsResult && !modelsResult.ok" class="mt-1 text-[9px] text-red-500">{{ t(locale, 'gateway.modelsFailed') }}</p>
          <p v-else-if="modelsResult && modelsResult.ok && modelsPersisted && modelsResult.models.length > 0" class="mt-1 text-[9px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsSaved', { count: modelsResult.models.length }) }}</p>
          <p v-else-if="modelsResult && modelsResult.ok && !modelsPersisted && modelsResult.models.length > 0" class="mt-1 text-[9px] text-amber-500">{{ t(locale, 'gateway.modelsSaveFailed') }}</p>
          <p v-else-if="modelsResult && modelsResult.ok && modelsResult.models.length === 0" class="mt-1 text-[9px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsEmpty') }}</p>
          <div v-if="modelsResult && modelsResult.models.length > 0" class="mt-1.5 max-h-[120px] space-y-1 overflow-y-auto">
            <label v-for="model in modelsResult.models" :key="model.id" class="flex cursor-pointer items-center gap-2 rounded-lg border px-2 py-1.5 text-[10px]" :class="selectedModelId === model.id ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-primary)]/5' : 'border-[var(--theme-border-default)]'"><input v-model="selectedModelId" :value="model.id" type="radio" class="sr-only" /><span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="selectedModelId === model.id ? 'bg-[var(--theme-accent-primary)]' : 'bg-[var(--theme-border-strong)]'"></span><span class="min-w-0 flex-1 truncate font-mono text-[var(--theme-text-secondary)]">{{ model.id }}</span></label>
          </div>
          <div v-if="modelsResult && modelsResult.models.length > 0" class="mt-2 flex items-center gap-2"><button type="button" class="rounded-lg border border-[var(--theme-border-default)] px-2.5 py-1 text-[10px] font-semibold text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)] disabled:opacity-50" :disabled="testingModel || !selectedModelId" @click="testSelectedModel">{{ testingModel ? t(locale, 'gateway.testingModel') : t(locale, 'gateway.testModel') }}</button><span v-if="testResult && testResult.ok" class="text-[9.5px] text-emerald-600 dark:text-emerald-300">{{ t(locale, 'gateway.testSuccess', { ms: testResult.latencyMs ?? 0 }) }}</span><span v-else-if="testResult && !testResult.ok" class="text-[9.5px] text-red-500">{{ testErrorMessage }}</span></div>
        </div>
        <p class="px-1 text-[9px] leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.singleUpstreamKeyHint') }}</p>
      </template>

      <!-- Key 子面板（上游 / 本地） -->
      <template v-else>
        <button type="button" class="inline-flex items-center gap-1 text-[10px] font-semibold text-[var(--theme-text-secondary)]" @click="keyPanel = null"><span class="text-[15px] leading-none">‹</span>{{ draft.name }}</button>
        <section class="theme-surface-muted overflow-hidden rounded-xl border">
          <div class="border-b border-[var(--theme-border-default)] px-3 py-2.5"><h3 class="text-[11px] font-semibold text-[var(--theme-text-primary)]">{{ keyPanel === 'upstream' ? t(locale, 'gateway.upstreamKeys') : t(locale, 'gateway.localKeys') }}</h3><p class="mt-0.5 text-[9px] text-[var(--theme-text-tertiary)]">{{ keyPanel === 'upstream' ? t(locale, 'gateway.upstreamKeyHint') : t(locale, 'gateway.localKeyHint') }}</p></div>
          <div class="p-2">
            <div v-if="keyPanel === 'upstream' && draft.upstreamKeys.length === 0" class="flex gap-1.5"><input v-model="upstreamRemark" class="min-w-0 w-20 flex-1 rounded-lg border border-[var(--theme-border-default)] bg-transparent px-2 py-2 text-[10px] text-[var(--theme-text-primary)] outline-none" :placeholder="t(locale, 'gateway.keyRemark')" /><input v-model="upstreamSecret" type="password" class="min-w-0 flex-[1.6] rounded-lg border border-[var(--theme-border-default)] bg-transparent px-2 py-2 font-mono text-[10px] text-[var(--theme-text-primary)] outline-none" :placeholder="t(locale, 'gateway.upstreamKeyPlaceholder')" autocomplete="off" /><button type="button" class="theme-icon-button rounded-lg p-1.5" :title="t(locale, 'gateway.addKey')" :aria-label="t(locale, 'gateway.addKey')" :disabled="keyBusy || !upstreamSecret.trim()" @click="addUpstreamKey"><Plus class="h-3.5 w-3.5" aria-hidden="true" /></button></div>
            <div v-else-if="keyPanel === 'upstream'" class="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2 text-[10px] text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.singleUpstreamKeyHint') }}</div>
            <div v-else-if="draft.localKeys.length === 0" class="flex gap-1.5"><input v-model="localRemark" class="min-w-0 flex-1 rounded-lg border border-[var(--theme-border-default)] bg-transparent px-2 py-2 text-[10px] text-[var(--theme-text-primary)] outline-none" :placeholder="t(locale, 'gateway.keyRemark')" /><button type="button" class="theme-icon-button rounded-lg p-1.5" :title="t(locale, 'gateway.createLocalKey')" :aria-label="t(locale, 'gateway.createLocalKey')" :disabled="keyBusy" @click="createReplacementLocalKey"><Plus class="h-3.5 w-3.5" aria-hidden="true" /></button></div>
            <div v-else class="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2 text-[10px] text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.singleLocalKeyHint') }}</div>
            <div v-for="key in keyPanel === 'upstream' ? draft.upstreamKeys : draft.localKeys" :key="key.id" class="mt-1.5 rounded-lg border border-[var(--theme-border-default)] px-2 py-1.5 text-[10px]"><div class="flex h-6 items-center gap-2"><span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="key.enabled ? 'bg-emerald-500' : 'bg-gray-400'"></span><span class="min-w-0 flex-1 truncate text-[var(--theme-text-secondary)]">{{ key.remark || t(locale, 'gateway.unnamedKey') }}</span><template v-if="keyPanel === 'local'"><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.showLocalKey')" :aria-label="t(locale, 'gateway.showLocalKey')" @click="revealLocalKey(key.id)"><Eye class="h-3 w-3" aria-hidden="true" /></button><button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.copyLocalKey')" :aria-label="t(locale, 'gateway.copyLocalKey')" @click="copyStoredLocalKey(key.id)"><Copy class="h-3 w-3" aria-hidden="true" /></button></template><button type="button" class="theme-icon-button rounded-lg p-1" :title="keyPanel === 'upstream' ? t(locale, 'gateway.deleteKey') : t(locale, 'gateway.revokeKey')" @click="keyPanel === 'upstream' ? deleteUpstreamKey(key.id) : revokeLocalKey(key.id)"><Trash2 class="h-3 w-3 text-red-500" aria-hidden="true" /></button></div><div v-if="keyPanel === 'upstream'" class="mt-1 flex items-center gap-2 border-t border-[var(--theme-border-default)] pt-1"><label class="flex items-center gap-1 text-[9px] text-[var(--theme-text-tertiary)]"><input v-model="key.enabled" type="checkbox" class="h-3 w-3 accent-[var(--theme-accent-primary)]" :aria-label="t(locale, 'gateway.enabled')" @change="updateUpstreamKey(key as GatewayUpstreamKey)" />{{ t(locale, 'gateway.enabled') }}</label></div><p v-if="keyPanel === 'local' && revealedLocalKeys[key.id]" class="mt-1 break-all border-t border-[var(--theme-border-default)] pt-1 font-mono text-[9px] text-[var(--theme-text-primary)]">{{ revealedLocalKeys[key.id] }}</p></div>
          </div>
        </section>
      </template>

      <p v-if="feedback === 'error'" class="text-[10px] leading-snug text-red-500">{{ feedbackMessage }}</p>
      <p v-else-if="feedback === 'saved'" class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" aria-hidden="true" />{{ t(locale, 'gateway.saveSuccess') }}</p>
      <p v-else-if="feedback === 'copied'" class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" aria-hidden="true" />{{ t(locale, 'gateway.copied') }}</p>
    </div>

    <!-- 表单底部 -->
    <footer class="flex gap-2 border-t border-[var(--theme-border-default)] px-4 py-3">
      <button v-if="keyPanel" type="button" class="inline-flex flex-1 items-center justify-center rounded-lg border border-[var(--theme-border-default)] px-3 py-2 text-[10.5px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="keyPanel = null">{{ t(locale, 'gateway.cancel') }}</button>
      <template v-else><button type="button" class="rounded-lg border border-[var(--theme-border-default)] px-3 text-[10.5px] font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="emit('cancel')">{{ t(locale, 'gateway.cancel') }}</button><button type="submit" class="inline-flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3 py-2 text-[10.5px] font-semibold text-[var(--theme-accent-contrast)] shadow-sm transition-opacity disabled:opacity-50" :disabled="saving"><Save class="h-3.5 w-3.5" aria-hidden="true" />{{ draft.id ? t(locale, 'gateway.update') : t(locale, 'gateway.save') }}</button></template>
    </footer>
  </form>
</template>
