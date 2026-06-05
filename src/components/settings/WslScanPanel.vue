<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useMonitorStore } from '../../stores/monitor'
import { t } from '../../i18n'
import SettingsSwitch from './SettingsSwitch.vue'

const store = useMonitorStore()

// ── state ──

const wsEnabled = ref(store.settings.wslScan?.enabled ?? false)
const wsExtraRoots = ref((store.settings.wslScan?.extraRoots ?? []).join(', '))
const wsAdvanced = ref(false)
const wsSavedFlash = ref(false)
const availableDistros = ref<string[]>([])
const distroLoading = ref(false)

// The distros list in settings = enabled distros. Empty = all enabled (auto-detect mode).
const storedDistros = computed(() => store.settings.wslScan?.distros ?? [])

/** Is a given distro enabled for scanning? */
function distroEnabled(name: string): boolean {
  if (!wsEnabled.value) return false
  if (storedDistros.value.length === 0) return true // auto-detect mode — all on
  return storedDistros.value.includes(name)
}

const anyDistroOff = computed(() =>
  wsEnabled.value && availableDistros.value.some((d) => !distroEnabled(d)),
)

// ── fetch distros ──

async function refreshDistros() {
  distroLoading.value = true
  try {
    availableDistros.value = await invoke<string[]>('list_wsl_distros')
  } catch {
    availableDistros.value = []
  } finally {
    distroLoading.value = false
  }
}

onMounted(() => {
  if (wsEnabled.value) refreshDistros()
})

watch(wsEnabled, (on) => {
  if (on && availableDistros.value.length === 0) refreshDistros()
})

// sync local ← store
watch(
  () => store.settings.wslScan,
  (value) => {
    if (!value) return
    wsEnabled.value = value.enabled ?? false
    wsExtraRoots.value = (value.extraRoots ?? []).join(', ')
  },
  { deep: true },
)

// ── dirty check (only for extra roots / advanced) ──

const advancedDirty = computed(() => {
  const cur = store.settings.wslScan
  if (!cur) return wsExtraRoots.value.trim() !== ''
  return wsExtraRoots.value !== (cur.extraRoots ?? []).join(', ')
})

// ── helpers ──

function splitList(input: string): string[] {
  return input
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
}

// ── actions ──

async function toggleMaster() {
  wsEnabled.value = !wsEnabled.value
  if (wsEnabled.value && availableDistros.value.length === 0) {
    await refreshDistros()
  }
  if (!wsEnabled.value) wsAdvanced.value = false
  await persist()
}

/** Toggle a single distro on/off. */
async function toggleDistro(name: string) {
  const cur = storedDistros.value.length === 0 ? [...availableDistros.value] : [...storedDistros.value]
  let next: string[]
  if (cur.includes(name)) {
    next = cur.filter((d) => d !== name)
  } else {
    next = [...cur, name]
  }
  // If the new set covers all available distros, store [] (auto-detect mode).
  const all = new Set(availableDistros.value)
  if (all.size > 0 && next.length >= all.size && next.every((d) => all.has(d))) {
    next = []
  }
  store.settings.wslScan = { ...store.settings.wslScan!, distros: next, enabled: wsEnabled.value, extraRoots: splitList(wsExtraRoots.value) }
  await persist()
}

/** Select all distros → distros = [] (auto-detect mode). */
async function selectAll() {
  store.settings.wslScan = { ...store.settings.wslScan!, distros: [], enabled: wsEnabled.value, extraRoots: splitList(wsExtraRoots.value) }
  await persist()
}

async function saveAdvanced() {
  wsSavedFlash.value = true
  setTimeout(() => (wsSavedFlash.value = false), 1200)
  await persist()
}

async function persist() {
  try {
    await store.saveSettings()
  } catch {
    // rollback handled by watch
  }
}
</script>

