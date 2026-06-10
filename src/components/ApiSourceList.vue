<script setup lang="ts">
import { ref, computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import type { ApiSource, SourceQuotaQueryConfig } from '../types'
import { Pencil, Trash2, Merge, Key, Clock, ExternalLink, AlertTriangle, Check, X, ChevronLeft, Wallet, LoaderCircle } from 'lucide-vue-next'
import LobeIcon from './LobeIcon.vue'
import { SOURCE_ICON_CATEGORIES } from '../iconConfig'

const store = useMonitorStore()

const editingSourceId = ref<string | null>(null)
const iconPickerSourceId = ref<string | null>(null)
const iconSearch = ref('')
const iconLoadFailed = ref<Record<string, boolean>>({})

const onIconError = (sourceId: string) => {
  iconLoadFailed.value = { ...iconLoadFailed.value, [sourceId]: true }
}

const filteredCategories = computed(() => {
  const q = iconSearch.value.toLowerCase().trim()
  if (!q) return SOURCE_ICON_CATEGORIES
  return SOURCE_ICON_CATEGORIES.map(cat => ({
    label: cat.label,
    icons: cat.icons.filter(icon => icon.toLowerCase().includes(q)),
  })).filter(cat => cat.icons.length > 0)
})
const editingName = ref('')
const keyNoteDrafts = ref<Record<string, string>>({})

const showMergeDialog = ref(false)
const mergeSourceId = ref<string | null>(null)
const mergeTargetId = ref<string | null>(null)

const showDeleteDialog = ref(false)
const deleteSourceId = ref<string | null>(null)
const deleteWithRecords = ref(false)
const quotaEditorSourceId = ref<string | null>(null)
const quotaSavingSourceId = ref<string | null>(null)
const quotaDrafts = ref<Record<string, SourceQuotaQueryConfig>>({})

const sources = computed(() => store.settings.sourceAware.sources)
const hasNewSources = computed(() => sources.value.some(s => s.autoDetected && !s.displayName))

const noteKey = (sourceId: string, prefix: string) => `${sourceId}:${prefix}`

const getSourceName = (source: ApiSource) => {
  if (source.displayName) return source.displayName
  if (source.baseUrl) {
    try {
      return new URL(source.baseUrl).hostname
    } catch {
      return source.baseUrl
    }
  }
  return t(store.settings.locale, 'sources.officialAnthropic')
}

const getSourceUrl = (source: ApiSource) => {
  if (!source.baseUrl) return t(store.settings.locale, 'sources.officialAnthropic')
  return source.baseUrl
}

const getKeyNote = (source: ApiSource, prefix: string) => {
  const draft = keyNoteDrafts.value[noteKey(source.id, prefix)]
  return draft ?? source.apiKeyNotes?.[prefix] ?? ''
}

const updateKeyNoteDraft = (source: ApiSource, prefix: string, event: Event) => {
  const input = event.target as HTMLInputElement
  keyNoteDrafts.value[noteKey(source.id, prefix)] = input.value
}

const saveKeyNote = async (source: ApiSource, prefix: string) => {
  const key = noteKey(source.id, prefix)
  const note = (keyNoteDrafts.value[key] ?? source.apiKeyNotes?.[prefix] ?? '').trim()
  await store.updateSourceKeyNote(source.id, prefix, note)
  delete keyNoteDrafts.value[key]
}

const formatTime = (ms: number) => {
  const date = new Date(ms)
  const now = new Date()
  const diffMs = now.getTime() - date.getTime()
  const diffMins = Math.floor(diffMs / 60000)
  const diffHours = Math.floor(diffMs / 3600000)
  const diffDays = Math.floor(diffMs / 86400000)

  if (diffMins < 1) return t(store.settings.locale, 'common.justNow')
  if (diffMins < 60) return `${diffMins}${t(store.settings.locale, 'common.minutesAgo')}`
  if (diffHours < 24) return `${diffHours}${t(store.settings.locale, 'common.hoursAgo')}`
  if (diffDays < 7) return `${diffDays}${t(store.settings.locale, 'common.daysAgo')}`

  return date.toLocaleDateString(store.settings.locale)
}

const startEdit = (source: ApiSource) => {
  editingSourceId.value = source.id
  editingName.value = source.displayName || getSourceName(source)
}

const saveName = async () => {
  if (!editingSourceId.value) return
  await store.renameSource(editingSourceId.value, editingName.value.trim())
  editingSourceId.value = null
  editingName.value = ''
}

const cancelEdit = () => {
  editingSourceId.value = null
  editingName.value = ''
}

const setSourceIcon = async (sourceId: string, icon: string | null) => {
  const source = sources.value.find(s => s.id === sourceId)
  if (!source) return
  source.icon = icon || undefined
  source.autoDetected = false
  await store.saveSettings()
  iconPickerSourceId.value = null
}

const toggleIconPicker = (sourceId: string) => {
  if (iconPickerSourceId.value === sourceId) {
    iconPickerSourceId.value = null
    iconSearch.value = ''
  } else {
    iconPickerSourceId.value = sourceId
    iconSearch.value = ''
  }
}

const sameBaseSourceCount = (source: ApiSource) => {
  return sources.value.filter(s => s.id !== source.id && s.baseUrl === source.baseUrl).length
}

const openMergeDialog = (sourceId: string) => {
  mergeSourceId.value = sourceId
  mergeTargetId.value = null
  showMergeDialog.value = true
}

const doMerge = async () => {
  if (mergeSourceId.value && mergeTargetId.value) {
    await store.mergeSource(mergeSourceId.value, mergeTargetId.value)
    showMergeDialog.value = false
    mergeSourceId.value = null
    mergeTargetId.value = null
  }
}

const mergeTargets = computed(() => {
  if (!mergeSourceId.value) return []
  const source = sources.value.find(s => s.id === mergeSourceId.value)
  return sources.value.filter(s => s.id !== mergeSourceId.value && s.baseUrl === source?.baseUrl)
})

const openDeleteDialog = (sourceId: string) => {
  deleteSourceId.value = sourceId
  deleteWithRecords.value = false
  showDeleteDialog.value = true
}

const doDelete = async () => {
  if (deleteSourceId.value) {
    await store.deleteSource(deleteSourceId.value, deleteWithRecords.value)
    showDeleteDialog.value = false
    deleteSourceId.value = null
    deleteWithRecords.value = false
  }
}

const truncatePrefix = (prefix: string) => {
  if (prefix.length <= 11) return prefix
  return prefix.slice(0, 11) + '···'
}

const prefixTitle = (prefix: string) => {
  return t(store.settings.locale, 'sources.keyPrefixTruncated', { prefix })
}

const defaultQuotaDraft = (): SourceQuotaQueryConfig => ({
  enabled: false,
  queryType: 'generic_balance',
  accessToken: '',
  userId: '',
})

const ensureQuotaDraft = (source: ApiSource) => {
  if (!quotaDrafts.value[source.id]) {
    const existing = source.quotaQuery
    quotaDrafts.value[source.id] = existing
      ? {
          enabled: existing.enabled,
          queryType: existing.queryType,
          accessToken: existing.queryType === 'generic_balance'
            ? (source.apiKeyNotes?.__quota_api_key || '')
            : (existing.accessToken || ''),
          userId: existing.userId || '',
        }
      : defaultQuotaDraft()
  }
  return quotaDrafts.value[source.id]
}

const toggleQuotaEditor = (source: ApiSource) => {
  if (quotaEditorSourceId.value === source.id) {
    quotaEditorSourceId.value = null
    return
  }
  ensureQuotaDraft(source)
  quotaEditorSourceId.value = source.id
}

const saveQuotaQuery = async (source: ApiSource) => {
  const draft = ensureQuotaDraft(source)
  quotaSavingSourceId.value = source.id
  try {
    const trimmedAccessToken = draft.accessToken?.trim() || ''
    const trimmedUserId = draft.userId?.trim() || ''
    if (!source.apiKeyNotes) source.apiKeyNotes = {}
    if (draft.queryType === 'generic_balance') {
      if (trimmedAccessToken) {
        source.apiKeyNotes.__quota_api_key = trimmedAccessToken
      } else {
        delete source.apiKeyNotes.__quota_api_key
      }
    }
    const quotaQuery = draft.enabled
      ? {
          enabled: true,
          queryType: draft.queryType,
          accessToken: draft.queryType === 'new_api' ? trimmedAccessToken : undefined,
          userId: draft.queryType === 'new_api' ? trimmedUserId : undefined,
        }
      : null
    await store.updateSourceQuotaQuery(source.id, quotaQuery)
    quotaEditorSourceId.value = null
  } finally {
    quotaSavingSourceId.value = null
  }
}

const clearQuotaQuery = async (source: ApiSource) => {
  quotaSavingSourceId.value = source.id
  try {
    quotaDrafts.value[source.id] = defaultQuotaDraft()
    if (source.apiKeyNotes) {
      delete source.apiKeyNotes.__quota_api_key
    }
    await store.updateSourceQuotaQuery(source.id, null)
    quotaEditorSourceId.value = null
  } finally {
    quotaSavingSourceId.value = null
  }
}

const deletingSource = computed(() => {
  if (!deleteSourceId.value) return null
  return sources.value.find(s => s.id === deleteSourceId.value)
})

defineProps<{
  onBack: () => void
}>()
</script>

<template>
  <div class="space-y-3">
    <div class="flex items-center gap-2">
      <button
        @click="$props.onBack"
        class="rounded-lg p-1 text-[var(--theme-text-tertiary)] transition-colors hover:bg-gray-100 hover:text-[var(--theme-text-primary)]"
      >
        <ChevronLeft class="w-4 h-4" />
      </button>
      <h3 class="text-sm font-semibold text-[var(--theme-text-primary)]">
        {{ t(store.settings.locale, 'sources.title') }}
      </h3>
      <span v-if="hasNewSources" class="theme-status-danger rounded-full border px-1.5 py-0.5 text-xs font-medium">
        {{ sources.filter(s => s.autoDetected && !s.displayName).length }}
      </span>
      <span class="ml-auto text-xs text-[var(--theme-text-tertiary)]">{{ sources.length }} {{ t(store.settings.locale, 'sources.sourcesCount') }}</span>
    </div>

    <div v-if="sources.length === 0" class="theme-surface rounded-xl border py-8 text-center">
      <Key class="mx-auto mb-2 h-7 w-7 text-[var(--theme-text-quaternary)]" />
      <p class="text-xs text-[var(--theme-text-tertiary)]">
        {{ t(store.settings.locale, 'sources.noSources') }}
      </p>
    </div>

    <div v-else class="space-y-1.5">
      <div
        v-for="source in sources"
        :key="source.id"
        class="theme-surface rounded-xl border p-2"
      >
        <!-- Row 1: icon/dot + name + time + actions -->
        <div class="flex items-center gap-1.5">
          <button
            @click="toggleIconPicker(source.id)"
            class="shrink-0 hover:scale-110 transition-transform cursor-pointer relative"
            :title="t(store.settings.locale, 'sources.changeIcon')"
          >
            <LobeIcon
              v-if="source.icon && !iconLoadFailed[source.id]"
              :slug="source.icon"
              :size="18"
              @error="onIconError(source.id)"
            />
            <span
              v-else
              class="w-3 h-3 rounded-full block"
              :style="{ backgroundColor: source.color }"
            ></span>
          </button>

          <!-- Editing state -->
          <template v-if="editingSourceId === source.id">
            <input
              v-model="editingName"
              :placeholder="t(store.settings.locale, 'sources.namePlaceholder')"
              class="theme-input min-w-0 flex-1 rounded px-1.5 py-0.5 text-xs"
              @keyup.enter="saveName"
              @keyup.esc="cancelEdit"
              autofocus
            />
            <button
              @click="saveName"
              class="theme-button-accent shrink-0 rounded p-0.5 transition-colors"
            >
              <Check class="w-3.5 h-3.5" />
            </button>
            <button
              @click="cancelEdit"
              class="theme-button-secondary shrink-0 rounded p-0.5 transition-colors"
            >
              <X class="w-3.5 h-3.5" />
            </button>
          </template>

          <!-- Normal state -->
          <template v-else>
            <span class="min-w-0 flex-1 truncate text-xs font-semibold text-[var(--theme-text-primary)]">
              {{ getSourceName(source) }}
            </span>

            <span class="flex shrink-0 items-center gap-0.5 text-xs text-[var(--theme-text-tertiary)]">
              <Clock class="w-3.5 h-3.5" />
              {{ formatTime(source.lastSeenMs) }}
            </span>

            <button
              @click="startEdit(source)"
              class="shrink-0 p-0.5 text-[var(--theme-text-tertiary)] transition-colors hover:text-[var(--theme-text-primary)]"
            >
              <Pencil class="w-3.5 h-3.5" />
            </button>

            <button
              :title="t(store.settings.locale, 'sources.mergeInto')"
              @click="openMergeDialog(source.id)"
              :disabled="sameBaseSourceCount(source) === 0"
              :class="[
                'p-0.5 rounded transition-colors shrink-0',
                sameBaseSourceCount(source) === 0
                  ? 'text-[var(--theme-text-quaternary)] cursor-not-allowed'
                  : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)]'
              ]"
            >
              <Merge class="w-3.5 h-3.5" />
            </button>
            <button
              :title="t(store.settings.locale, 'sources.delete')"
              @click="openDeleteDialog(source.id)"
              class="p-0.5 text-red-400 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-500/10 rounded transition-colors shrink-0"
            >
              <Trash2 class="w-3.5 h-3.5" />
            </button>
            <button
              :title="t(store.settings.locale, 'sources.quotaQueryTitle')"
              @click="toggleQuotaEditor(source)"
              class="shrink-0 p-0.5 text-[var(--theme-text-tertiary)] transition-colors hover:text-[var(--theme-text-primary)]"
            >
              <Wallet class="w-3.5 h-3.5" />
            </button>
          </template>
        </div>

        <!-- Row 2: URL -->
        <div class="mt-1 flex items-center gap-1 text-xs text-[var(--theme-text-tertiary)]">
          <ExternalLink class="w-3.5 h-3.5 shrink-0" />
          <span class="font-mono truncate" :title="getSourceUrl(source)">{{ getSourceUrl(source) }}</span>
        </div>

        <!-- Rows 3+: Key prefix + note -->
        <div
          v-for="prefix in source.apiKeyPrefixes"
          :key="prefix"
          class="mt-1 flex items-center gap-1"
        >
          <Key class="w-3.5 h-3.5 text-gray-300 dark:text-gray-600 shrink-0" />
          <span
            class="text-xs font-mono text-gray-500 dark:text-gray-400 shrink-0 cursor-default"
            :title="prefixTitle(prefix)"
          >{{ truncatePrefix(prefix) }}</span>
          <input
            :value="getKeyNote(source, prefix)"
            :placeholder="t(store.settings.locale, 'sources.keyNotePlaceholder')"
            class="theme-input min-w-0 flex-1 rounded px-1.5 py-0.5 text-xs text-[var(--theme-text-secondary)]"
            @input="updateKeyNoteDraft(source, prefix, $event)"
            @blur="saveKeyNote(source, prefix)"
            @keyup.enter="saveKeyNote(source, prefix)"
          />
        </div>

        <div v-if="source.quotaQuery?.enabled" class="mt-1 flex items-center gap-1 text-xs text-emerald-600 dark:text-emerald-400">
          <Wallet class="w-3.5 h-3.5 shrink-0" />
          <span>{{ t(store.settings.locale, 'sources.quotaQueryEnabledBadge') }}</span>
        </div>

        <div
          v-if="quotaEditorSourceId === source.id"
          class="theme-surface-muted mt-1.5 rounded-lg border p-2 space-y-2"
        >
          <div class="flex items-center justify-between gap-2">
            <div>
              <p class="text-xs font-semibold text-[var(--theme-text-primary)]">
                {{ t(store.settings.locale, 'sources.quotaQueryTitle') }}
              </p>
              <p class="mt-0.5 text-[11px] text-[var(--theme-text-tertiary)]">
                {{ t(store.settings.locale, 'sources.quotaQueryDesc') }}
              </p>
            </div>
            <label class="flex items-center gap-1.5 text-[11px] text-[var(--theme-text-secondary)]">
              <input
                v-model="ensureQuotaDraft(source).enabled"
                type="checkbox"
                class="w-3.5 h-3.5 rounded border-gray-300"
              />
              <span>{{ t(store.settings.locale, 'common.enabled') }}</span>
            </label>
          </div>

          <div class="space-y-1.5">
            <div>
              <label class="mb-1 block text-[11px] font-medium text-[var(--theme-text-secondary)]">
                {{ t(store.settings.locale, 'sources.quotaQueryMode') }}
              </label>
              <select
                v-model="ensureQuotaDraft(source).queryType"
                class="theme-input w-full rounded px-2 py-1 text-xs"
              >
                <option value="generic_balance">{{ t(store.settings.locale, 'sources.quotaQueryModeGeneric') }}</option>
                <option value="new_api">{{ t(store.settings.locale, 'sources.quotaQueryModeNewApi') }}</option>
              </select>
            </div>
            <div>
              <label class="mb-1 block text-[11px] font-medium text-[var(--theme-text-secondary)]">
                {{ ensureQuotaDraft(source).queryType === 'new_api'
                  ? t(store.settings.locale, 'sources.quotaAccessToken')
                  : t(store.settings.locale, 'sources.quotaApiKey') }}
              </label>
              <input
                v-model="ensureQuotaDraft(source).accessToken"
                type="password"
                class="theme-input w-full rounded px-2 py-1 text-xs"
                :placeholder="ensureQuotaDraft(source).queryType === 'new_api'
                  ? t(store.settings.locale, 'sources.quotaAccessTokenPlaceholder')
                  : t(store.settings.locale, 'sources.quotaApiKeyPlaceholder')"
              />
            </div>
            <div v-if="ensureQuotaDraft(source).queryType === 'new_api'">
              <label class="mb-1 block text-[11px] font-medium text-[var(--theme-text-secondary)]">
                {{ t(store.settings.locale, 'sources.quotaUserId') }}
              </label>
              <input
                v-model="ensureQuotaDraft(source).userId"
                type="text"
                class="theme-input w-full rounded px-2 py-1 text-xs"
                :placeholder="t(store.settings.locale, 'sources.quotaUserIdPlaceholder')"
              />
            </div>
          </div>

          <div class="flex items-center justify-end gap-2">
            <button
              @click="clearQuotaQuery(source)"
              class="theme-button-secondary rounded-lg px-2 py-1 text-[11px]"
              :disabled="quotaSavingSourceId === source.id"
            >
              {{ t(store.settings.locale, 'sources.quotaClear') }}
            </button>
            <button
              @click="saveQuotaQuery(source)"
              class="theme-button-accent rounded-lg px-2 py-1 text-[11px] flex items-center gap-1"
              :disabled="quotaSavingSourceId === source.id"
            >
              <LoaderCircle v-if="quotaSavingSourceId === source.id" class="w-3 h-3 animate-spin" />
              <span>{{ t(store.settings.locale, 'sources.quotaSave') }}</span>
            </button>
          </div>
        </div>

        <!-- Icon picker -->
        <div
          v-if="iconPickerSourceId === source.id"
          class="theme-surface-muted mt-1.5 rounded-lg border p-2"
        >
          <input
            v-model="iconSearch"
            type="text"
            :placeholder="t(store.settings.locale, 'sources.searchIcon')"
            class="theme-input mb-2 w-full rounded px-2 py-1 text-[11px]"
            autofocus
          />

          <div
            v-for="category in filteredCategories"
            :key="category.label"
            v-show="category.icons.length > 0"
            class="mb-2 last:mb-0"
          >
            <p class="mb-1 px-0.5 text-[10px] text-[var(--theme-text-tertiary)]">{{ category.label }} · {{ category.icons.length }}</p>
            <div class="grid grid-cols-9 gap-0.5 max-h-[120px] overflow-y-auto">
              <button
                v-for="icon in category.icons"
                :key="icon"
                @click="setSourceIcon(source.id, icon)"
                :class="[
                  'p-1 rounded-md transition-colors flex items-center justify-center',
                  source.icon === icon
                    ? 'bg-[var(--theme-accent-soft)] ring-1 ring-[var(--theme-accent-primary)]'
                    : 'theme-surface hover:bg-gray-100'
                ]"
                :title="icon"
              >
                <LobeIcon :slug="icon" :size="18" />
              </button>
            </div>
          </div>

          <button
            v-if="source.icon"
            @click="setSourceIcon(source.id, null)"
            class="mt-1 text-[10px] text-[var(--theme-text-tertiary)] transition-colors hover:text-red-500"
          >
            {{ t(store.settings.locale, 'sources.removeIcon') }}
          </button>
        </div>
      </div>
    </div>

    <Teleport to="body">
      <Transition
        enter-active-class="transition ease-out duration-200"
        enter-from-class="opacity-0"
        enter-to-class="opacity-100"
        leave-active-class="transition ease-in duration-150"
        leave-from-class="opacity-100"
        leave-to-class="opacity-0"
      >
        <div v-if="showMergeDialog" class="theme-backdrop fixed inset-0 z-50 flex items-center justify-center p-4">
          <div class="theme-modal-panel w-full max-w-xs rounded-2xl p-5">
            <h4 class="mb-1 text-sm font-semibold text-[var(--theme-text-primary)]">
              {{ t(store.settings.locale, 'sources.mergeInto') }}
            </h4>
            <p class="mb-3 text-xs text-[var(--theme-text-tertiary)]">
              {{ t(store.settings.locale, 'sources.mergeConfirm') }}
            </p>
            <div class="space-y-1.5">
              <button
                v-for="target in mergeTargets"
                :key="target.id"
                @click="mergeTargetId = target.id"
                :class="[
                  'w-full flex items-center gap-2 p-2.5 rounded-xl border transition-colors text-left',
                  mergeTargetId === target.id
                    ? 'theme-accent-soft theme-accent-border'
                    : 'theme-surface hover:bg-gray-50'
                ]"
              >
                <span
                  class="w-2.5 h-2.5 rounded-full shrink-0"
                  :style="{ backgroundColor: target.color }"
                ></span>
                <span class="text-xs text-[var(--theme-text-primary)]">{{ getSourceName(target) }}</span>
              </button>
            </div>
            <div class="flex gap-2 mt-4">
              <button
                @click="showMergeDialog = false"
                class="theme-button-secondary flex-1 rounded-xl py-2 text-[12px] font-medium transition-colors"
              >
                {{ t(store.settings.locale, 'common.cancel') }}
              </button>
              <button
                @click="doMerge"
                :disabled="!mergeTargetId"
                :class="[
                  'flex-1 py-2 text-[12px] font-medium text-white rounded-xl transition-colors',
                  mergeTargetId ? 'theme-button-accent' : 'bg-[var(--theme-accent-primary)] opacity-40 cursor-not-allowed'
                ]"
              >
                {{ t(store.settings.locale, 'common.confirm') }}
              </button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>

    <Teleport to="body">
      <Transition
        enter-active-class="transition ease-out duration-200"
        enter-from-class="opacity-0"
        enter-to-class="opacity-100"
        leave-active-class="transition ease-in duration-150"
        leave-from-class="opacity-100"
        leave-to-class="opacity-0"
      >
        <div v-if="showDeleteDialog" class="theme-backdrop fixed inset-0 z-50 flex items-center justify-center p-4">
          <div class="theme-modal-panel w-full max-w-xs rounded-2xl p-5">
            <div class="flex flex-col items-center text-center mb-4">
              <div class="theme-status-danger mb-3 flex h-10 w-10 items-center justify-center rounded-full border">
                <AlertTriangle class="w-5 h-5 text-red-500" />
              </div>
              <h4 class="text-sm font-semibold text-[var(--theme-text-primary)]">
                {{ t(store.settings.locale, 'sources.delete') }}
              </h4>
              <p class="mt-1 text-xs text-[var(--theme-text-tertiary)]">
                {{ t(store.settings.locale, 'sources.deleteConfirm') }}
              </p>
            </div>

            <div v-if="deletingSource" class="theme-surface-muted mb-3 flex items-center gap-2 rounded-xl border p-2.5">
              <span
                class="w-2.5 h-2.5 rounded-full shrink-0"
                :style="{ backgroundColor: deletingSource.color }"
              ></span>
              <span class="text-xs font-medium text-[var(--theme-text-primary)]">{{ getSourceName(deletingSource) }}</span>
            </div>

            <label class="theme-status-danger mb-4 flex cursor-pointer items-center gap-2.5 rounded-xl border p-2.5">
              <input
                type="checkbox"
                v-model="deleteWithRecords"
                class="w-4 h-4 rounded border-gray-300 text-red-500 focus:ring-red-500"
              />
              <span class="text-xs">
                {{ t(store.settings.locale, 'sources.deleteRecordsToo') }}
              </span>
            </label>

            <div class="flex gap-2">
              <button
                @click="showDeleteDialog = false"
                class="theme-button-secondary flex-1 rounded-xl py-2 text-[12px] font-medium transition-colors"
              >
                {{ t(store.settings.locale, 'common.cancel') }}
              </button>
              <button
                @click="doDelete"
                class="theme-status-danger flex-1 rounded-xl border py-2 text-[12px] font-medium transition-colors"
              >
                {{ t(store.settings.locale, 'common.confirm') }}
              </button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>
