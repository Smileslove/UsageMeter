<script setup lang="ts">
import { computed, ref, onMounted, onUnmounted } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { ChevronDown, LayoutGrid } from 'lucide-vue-next'
import LobeIcon from './LobeIcon.vue'
import { TOOL_LOBE_ICONS } from '../iconConfig'

const store = useMonitorStore()

const isOpen = ref(false)
const dropdownRef = ref<HTMLElement | null>(null)
const iconFailed = ref(false)

const activeFilter = computed(() => store.settings.clientTools.activeToolFilter)
const profiles = computed(() => store.settings.clientTools.profiles)

const showSelector = computed(() => {
  return store.settings.dataSource === 'proxy' && profiles.value.length > 0
})

const getToolName = (tool: string) => {
  const profile = profiles.value.find(p => p.tool === tool)
  return profile?.displayName || tool
}

const getToolIcon = (tool: string) => {
  const profile = profiles.value.find(p => p.tool === tool)
  return profile?.icon || TOOL_LOBE_ICONS[tool] || null
}

const currentProfile = computed(() => {
  if (!activeFilter.value) return null
  return profiles.value.find(p => p.tool === activeFilter.value) || null
})

const currentIcon = computed(() => {
  if (!currentProfile.value) return null
  return currentProfile.value.icon || TOOL_LOBE_ICONS[currentProfile.value.tool] || null
})

const currentLabel = computed(() => {
  if (!activeFilter.value) return t(store.settings.locale, 'tools.all')
  return getToolName(activeFilter.value)
})

const isFiltered = computed(() => activeFilter.value !== null)

const selectTool = async (toolId: string | null) => {
  await store.setActiveToolFilter(toolId)
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
      <LayoutGrid
        v-if="!isFiltered"
        class="w-4 h-4 text-gray-400"
      />
      <LobeIcon
        v-else-if="currentIcon && !iconFailed"
        :slug="currentIcon"
        :size="16"
        @error="iconFailed = true"
      />
      <span v-else class="w-2.5 h-2.5 rounded-full bg-gray-400 shrink-0"></span>
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
        class="absolute top-full right-0 mt-1 w-40 bg-white dark:bg-[#1C1C1E] rounded-xl shadow-lg border border-gray-100 dark:border-neutral-800 overflow-hidden z-50"
      >
        <button
          @click="selectTool(null)"
          :class="[
            'w-full flex items-center gap-2.5 px-3 py-2 text-xs text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors',
            !activeFilter ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium' : 'text-gray-700 dark:text-gray-200'
          ]"
        >
          <LayoutGrid class="w-3.5 h-3.5" />
          {{ t(store.settings.locale, 'tools.all') }}
        </button>

        <div class="border-t border-gray-50 dark:border-neutral-800">
          <button
            v-for="profile in profiles"
            :key="profile.id"
            @click="selectTool(profile.tool)"
            :class="[
              'w-full flex items-center gap-2.5 px-3 py-2 text-xs text-left transition-colors',
              activeFilter === profile.tool
                ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                : profile.enabled
                  ? 'text-gray-700 dark:text-gray-200 hover:bg-gray-50 dark:hover:bg-neutral-800'
                  : 'text-gray-400 dark:text-gray-500'
            ]"
          >
            <LobeIcon
              v-if="getToolIcon(profile.tool)"
              :slug="getToolIcon(profile.tool)!"
              :size="16"
              @error="() => {}"
            />
            <span
              v-else
              class="w-2.5 h-2.5 rounded-full bg-gray-400 shrink-0"
            ></span>
            <span class="truncate">{{ profile.displayName || profile.tool }}</span>
          </button>
        </div>
      </div>
    </Transition>
  </div>
</template>
