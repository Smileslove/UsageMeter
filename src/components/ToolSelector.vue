<script setup lang="ts">
import { computed, ref, watch, onMounted, onUnmounted } from 'vue'
import { useMonitorStore } from '../stores/monitor'
import { t } from '../i18n'
import { ChevronDown, ChevronRight, LayoutGrid } from 'lucide-vue-next'
import LobeIcon from './LobeIcon.vue'
import { resolveToolLobeIcon } from '../iconConfig'
import { getFamilyForTool, getFamilyHead } from '../toolFamilies'
import { formatToolDisplayName } from '../utils/toolDisplay'

const store = useMonitorStore()

const isOpen = ref(false)
const dropdownRef = ref<HTMLElement | null>(null)
const iconFailed = ref(false)
const expandedFamily = ref<string | null>(null)

const activeFilter = computed(() => store.settings.clientTools.activeToolFilter)
const profiles = computed(() => store.settings.clientTools.profiles)
const visibleProfiles = computed(() => profiles.value)

const showSelector = computed(() => visibleProfiles.value.length > 0)

const getToolName = (tool: string) => {
  return formatToolDisplayName(tool, store.settings.locale, profiles.value)
}

const getToolIcon = (tool: string) => {
  // Try exact match first (so variants like qoder_work get their own icon)
  const exact = resolveToolLobeIcon(tool)
  if (exact) return exact
  // Fallback: use family head's icon
  const headId = getFamilyHead(tool)
  const profile = profiles.value.find(p => p.tool === headId)
  return resolveToolLobeIcon(headId, profile?.icon)
}

const currentProfile = computed(() => {
  if (!activeFilter.value) return null
  const headId = getFamilyHead(activeFilter.value)
  return visibleProfiles.value.find(p => p.tool === headId) || null
})

const currentIcon = computed(() => {
  if (!activeFilter.value) return null
  const exact = resolveToolLobeIcon(activeFilter.value)
  if (exact) return exact
  if (!currentProfile.value) return null
  return resolveToolLobeIcon(currentProfile.value.tool, currentProfile.value.icon)
})

const currentLabel = computed(() => {
  if (!activeFilter.value) return t(store.settings.locale, 'tools.all')
  return getToolName(activeFilter.value)
})

const isFiltered = computed(() => activeFilter.value !== null)

const selectTool = async (toolId: string | null) => {
  await store.setActiveToolFilter(toolId)
  isOpen.value = false
  expandedFamily.value = null
}

const toggleFamily = (headId: string) => {
  expandedFamily.value = expandedFamily.value === headId ? null : headId
}

const toggleDropdown = () => {
  isOpen.value = !isOpen.value
  if (!isOpen.value) expandedFamily.value = null
}

const handleClickOutside = (event: MouseEvent) => {
  if (dropdownRef.value && !dropdownRef.value.contains(event.target as Node)) {
    isOpen.value = false
    expandedFamily.value = null
  }
}

watch(activeFilter, () => { iconFailed.value = false })

onMounted(() => document.addEventListener('click', handleClickOutside))
onUnmounted(() => document.removeEventListener('click', handleClickOutside))
</script>

