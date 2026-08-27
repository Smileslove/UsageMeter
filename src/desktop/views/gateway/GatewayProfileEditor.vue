<script setup lang="ts">
/**
 * 桌面网关页右侧「详情 / 编辑表单」（内嵌，非 modal）：
 * 基本信息（名称/协议/地址/启用）、密钥管理（上游 Key 增删改、本地 Key 生成/查看/复制/撤销）、
 * 模型连通性测试。draft 由父组件持有（props 传入，表单直接编辑其字段）；
 * 提交、取消、密钥 CRUD、复制地址等副作用一律通过事件上抛由父组件编排。
 */
import { computed, onUnmounted, toRef, watch } from 'vue'
import { ArrowLeft, Check, Copy, Eye, KeyRound, Plus, RefreshCw, Save, Trash2, X } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t } from '../../../i18n'
import { gatewayProtocolOptions } from './protocolOptions'
import DesktopSelect from '../../components/DesktopSelect.vue'
import type { SelectOption } from '../../components/DesktopSelect.vue'
import type { GatewayProfile, GatewayUpstreamKey } from '../../../types'
import type { DraftProfile, GatewayDraftInput } from './gatewayTypes'
import { useGatewayKeyManager } from '../../composables/useGatewayKeyManager'
import { useGatewayModelTester } from '../../composables/useGatewayModelTester'
import { useGatewayBaseUrlPreview } from '../../composables/useGatewayBaseUrlPreview'

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
  (e: 'submit', payload: GatewayDraftInput): void
  (e: 'cancel'): void
  (e: 'applyProfile', profile: GatewayProfile): void
  (e: 'updateDraft', partial: Partial<DraftProfile>): void
  (e: 'notice', payload: { kind: 'saved' | 'copied' | 'error'; errorCode?: string }): void
  (e: 'copyAddress'): void
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

const upstreamKeyRecoveryRequired = computed(() => props.draft.credentialRecovery.upstreamKeyRequired)
const localKeyRotationRecommended = computed(() => props.draft.credentialRecovery.localKeyRotationRecommended)

const protocolSelectOptions = computed<SelectOption[]>(() =>
  gatewayProtocolOptions.map(opt => ({ value: opt.value, label: t(locale.value, opt.labelKey) }))
)

function applyProfile(profile: GatewayProfile) {
  emit('applyProfile', profile)
}

function updateDraft(partial: Partial<DraftProfile>) {
  emit('updateDraft', partial)
}

function notify(kind: 'saved' | 'copied' | 'error', errorCode?: string) {
  emit('notice', { kind, errorCode })
}

const draftRef = toRef(props, 'draft')
const keyManager = useGatewayKeyManager(draftRef, applyProfile, notify)
const modelTester = useGatewayModelTester(draftRef, locale, notify, updateDraft)
const { baseUrlPreview, clearBaseUrlPreview, dispose: disposePreview } = useGatewayBaseUrlPreview(draftRef)

const {
  keyBusy, upstreamRemark, upstreamSecret, localRemark, generatedLocalKey,
  keyPanel, revealedLocalKeys,
  addUpstreamKey, deleteUpstreamKey, updateUpstreamKey,
  revokeLocalKey, createReplacementLocalKey, revealLocalKey, copyStoredLocalKey,
} = keyManager

const {
  modelsLoading, modelsResult, modelsPersisted, selectedModelId,
  testingModel, testResult, testErrorMessage,
  refreshModels, testSelectedModel,
} = modelTester

function resetFormState() {
  keyManager.resetKeyState()
  modelTester.resetModelState()
  clearBaseUrlPreview()
}

watch(
  () => props.draft.id,
  id => {
    keyManager.onDraftIdChange(id)
    modelTester.onDraftIdChange(id)
    clearBaseUrlPreview()
  }
)

watch(
  () => props.resetKey,
  () => resetFormState()
)

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

onUnmounted(() => {
  disposePreview()
})
</script>

