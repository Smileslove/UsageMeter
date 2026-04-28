<script setup lang="ts">
import { computed, ref, onMounted, onUnmounted } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { ChevronDown, Globe, HelpCircle } from 'lucide-vue-next'
import LobeIcon from './LobeIcon.vue'

const store = useMonitorStore()

const isOpen = ref(false)
const dropdownRef = ref<HTMLElement | null>(null)
const iconFailed = ref(false)

const activeFilter = computed(() => store.settings.sourceAware.activeSourceFilter)
const sources = computed(() => store.settings.sourceAware.sources)

const showSelector = computed(() => {
  return store.settings.dataSource === 'proxy' && sources.value.length > 0
})

const getSourceName = (source: { id: string; displayName?: string; baseUrl?: string; apiKeyPrefixes: string[] }) => {
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

const currentSource = computed(() => {
  if (!activeFilter.value || activeFilter.value === '__unknown__') return null
  return sources.value.find(s => s.id === activeFilter.value) || null
})

const currentColor = computed(() => {
  if (!currentSource.value) return '#9CA3AF'
  return currentSource.value.color
})

const currentIcon = computed(() => currentSource.value?.icon || null)

const currentLabel = computed(() => {
  if (!activeFilter.value) return t(store.settings.locale, 'sources.all')
  if (activeFilter.value === '__unknown__') return t(store.settings.locale, 'sources.unknown')
  const source = sources.value.find(s => s.id === activeFilter.value)
  return source ? getSourceName(source) : t(store.settings.locale, 'sources.all')
})

const isFiltered = computed(() => activeFilter.value !== null)

const selectSource = async (sourceId: string | null) => {
  await store.setActiveSourceFilter(sourceId)
  isOpen.value = false
}

const toggleDropdown = () => {
  isOpen.value = !isOpen.value
}

const handleClickOutside = (event: MouseEvent) => {
  if (dropdownRef.value && !dropdownRef.value.contains(event.target as Node)) {
    isOpen.value = false
  }
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside)
})

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside)
})
</script>

<template>
  <div v-if="showSelector" ref="dropdownRef" class="relative">
    <button
      @click="toggleDropdown"
      :title="currentLabel"
      :class="[
        'p-1.5 rounded-lg transition-all flex items-center gap-0.5',
        isFiltered
          ? 'bg-blue-50 dark:bg-blue-500/15 ring-2 ring-blue-400/50 dark:ring-blue-400/40'
          : 'text-gray-400 hover:text-gray-700 hover:bg-gray-200/60 dark:hover:text-gray-200 dark:hover:bg-neutral-800/80'
      ]"
    >
      <Globe
        v-if="!isFiltered"
        class="w-4 h-4 text-gray-400"
      />
      <LobeIcon
        v-else-if="currentIcon && !iconFailed"
        :slug="currentIcon"
        :size="16"
        @error="iconFailed = true"
      />
      <span
        v-else
        class="w-2.5 h-2.5 rounded-full shrink-0"
        :style="{ backgroundColor: currentColor }"
      ></span>
      <ChevronDown :class="['w-3 h-3 transition-transform', isOpen && 'rotate-180']" />
    </button>

    <Transition
      enter-active-class="transition ease-out duration-100"
      enter-from-class="transform opacity-0 scale-95"
      enter-to-class="transform opacity-100 scale-100"
      leave-active-class="transition ease-in duration-75"
      leave-from-class="transform opacity-100 scale-100"
      leave-to-class="transform opacity-0 scale-95"
    >
      <div
        v-if="isOpen"
        class="absolute top-full right-0 mt-1 w-44 bg-white dark:bg-[#1C1C1E] rounded-xl shadow-lg border border-gray-100 dark:border-neutral-800 overflow-hidden z-50"
      >
        <button
          @click="selectSource(null)"
          :class="[
            'w-full flex items-center gap-2.5 px-3 py-2 text-xs text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors',
            !activeFilter ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium' : 'text-gray-700 dark:text-gray-200'
          ]"
        >
          <Globe class="w-3.5 h-3.5" />
          {{ t(store.settings.locale, 'sources.all') }}
        </button>

        <div v-if="sources.length > 0" class="border-t border-gray-50 dark:border-neutral-800">
          <button
            v-for="source in sources"
            :key="source.id"
            @click="selectSource(source.id)"
            :class="[
              'w-full flex items-center gap-2.5 px-3 py-2 text-xs text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors',
              activeFilter === source.id ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium' : 'text-gray-700 dark:text-gray-200'
            ]"
          >
            <LobeIcon
              v-if="source.icon"
              :slug="source.icon"
              :size="16"
            />
            <span
              v-else
              class="w-2.5 h-2.5 rounded-full shrink-0"
              :style="{ backgroundColor: source.color }"
            ></span>
            <span class="truncate">{{ getSourceName(source) }}</span>
          </button>
        </div>

        <div class="border-t border-gray-50 dark:border-neutral-800">
          <button
            @click="selectSource('__unknown__')"
            :class="[
              'w-full flex items-center gap-2.5 px-3 py-2 text-xs text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors',
              activeFilter === '__unknown__' ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium' : 'text-gray-700 dark:text-gray-200'
            ]"
          >
            <HelpCircle class="w-3.5 h-3.5" />
            {{ t(store.settings.locale, 'sources.unknown') }}
          </button>
        </div>
      </div>
    </Transition>
  </div>
</template>