<template>
  <div v-if="showSelector" ref="dropdownRef" class="relative">
    <!-- 触发按钮 -->
    <button
      @click="toggleDropdown"
      :title="currentLabel"
      :class="[
        'p-1.5 rounded-full transition-all flex items-center gap-0.5',
        isFiltered
          ? 'bg-blue-50/90 text-blue-600 shadow-[0_1px_8px_rgba(37,99,235,0.12)] ring-1 ring-blue-300/45 dark:bg-blue-500/15 dark:text-blue-300 dark:ring-blue-400/30'
          : 'text-slate-400 hover:bg-white/70 hover:text-slate-700 hover:shadow-[0_1px_6px_rgba(15,23,42,0.08)] dark:text-white/36 dark:hover:bg-white/10 dark:hover:text-gray-200 dark:hover:shadow-none'
      ]"
    >
      <LayoutGrid v-if="!isFiltered" class="w-4 h-4" />
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
        class="absolute top-full right-0 mt-1 w-44 bg-white dark:bg-[#1C1C1E] rounded-xl shadow-lg border border-gray-100 dark:border-neutral-800 overflow-hidden z-50"
      >
        <!-- 全部工具 -->
        <button
          @click="selectTool(null)"
          :class="[
            'w-full flex items-center gap-2.5 px-3 py-2 text-xs text-left hover:bg-gray-50 dark:hover:bg-neutral-800 transition-colors',
            !activeFilter
              ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
              : 'text-gray-700 dark:text-gray-200'
          ]"
        >
          <LayoutGrid class="w-3.5 h-3.5 shrink-0" />
          {{ t(store.settings.locale, 'tools.all') }}
        </button>

        <div class="border-t border-gray-50 dark:border-neutral-800">
          <template v-for="profile in visibleProfiles" :key="profile.id">
            <!-- 判断是否为家族 head -->
            <template v-if="getFamilyForTool(profile.tool)?.head === profile.tool">
              <!-- 家族 head 行：点击展开/收起，同时作为「家族整体过滤」选项 -->
              <div class="flex items-stretch">
                <!-- 左侧：选中家族整体 -->
                <button
                  @click="selectTool(profile.tool)"
                  :class="[
                    'flex-1 flex items-center gap-2.5 pl-3 pr-1 py-2 text-xs text-left transition-colors',
                    activeFilter === profile.tool || getFamilyForTool(profile.tool)?.members.includes(activeFilter ?? '')
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
                  <span v-else class="w-2.5 h-2.5 rounded-full bg-gray-400 shrink-0"></span>
                  <span class="truncate">{{ formatToolDisplayName(profile.tool, store.settings.locale, profiles) }}</span>
                </button>
                <!-- 右侧：展开/收起箭头 -->
                <button
                  @click.stop="toggleFamily(profile.tool)"
                  :class="[
                    'flex items-center justify-center w-7 transition-colors shrink-0',
                    expandedFamily === profile.tool
                      ? 'text-blue-500 dark:text-blue-400'
                      : 'text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300'
                  ]"
                  :title="expandedFamily === profile.tool ? t(store.settings.locale, 'tools.collapseVariants') : t(store.settings.locale, 'tools.expandVariants')"
                >
                  <ChevronRight
                    :class="['w-3 h-3 transition-transform duration-150', expandedFamily === profile.tool && 'rotate-90']"
                  />
                </button>
              </div>

              <!-- 二级子条目 -->
              <template v-if="expandedFamily === profile.tool">
                <!-- 子菜单「全部」：等同于选中家族 head -->
                <button
                  @click="selectTool(profile.tool)"
                  :class="[
                    'w-full flex items-center gap-2 pl-7 pr-3 py-1.5 text-[11px] text-left transition-colors',
                    activeFilter === profile.tool || !getFamilyForTool(profile.tool)!.members.includes(activeFilter ?? '')
                      ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                      : 'text-gray-500 dark:text-gray-400 hover:bg-gray-50 dark:hover:bg-neutral-800'
                  ]"
                >
                  <LayoutGrid class="w-3 h-3 shrink-0" />
                  <span class="truncate">{{ t(store.settings.locale, 'tools.familyAll') }}</span>
                </button>
                <!-- 各变体 -->
                <button
                  v-for="memberId in getFamilyForTool(profile.tool)!.members"
                  :key="memberId"
                  @click="selectTool(memberId)"
                  :class="[
                    'w-full flex items-center gap-2 pl-7 pr-3 py-1.5 text-[11px] text-left transition-colors',
                    activeFilter === memberId
                      ? 'bg-blue-50 dark:bg-blue-500/10 text-blue-600 dark:text-blue-400 font-medium'
                      : 'text-gray-500 dark:text-gray-400 hover:bg-gray-50 dark:hover:bg-neutral-800'
                  ]"
                >
                  <LobeIcon
                    v-if="getToolIcon(memberId)"
                    :slug="getToolIcon(memberId)!"
                    :size="13"
                    @error="() => {}"
                  />
                  <span class="truncate">
                    {{ getFamilyForTool(profile.tool)!.variantLabels[memberId] ?? formatToolDisplayName(memberId, store.settings.locale, profiles) }}
                  </span>
                </button>
              </template>
            </template>

            <!-- 普通（非家族）条目 -->
            <template v-else-if="!getFamilyForTool(profile.tool)">
              <button
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
                <span v-else class="w-2.5 h-2.5 rounded-full bg-gray-400 shrink-0"></span>
                <span class="truncate">{{ formatToolDisplayName(profile.tool, store.settings.locale, profiles) }}</span>
              </button>
            </template>
          </template>
        </div>
      </div>
    </Transition>
  </div>
</template>