<template>
  <form class="overflow-hidden rounded-lg border" style="background: var(--theme-surface-gradient)" role="dialog" aria-modal="false" @submit.prevent="onSubmit">
    <!-- 表单头部 -->
    <header class="flex items-start justify-between border-b border-[var(--theme-border-default)] px-4 py-3">
      <div class="min-w-0">
        <h2 class="text-[15px] font-semibold text-[var(--theme-text-primary)]">{{ draft.id ? t(locale, 'gateway.editProfile') : t(locale, 'gateway.newProfile') }}</h2>
        <p class="mt-0.5 truncate text-xs text-[var(--theme-text-tertiary)]">{{ selectedProtocolLabel }}</p>
      </div>
      <button type="button" class="theme-icon-button -mr-1 -mt-1 rounded-lg p-2" :title="t(locale, 'gateway.cancel')" :aria-label="t(locale, 'gateway.cancel')" @click="emit('cancel')"><X class="h-4 w-4" aria-hidden="true" /></button>
    </header>

    <div class="space-y-3 p-4">
      <!-- ============ Key 子面板（上游 / 本地） ============ -->
      <template v-if="keyPanel">
        <button type="button" class="inline-flex items-center gap-1.5 text-xs font-semibold text-[var(--theme-text-secondary)] transition-colors hover:text-[var(--theme-text-primary)]" @click="keyPanel = null">
          <ArrowLeft class="h-3.5 w-3.5" aria-hidden="true" />
          {{ draft.name }}
        </button>
        <section class="rounded-lg border" style="background: var(--theme-surface-muted-gradient)">
          <div class="border-b border-[var(--theme-border-default)] px-3 py-2.5">
            <h3 class="text-xs font-semibold text-[var(--theme-text-primary)]">{{ keyPanel === 'upstream' ? t(locale, 'gateway.upstreamKeys') : t(locale, 'gateway.localKeys') }}</h3>
            <p class="mt-0.5 text-xs text-[var(--theme-text-tertiary)]">{{ keyPanel === 'upstream' ? t(locale, 'gateway.upstreamKeyHint') : t(locale, 'gateway.localKeyHint') }}</p>
          </div>
          <div class="p-3">
            <!-- 添加上游 Key -->
            <div v-if="keyPanel === 'upstream' && draft.upstreamKeys.length === 0" class="flex gap-1.5">
              <input v-model="upstreamRemark" class="theme-input min-w-0 w-20 flex-1 rounded-lg px-2 py-1.5 text-xs outline-none" :placeholder="t(locale, 'gateway.keyRemark')" />
              <input v-model="upstreamSecret" type="password" class="theme-input min-w-0 flex-[1.6] rounded-lg px-2 py-1.5 font-mono text-xs outline-none" :placeholder="t(locale, 'gateway.upstreamKeyPlaceholder')" autocomplete="off" />
              <button type="button" class="theme-icon-button rounded-lg p-1.5" :title="t(locale, 'gateway.addKey')" :aria-label="t(locale, 'gateway.addKey')" :disabled="keyBusy || !upstreamSecret.trim()" @click="addUpstreamKey"><Plus class="h-3.5 w-3.5" aria-hidden="true" /></button>
            </div>
            <div v-else-if="keyPanel === 'upstream'" class="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2.5 text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.singleUpstreamKeyHint') }}</div>

            <!-- 添加本地 Key -->
            <div v-else-if="draft.localKeys.length === 0" class="flex gap-1.5">
              <input v-model="localRemark" class="theme-input min-w-0 flex-1 rounded-lg px-2 py-1.5 text-xs outline-none" :placeholder="t(locale, 'gateway.keyRemark')" />
              <button type="button" class="theme-icon-button rounded-lg p-1.5" :title="t(locale, 'gateway.createLocalKey')" :aria-label="t(locale, 'gateway.createLocalKey')" :disabled="keyBusy" @click="createReplacementLocalKey"><Plus class="h-3.5 w-3.5" aria-hidden="true" /></button>
            </div>
            <div v-else class="rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2.5 text-xs text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.singleLocalKeyHint') }}</div>

            <!-- 已有 Key 列表 -->
            <div v-for="key in keyPanel === 'upstream' ? draft.upstreamKeys : draft.localKeys" :key="key.id" class="mt-2 rounded-lg border border-[var(--theme-border-default)] p-2.5">
              <div class="flex items-center gap-2">
                <span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="key.enabled ? 'bg-emerald-500' : 'bg-gray-400'"></span>
                <span class="min-w-0 flex-1 truncate text-xs font-medium text-[var(--theme-text-secondary)]">{{ key.remark || t(locale, 'gateway.unnamedKey') }}</span>
                <template v-if="keyPanel === 'local'">
                  <button type="button" class="theme-icon-button rounded-md p-1" :title="t(locale, 'gateway.showLocalKey')" :aria-label="t(locale, 'gateway.showLocalKey')" @click="revealLocalKey(key.id)"><Eye class="h-3 w-3" aria-hidden="true" /></button>
                  <button type="button" class="theme-icon-button rounded-md p-1" :title="t(locale, 'gateway.copyLocalKey')" :aria-label="t(locale, 'gateway.copyLocalKey')" @click="copyStoredLocalKey(key.id)"><Copy class="h-3 w-3" aria-hidden="true" /></button>
                </template>
                <button type="button" class="theme-icon-button rounded-md p-1" :title="keyPanel === 'upstream' ? t(locale, 'gateway.deleteKey') : t(locale, 'gateway.revokeKey')" :aria-label="keyPanel === 'upstream' ? t(locale, 'gateway.deleteKey') : t(locale, 'gateway.revokeKey')" @click="keyPanel === 'upstream' ? deleteUpstreamKey(key.id) : revokeLocalKey(key.id)"><Trash2 class="h-3 w-3 text-red-500" aria-hidden="true" /></button>
              </div>
              <div v-if="keyPanel === 'upstream'" class="mt-1.5 flex items-center gap-2 border-t border-[var(--theme-border-default)] pt-1.5">
                <label class="flex items-center gap-1.5 text-xs text-[var(--theme-text-tertiary)]">
                  <input v-model="key.enabled" type="checkbox" class="h-3 w-3 accent-[var(--theme-accent-primary)]" :aria-label="t(locale, 'gateway.enabled')" @change="updateUpstreamKey(key as GatewayUpstreamKey)" />
                  {{ t(locale, 'gateway.enabled') }}
                </label>
              </div>
              <p v-if="keyPanel === 'local' && revealedLocalKeys[key.id]" class="mt-1.5 break-all border-t border-[var(--theme-border-default)] pt-1.5 font-mono text-xs text-[var(--theme-text-primary)]">{{ revealedLocalKeys[key.id] }}</p>
            </div>

            <!-- 生成的本地 Key 一次性展示 -->
            <div v-if="keyPanel === 'local' && generatedLocalKey" class="mt-2 rounded-lg border border-emerald-500/20 bg-emerald-500/5 p-2.5">
              <p class="mb-1 text-xs font-semibold text-emerald-600 dark:text-emerald-300">{{ t(locale, 'gateway.localKeyCreated') }}</p>
              <p class="break-all font-mono text-xs text-[var(--theme-text-primary)]">{{ generatedLocalKey }}</p>
            </div>
          </div>
        </section>
      </template>

      <!-- ============ 主表单 ============ -->
      <template v-else>
        <!-- 凭据恢复提示 -->
        <div v-if="upstreamKeyRecoveryRequired || localKeyRotationRecommended" class="rounded-lg border border-amber-500/25 bg-amber-500/8 px-3 py-2 text-xs leading-snug text-amber-700 dark:text-amber-200">
          <p v-if="upstreamKeyRecoveryRequired">{{ t(locale, 'gateway.upstreamKeyRecoveryNotice') }}</p>
          <p v-if="localKeyRotationRecommended" :class="{ 'mt-1': upstreamKeyRecoveryRequired }">{{ t(locale, 'gateway.localKeyRecoveryNotice') }}</p>
        </div>

        <!-- 基本信息卡片 -->
        <section class="rounded-lg border" style="background: var(--theme-surface-muted-gradient)">
          <div class="border-b border-[var(--theme-border-default)] px-3 py-2">
            <h3 class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.profileInfo') }}</h3>
          </div>
          <div class="p-3">
            <label class="flex items-center gap-3 py-1.5">
              <span class="w-16 shrink-0 text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.name') }}</span>
              <input v-model="draft.name" class="theme-input min-w-0 flex-1 rounded-lg px-2.5 py-1.5 text-xs outline-none" :placeholder="t(locale, 'gateway.namePlaceholder')" />
            </label>
            <label class="flex items-center gap-3 py-1.5">
              <span class="w-16 shrink-0 text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.protocol') }}</span>
              <DesktopSelect v-model="draft.protocol" :options="protocolSelectOptions" :aria-label="t(locale, 'gateway.protocol')" class="min-w-0 flex-1" />
            </label>
            <label class="flex items-center gap-3 py-1.5">
              <span class="w-16 shrink-0 text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.baseUrl') }}</span>
              <input v-model="draft.baseUrl" class="theme-input min-w-0 flex-1 rounded-lg px-2.5 py-1.5 font-mono text-xs outline-none" :placeholder="t(locale, 'gateway.baseUrlPlaceholder')" inputmode="url" />
            </label>
            <div v-if="baseUrlPreview" class="mt-1.5 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-2.5 py-2">
              <p class="break-all font-mono text-xs leading-relaxed text-[var(--theme-text-secondary)]">POST {{ baseUrlPreview.sampleRequestUrl }}</p>
              <p v-if="!baseUrlPreview.basePathAdded" class="mt-1 text-xs text-emerald-600 dark:text-emerald-400">
                {{ t(locale, 'gateway.basePathAlreadyIncluded', { path: baseUrlPreview.basePath }) }}
              </p>
            </div>
            <label class="flex items-center gap-3 py-1.5">
              <span class="w-16 shrink-0 text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.upstreamKey') }}</span>
              <input v-model="upstreamSecret" type="password" class="theme-input min-w-0 flex-1 rounded-lg px-2.5 py-1.5 font-mono text-xs outline-none" :placeholder="upstreamKeyRecoveryRequired ? t(locale, 'gateway.upstreamKeyRecoveryPlaceholder') : (draft.id ? t(locale, 'gateway.upstreamKeyKeepHint') : t(locale, 'gateway.upstreamKeyPlaceholder'))" autocomplete="off" />
            </label>
            <label class="flex cursor-pointer items-center justify-between py-1.5">
              <span class="text-xs font-medium text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.enabled') }}</span>
              <input v-model="draft.enabled" type="checkbox" class="peer sr-only" />
              <span class="relative h-5 w-9 rounded-full bg-[var(--theme-border-strong)] transition-colors peer-checked:bg-[var(--theme-accent-primary)] after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:shadow-sm after:transition-transform peer-checked:after:translate-x-4 peer-focus-visible:ring-2 peer-focus-visible:ring-[var(--theme-accent-primary)] peer-focus-visible:ring-offset-2"></span>
            </label>
          </div>
        </section>

        <p class="px-1 text-xs leading-snug text-[var(--theme-text-quaternary)]">{{ t(locale, 'gateway.managedKeyHint') }}</p>

        <!-- 接入地址卡片 -->
        <section v-if="draft.id" class="rounded-lg border" style="background: var(--theme-surface-muted-gradient)">
          <div class="flex items-center justify-between gap-2 border-b border-[var(--theme-border-default)] px-3 py-2">
            <h3 class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.address') }}</h3>
            <div class="flex items-center gap-1">
              <button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.localKeys')" :aria-label="t(locale, 'gateway.localKeys')" @click="keyPanel = 'local'"><KeyRound class="h-3.5 w-3.5" aria-hidden="true" /></button>
              <button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click="emit('copyAddress')"><Copy class="h-3.5 w-3.5" aria-hidden="true" /></button>
            </div>
          </div>
          <div class="px-3 py-2.5">
            <p class="break-all font-mono text-xs leading-relaxed text-[var(--theme-text-primary)]">{{ selectedAddress }}</p>
          </div>
        </section>

        <!-- 模型测试卡片 -->
        <section v-if="draft.id" class="rounded-lg border" style="background: var(--theme-surface-muted-gradient)">
          <div class="flex items-center justify-between gap-2 border-b border-[var(--theme-border-default)] px-3 py-2">
            <h3 class="text-xs font-semibold uppercase tracking-wide text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelTest') }}</h3>
            <button type="button" class="theme-icon-button rounded-lg p-1" :title="t(locale, 'gateway.refreshModels')" :aria-label="t(locale, 'gateway.refreshModels')" :disabled="modelsLoading" @click="refreshModels"><RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': modelsLoading }" aria-hidden="true" /></button>
          </div>
          <div class="p-3">
            <p v-if="modelsLoading" class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsLoading') }}</p>
            <p v-else-if="modelsResult && !modelsResult.ok" class="text-xs text-red-500">{{ t(locale, 'gateway.modelsFailed') }}</p>
            <p v-else-if="modelsResult && modelsResult.ok && modelsPersisted && modelsResult.models.length > 0" class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsSaved', { count: modelsResult.models.length }) }}</p>
            <p v-else-if="modelsResult && modelsResult.ok && !modelsPersisted && modelsResult.models.length > 0" class="text-xs text-amber-500">{{ t(locale, 'gateway.modelsSaveFailed') }}</p>
            <p v-else-if="modelsResult && modelsResult.ok && modelsResult.models.length === 0" class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.modelsEmpty') }}</p>

            <div v-if="modelsResult && modelsResult.models.length > 0" class="mt-2 max-h-[120px] space-y-1 overflow-y-auto">
              <label v-for="model in modelsResult.models" :key="model.id" class="flex cursor-pointer items-center gap-2 rounded-lg border px-2 py-1.5 text-xs transition-colors" :class="selectedModelId === model.id ? 'border-[var(--theme-accent-primary)] bg-[var(--theme-accent-primary)]/5' : 'border-[var(--theme-border-default)] hover:bg-[var(--theme-bg-hover)]'">
                <input v-model="selectedModelId" :value="model.id" type="radio" class="sr-only" />
                <span class="h-1.5 w-1.5 shrink-0 rounded-full" :class="selectedModelId === model.id ? 'bg-[var(--theme-accent-primary)]' : 'bg-[var(--theme-border-strong)]'"></span>
                <span class="min-w-0 flex-1 truncate font-mono text-[var(--theme-text-secondary)]">{{ model.id }}</span>
              </label>
            </div>

            <div v-if="modelsResult && modelsResult.models.length > 0" class="mt-2 flex items-center gap-2">
              <button type="button" class="rounded-lg border border-[var(--theme-border-default)] px-2.5 py-1 text-xs font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:opacity-50" :disabled="testingModel || !selectedModelId" @click="testSelectedModel">
                {{ testingModel ? t(locale, 'gateway.testingModel') : t(locale, 'gateway.testModel') }}
              </button>
              <span v-if="testResult && testResult.ok" class="text-xs text-emerald-600 dark:text-emerald-300">{{ t(locale, 'gateway.testSuccess', { ms: testResult.latencyMs ?? 0 }) }}</span>
              <span v-else-if="testResult && !testResult.ok" class="text-xs text-red-500">{{ testErrorMessage }}</span>
            </div>
          </div>
        </section>

        <p class="px-1 text-xs leading-snug text-[var(--theme-text-quaternary)]">{{ t(locale, 'gateway.singleUpstreamKeyHint') }}</p>
      </template>

      <!-- 反馈 -->
      <p v-if="feedback === 'error'" class="text-xs leading-snug text-red-500">{{ feedbackMessage }}</p>
      <p v-else-if="feedback === 'saved'" class="flex items-center gap-1 text-xs text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" aria-hidden="true" />{{ t(locale, 'gateway.saveSuccess') }}</p>
      <p v-else-if="feedback === 'copied'" class="flex items-center gap-1 text-xs text-emerald-600 dark:text-emerald-300"><Check class="h-3 w-3" aria-hidden="true" />{{ t(locale, 'gateway.copied') }}</p>
    </div>

    <!-- 表单底部 -->
    <footer class="flex gap-2 border-t border-[var(--theme-border-default)] px-4 py-3">
      <button v-if="keyPanel" type="button" class="inline-flex flex-1 items-center justify-center rounded-lg border border-[var(--theme-border-default)] px-3 py-2 text-xs font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="keyPanel = null">{{ t(locale, 'gateway.cancel') }}</button>
      <template v-else>
        <button type="button" class="rounded-lg border border-[var(--theme-border-default)] px-3 py-2 text-xs font-semibold text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)]" @click="emit('cancel')">{{ t(locale, 'gateway.cancel') }}</button>
        <button type="submit" class="inline-flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-lg bg-[var(--theme-accent-primary)] px-3 py-2 text-xs font-semibold text-[var(--theme-accent-contrast)] shadow-sm transition-opacity disabled:opacity-50" :disabled="saving">
          <Save class="h-3.5 w-3.5" aria-hidden="true" />
          {{ draft.id ? t(locale, 'gateway.update') : t(locale, 'gateway.save') }}
        </button>
      </template>
    </footer>
  </form>
</template>