<template>
  <div class="py-2 px-4">
    <!-- 头部：标题 + 主开关 -->
    <div class="flex items-center justify-between gap-3">
      <div class="min-w-0 flex-1">
        <div class="text-[13px] text-gray-700 dark:text-gray-200">
          {{ t(store.settings.locale, 'settings.wslScanTitle') }}
        </div>
        <div class="mt-0.5 text-[10px] text-gray-400">
          <template v-if="!wsEnabled">{{ t(store.settings.locale, 'settings.wslScanDesc') }}</template>
          <template v-else-if="distroLoading">…</template>
          <template v-else-if="availableDistros.length === 0">{{ t(store.settings.locale, 'settings.wslScanNoDistros') }}</template>
          <template v-else-if="storedDistros.length === 0">{{ t(store.settings.locale, 'settings.wslScanChipAuto') }}</template>
          <template v-else>{{ t(store.settings.locale, 'settings.wslScanChipCustom', { n: storedDistros.length, total: availableDistros.length }) }}</template>
        </div>
      </div>
      <SettingsSwitch :checked="wsEnabled" @toggle="toggleMaster" />
    </div>

    <!-- 开启后的发行版列表 -->
    <div v-if="wsEnabled && availableDistros.length > 0" class="mt-3 space-y-1.5">
      <div
        v-for="d in availableDistros"
        :key="d"
        class="flex items-center justify-between rounded-lg px-2 py-1.5 transition-colors hover:bg-gray-50 dark:hover:bg-neutral-800/50"
      >
        <div class="flex items-center gap-2 min-w-0">
          <span class="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded bg-[#0DB97D]/10 text-[11px] font-medium text-[#0DB97D]">
            {{ d.charAt(0).toUpperCase() }}
          </span>
          <span class="truncate text-[12px] text-gray-700 dark:text-gray-200">{{ d }}</span>
        </div>
        <SettingsSwitch :checked="distroEnabled(d)" @toggle="toggleDistro(d)" />
      </div>

      <!-- 操作栏 -->
      <div class="flex items-center justify-between pt-2">
        <div class="flex items-center gap-2">
          <button
            v-if="anyDistroOff"
            type="button"
            class="text-[11px] text-[var(--theme-accent-primary)] hover:underline"
            @click="selectAll"
          >
            {{ t(store.settings.locale, 'settings.wslScanSelectAll') }}
          </button>
          <button
            type="button"
            class="inline-flex items-center gap-1 text-[11px] text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
            @click="refreshDistros"
          >
            <span class="text-[13px] leading-none" :class="{ 'animate-spin': distroLoading }">↻</span>
            {{ t(store.settings.locale, 'settings.wslScanRefresh') }}
          </button>
        </div>

        <button
          type="button"
          class="text-[11px] text-[var(--theme-accent-primary)] hover:underline"
          @click="wsAdvanced = !wsAdvanced"
        >
          {{ wsAdvanced
            ? t(store.settings.locale, 'settings.wslScanCollapse')
            : t(store.settings.locale, 'settings.wslScanAdvanced') }}
        </button>
      </div>

      <p v-if="availableDistros.length > 0 && storedDistros.length === 0" class="text-[10px] text-gray-400 leading-relaxed">
        {{ t(store.settings.locale, 'settings.wslScanEnabledHint') }}
      </p>
    </div>

    <!-- 无发行版时的提示 + 刷新 -->
    <div v-else-if="wsEnabled && !distroLoading && availableDistros.length === 0" class="mt-3 space-y-3">
      <p class="text-[11px] text-gray-400">{{ t(store.settings.locale, 'settings.wslScanNoDistros') }}</p>
      <div class="flex items-center gap-2">
        <button
          type="button"
          class="inline-flex items-center gap-1 text-[11px] text-[var(--theme-accent-primary)] hover:underline"
          @click="refreshDistros"
        >
          <span class="text-[13px] leading-none" :class="{ 'animate-spin': distroLoading }">↻</span>
          {{ t(store.settings.locale, 'settings.wslScanRefresh') }}
        </button>
        <button
          type="button"
          class="text-[11px] text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
          @click="wsAdvanced = !wsAdvanced"
        >
          {{ t(store.settings.locale, 'settings.wslScanAdvanced') }}
        </button>
      </div>
    </div>

    <!-- 高级选项 -->
    <div v-if="wsEnabled && wsAdvanced" class="mt-3 space-y-2.5">
      <div>
        <label class="mb-1 block text-[11px] text-gray-500 dark:text-gray-400">
          {{ t(store.settings.locale, 'settings.wslScanExtraRootsLabel') }}
        </label>
        <input
          v-model="wsExtraRoots"
          type="text"
          :placeholder="t(store.settings.locale, 'settings.wslScanExtraRootsPlaceholder')"
          class="h-8 w-full rounded-lg border border-gray-200 bg-white px-3 text-[12px] text-gray-700 placeholder:text-gray-300 focus:outline-none focus:ring-1 focus:ring-[var(--theme-ring-focus)] dark:border-neutral-700 dark:bg-neutral-800 dark:text-gray-200 dark:placeholder:text-neutral-600"
        />
        <p class="mt-1 text-[10px] text-gray-400">
          {{ t(store.settings.locale, 'settings.wslScanExtraRootsHint') }}
        </p>
        <div class="mt-2 flex justify-end">
          <button
            type="button"
            class="h-7 rounded-lg bg-[var(--theme-accent-primary)] px-3 text-[11px] text-[var(--theme-accent-contrast)] transition-opacity hover:opacity-90 disabled:opacity-40"
            :disabled="!advancedDirty"
            @click="saveAdvanced"
          >
            <span v-if="wsSavedFlash">✓</span>
            <span v-else>{{ t(store.settings.locale, 'settings.wslScanSave') }}</span>
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
