<script setup lang="ts">
import { ref, computed } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import type { ApiSource } from '../types'
import { Pencil, Trash2, Merge, Key, Clock, ExternalLink, AlertTriangle, Check, X, ChevronLeft } from 'lucide-vue-next'
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
        class="p-1 rounded-lg text-gray-400 hover:text-gray-600 hover:bg-gray-100 dark:hover:bg-neutral-800 transition-colors"
      >
        <ChevronLeft class="w-4 h-4" />
      </button>
      <h3 class="text-sm font-semibold text-gray-800 dark:text-gray-100">
        {{ t(store.settings.locale, 'sources.title') }}
      </h3>
      <span v-if="hasNewSources" class="px-1.5 py-0.5 text-xs font-medium bg-red-100 text-red-600 dark:bg-red-500/20 dark:text-red-400 rounded-full">
        {{ sources.filter(s => s.autoDetected && !s.displayName).length }}
      </span>
      <span class="ml-auto text-xs text-gray-400">{{ sources.length }} {{ t(store.settings.locale, 'sources.sourcesCount') }}</span>
    </div>

    <div v-if="sources.length === 0" class="text-center py-8 bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800">
      <Key class="w-7 h-7 mx-auto text-gray-300 dark:text-gray-600 mb-2" />
      <p class="text-xs text-gray-400">
        {{ t(store.settings.locale, 'sources.noSources') }}
      </p>
    </div>

    <div v-else class="space-y-1.5">
      <div
        v-for="source in sources"
        :key="source.id"
        class="bg-white dark:bg-[#1C1C1E] rounded-xl border border-gray-100 dark:border-neutral-800 p-2 shadow-sm"
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
              class="flex-1 min-w-0 px-1.5 py-0.5 text-xs bg-gray-50 dark:bg-neutral-800 rounded border border-gray-200 dark:border-neutral-700 focus:border-blue-400 outline-none"
              @keyup.enter="saveName"
              @keyup.esc="cancelEdit"
              autofocus
            />
            <button
              @click="saveName"
              class="p-0.5 text-white bg-blue-500 rounded hover:bg-blue-600 transition-colors shrink-0"
            >
              <Check class="w-3.5 h-3.5" />
            </button>
            <button
              @click="cancelEdit"
              class="p-0.5 text-gray-400 bg-gray-100 dark:bg-neutral-800 rounded hover:bg-gray-200 dark:hover:bg-neutral-700 transition-colors shrink-0"
            >
              <X class="w-3.5 h-3.5" />
            </button>
          </template>

          <!-- Normal state -->
          <template v-else>
            <span class="text-xs font-semibold text-gray-800 dark:text-gray-100 truncate flex-1 min-w-0">
              {{ getSourceName(source) }}
            </span>

            <span class="flex items-center gap-0.5 text-xs text-gray-400 dark:text-gray-500 shrink-0">
              <Clock class="w-3.5 h-3.5" />
              {{ formatTime(source.lastSeenMs) }}
            </span>

            <button
              @click="startEdit(source)"
              class="p-0.5 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 transition-colors shrink-0"
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
                  ? 'text-gray-300 dark:text-gray-600 cursor-not-allowed'
                  : 'text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'
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
          </template>
        </div>

        <!-- Row 2: URL -->
        <div class="mt-1 flex items-center gap-1 text-xs text-gray-400 dark:text-gray-500">
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
            class="flex-1 min-w-0 px-1.5 py-0.5 text-xs bg-gray-50 dark:bg-neutral-900/60 text-gray-600 dark:text-gray-300 placeholder:text-gray-300 dark:placeholder:text-gray-600 rounded border border-gray-100 dark:border-neutral-800 focus:border-blue-300 outline-none"
            @input="updateKeyNoteDraft(source, prefix, $event)"
            @blur="saveKeyNote(source, prefix)"
            @keyup.enter="saveKeyNote(source, prefix)"
          />
        </div>

        <!-- Icon picker -->
        <div
          v-if="iconPickerSourceId === source.id"
          class="mt-1.5 p-2 bg-gray-50 dark:bg-neutral-800/80 rounded-lg border border-gray-100 dark:border-neutral-800"
        >
          <input
            v-model="iconSearch"
            type="text"
            :placeholder="t(store.settings.locale, 'sources.searchIcon')"
            class="w-full px-2 py-1 text-[11px] bg-white dark:bg-neutral-900 rounded border border-gray-200 dark:border-neutral-700 focus:border-blue-400 outline-none mb-2"
            autofocus
          />

          <div
            v-for="category in filteredCategories"
            :key="category.label"
            v-show="category.icons.length > 0"
            class="mb-2 last:mb-0"
          >
            <p class="text-[10px] text-gray-400 mb-1 px-0.5">{{ category.label }} · {{ category.icons.length }}</p>
            <div class="grid grid-cols-9 gap-0.5 max-h-[120px] overflow-y-auto">
              <button
                v-for="icon in category.icons"
                :key="icon"
                @click="setSourceIcon(source.id, icon)"
                :class="[
                  'p-1 rounded-md transition-colors flex items-center justify-center',
                  source.icon === icon
                    ? 'bg-blue-100 dark:bg-blue-500/20 ring-1 ring-blue-300 dark:ring-blue-500/30'
                    : 'hover:bg-gray-100 dark:hover:bg-neutral-700'
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
            class="mt-1 text-[10px] text-gray-400 hover:text-red-500 transition-colors"
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
        <div v-if="showMergeDialog" class="fixed inset-0 bg-black/40 backdrop-blur-sm flex items-center justify-center z-50 p-4">
          <div class="bg-white dark:bg-[#1C1C1E] rounded-2xl shadow-xl w-full max-w-xs p-5">
            <h4 class="text-sm font-semibold text-gray-800 dark:text-gray-100 mb-1">
              {{ t(store.settings.locale, 'sources.mergeInto') }}
            </h4>
            <p class="text-xs text-gray-400 dark:text-gray-500 mb-3">
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
                    ? 'border-blue-400 bg-blue-50 dark:bg-blue-500/10'
                    : 'border-gray-100 dark:border-neutral-800 hover:bg-gray-50 dark:hover:bg-neutral-800'
                ]"
              >
                <span
                  class="w-2.5 h-2.5 rounded-full shrink-0"
                  :style="{ backgroundColor: target.color }"
                ></span>
                <span class="text-xs text-gray-700 dark:text-gray-200">{{ getSourceName(target) }}</span>
              </button>
            </div>
            <div class="flex gap-2 mt-4">
              <button
                @click="showMergeDialog = false"
                class="flex-1 py-2 text-[12px] font-medium text-gray-500 bg-gray-100 dark:bg-neutral-800 rounded-xl hover:bg-gray-200 dark:hover:bg-neutral-700 transition-colors"
              >
                {{ t(store.settings.locale, 'common.cancel') }}
              </button>
              <button
                @click="doMerge"
                :disabled="!mergeTargetId"
                :class="[
                  'flex-1 py-2 text-[12px] font-medium text-white rounded-xl transition-colors',
                  mergeTargetId ? 'bg-blue-500 hover:bg-blue-600' : 'bg-blue-300 dark:bg-blue-500/30 cursor-not-allowed'
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
        <div v-if="showDeleteDialog" class="fixed inset-0 bg-black/40 backdrop-blur-sm flex items-center justify-center z-50 p-4">
          <div class="bg-white dark:bg-[#1C1C1E] rounded-2xl shadow-xl w-full max-w-xs p-5">
            <div class="flex flex-col items-center text-center mb-4">
              <div class="w-10 h-10 rounded-full bg-red-50 dark:bg-red-500/10 flex items-center justify-center mb-3">
                <AlertTriangle class="w-5 h-5 text-red-500" />
              </div>
              <h4 class="text-sm font-semibold text-gray-800 dark:text-gray-100">
                {{ t(store.settings.locale, 'sources.delete') }}
              </h4>
              <p class="text-xs text-gray-400 dark:text-gray-500 mt-1">
                {{ t(store.settings.locale, 'sources.deleteConfirm') }}
              </p>
            </div>

            <div v-if="deletingSource" class="flex items-center gap-2 p-2.5 bg-gray-50 dark:bg-neutral-800/80 rounded-xl mb-3">
              <span
                class="w-2.5 h-2.5 rounded-full shrink-0"
                :style="{ backgroundColor: deletingSource.color }"
              ></span>
              <span class="text-xs font-medium text-gray-700 dark:text-gray-200">{{ getSourceName(deletingSource) }}</span>
            </div>

            <label class="flex items-center gap-2.5 p-2.5 bg-red-50 dark:bg-red-500/10 rounded-xl cursor-pointer mb-4">
              <input
                type="checkbox"
                v-model="deleteWithRecords"
                class="w-4 h-4 rounded border-gray-300 text-red-500 focus:ring-red-500"
              />
              <span class="text-xs text-red-600 dark:text-red-400">
                {{ t(store.settings.locale, 'sources.deleteRecordsToo') }}
              </span>
            </label>

            <div class="flex gap-2">
              <button
                @click="showDeleteDialog = false"
                class="flex-1 py-2 text-[12px] font-medium text-gray-500 bg-gray-100 dark:bg-neutral-800 rounded-xl hover:bg-gray-200 dark:hover:bg-neutral-700 transition-colors"
              >
                {{ t(store.settings.locale, 'common.cancel') }}
              </button>
              <button
                @click="doDelete"
                class="flex-1 py-2 text-[12px] font-medium text-white bg-red-500 rounded-xl hover:bg-red-600 transition-colors"
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
