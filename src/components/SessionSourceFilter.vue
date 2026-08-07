<script setup lang="ts">
import { ChevronDown, LayoutGrid } from 'lucide-vue-next'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { t } from '../i18n'
import { TOOL_LOBE_ICONS } from '../iconConfig'
import { useMonitorStore } from '../stores/monitor'
import { getFamilyForTool, getFamilyHead } from '../toolFamilies'
import { formatToolDisplayName, formatToolFilterDisplayName } from '../utils/toolDisplay'
import LobeIcon from './LobeIcon.vue'

interface SourceOption {
  key: string
  tool: string | null
  label: string
  icon: string | null
  familyHead: string | null
  children: SourceOption[]
}

const props = defineProps<{
  modelValue: string | null
  sourceTools: ReadonlySet<string>
}>()

const emit = defineEmits<{
  'update:modelValue': [tool: string | null]
}>()

const store = useMonitorStore()
const expandedFamily = ref<string | null>(null)
const filterDropdownOpen = ref(false)
const filterDropdownRef = ref<HTMLElement | null>(null)

const sourceOptions = computed<SourceOption[]>(() => {
  const profiles = store.settings.clientTools.profiles
    .filter(profile => props.sourceTools.has(profile.tool))
    .sort((a, b) => {
      if (a.tool === 'claude_code') return -1
      if (b.tool === 'claude_code') return 1
      return (a.displayName || a.tool).localeCompare(b.displayName || b.tool)
    })

  const items: SourceOption[] = [
    { key: '__all__', tool: null, label: t(store.settings.locale, 'tools.all'), icon: null, familyHead: null, children: [] }
  ]

  for (const profile of profiles) {
    const family = getFamilyForTool(profile.tool)
    if (family && profile.tool === family.head) {
      const subItems: SourceOption[] = family.members.map(memberId => ({
        key: memberId,
        tool: memberId,
        label: formatToolDisplayName(memberId, store.settings.locale, profiles),
        icon: TOOL_LOBE_ICONS[memberId] || null,
        familyHead: null,
        children: [],
      }))
      items.push({
        key: profile.tool,
        tool: profile.tool,
        label: formatToolFilterDisplayName(profile.tool, store.settings.locale, profiles),
        icon: profile.icon || TOOL_LOBE_ICONS[profile.tool] || null,
        familyHead: family.head,
        children: subItems,
      })
    } else if (!family) {
      items.push({
        key: profile.tool,
        tool: profile.tool,
        label: formatToolDisplayName(profile.tool, store.settings.locale, profiles),
        icon: profile.icon || TOOL_LOBE_ICONS[profile.tool] || null,
        familyHead: null,
        children: [],
      })
    }
  }

  return items
})

const menuSourceOptions = computed(() => sourceOptions.value.filter(option => option.key !== '__all__'))

const activeFamilyHead = computed(() => (
  props.modelValue ? getFamilyHead(props.modelValue) : null
))

const currentSourceOption = computed<SourceOption>(() => {
  if (props.modelValue === null) return sourceOptions.value[0]

  for (const option of sourceOptions.value) {
    if (option.tool === props.modelValue) return option
    const child = option.children.find(item => item.tool === props.modelValue)
    if (child) return child
  }

  return sourceOptions.value[0]
})

const closeFilterDropdown = () => {
  filterDropdownOpen.value = false
  expandedFamily.value = null
}

const toggleFilterDropdown = () => {
  const next = !filterDropdownOpen.value
  filterDropdownOpen.value = next
  if (!next) {
    expandedFamily.value = null
    return
  }
  expandedFamily.value = activeFamilyHead.value && activeFamilyHead.value !== props.modelValue
    ? activeFamilyHead.value
    : null
}

const selectSourceTool = (tool: string | null) => {
  emit('update:modelValue', tool)
  closeFilterDropdown()
}

const toggleFamilyMenu = (headId: string) => {
  expandedFamily.value = expandedFamily.value === headId ? null : headId
}

const handleFilterClickOutside = (event: MouseEvent) => {
  if (!filterDropdownRef.value?.contains(event.target as Node)) {
    closeFilterDropdown()
  }
}

onMounted(() => {
  document.addEventListener('click', handleFilterClickOutside)
})

onUnmounted(() => {
  document.removeEventListener('click', handleFilterClickOutside)
})
</script>

<template>
  <div ref="filterDropdownRef" class="tool-filter">
    <button
      type="button"
      class="tool-filter__trigger"
      :class="{ 'tool-filter__trigger--open': filterDropdownOpen }"
      :aria-expanded="filterDropdownOpen"
      :title="currentSourceOption.label"
      @click="toggleFilterDropdown"
    >
      <LayoutGrid v-if="!currentSourceOption.icon" class="h-3.5 w-3.5 shrink-0" />
      <LobeIcon v-else :slug="currentSourceOption.icon" :size="14" @error="() => {}" />
      <span class="tool-filter__current">{{ currentSourceOption.label }}</span>
      <ChevronDown :class="['h-3.5 w-3.5 shrink-0 transition-transform duration-150', filterDropdownOpen && 'rotate-180']" />
    </button>

    <Transition
      enter-active-class="transition ease-out duration-120"
      enter-from-class="transform opacity-0 -translate-y-1"
      enter-to-class="transform opacity-100 translate-y-0"
      leave-active-class="transition ease-in duration-100"
      leave-from-class="transform opacity-100 translate-y-0"
      leave-to-class="transform opacity-0 -translate-y-1"
    >
      <div v-if="filterDropdownOpen" class="tool-filter__menu">
        <button
          type="button"
          class="tool-filter__menu-item"
          :class="{ 'tool-filter__menu-item--on': modelValue === null }"
          @click="selectSourceTool(null)"
        >
          <LayoutGrid class="h-3.5 w-3.5 shrink-0" />
          <span class="truncate">{{ t(store.settings.locale, 'tools.all') }}</span>
        </button>

        <div class="tool-filter__menu-list">
          <template v-for="option in menuSourceOptions" :key="option.key">
            <div class="tool-filter__menu-row">
              <button
                type="button"
                class="tool-filter__menu-item tool-filter__menu-item--family"
                :class="{ 'tool-filter__menu-item--on': modelValue === option.tool }"
                @click="selectSourceTool(option.tool)"
              >
                <LobeIcon v-if="option.icon" :slug="option.icon" :size="14" @error="() => {}" />
                <LayoutGrid v-else class="h-3.5 w-3.5 shrink-0" />
                <span class="truncate">{{ option.label }}</span>
              </button>
              <button
                v-if="option.familyHead"
                type="button"
                class="tool-filter__menu-expand"
                :title="expandedFamily === option.familyHead ? t(store.settings.locale, 'tools.collapseVariants') : t(store.settings.locale, 'tools.expandVariants')"
                @click.stop="toggleFamilyMenu(option.familyHead)"
              >
                <ChevronDown :class="['h-3 w-3 transition-transform duration-150', expandedFamily === option.familyHead && 'rotate-180']" />
              </button>
            </div>

            <div v-if="option.familyHead && expandedFamily === option.familyHead" class="tool-filter__submenu">
              <button
                type="button"
                class="tool-filter__menu-item tool-filter__menu-item--child"
                :class="{ 'tool-filter__menu-item--on': modelValue === option.tool }"
                @click="selectSourceTool(option.tool)"
              >
                <LayoutGrid class="h-3 w-3 shrink-0" />
                <span class="truncate">{{ t(store.settings.locale, 'tools.familyAll') }}</span>
              </button>
              <button
                v-for="child in option.children"
                :key="child.key"
                type="button"
                class="tool-filter__menu-item tool-filter__menu-item--child"
                :class="{ 'tool-filter__menu-item--on': modelValue === child.tool }"
                @click="selectSourceTool(child.tool)"
              >
                <LobeIcon v-if="child.icon" :slug="child.icon" :size="12" @error="() => {}" />
                <LayoutGrid v-else class="h-3 w-3 shrink-0" />
                <span class="truncate">{{ child.label }}</span>
              </button>
            </div>
          </template>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.tool-filter {
  position: relative;
  padding: 2px 0 4px;
}

.tool-filter__trigger {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  border-radius: 10px;
  border: 1px solid var(--theme-border-default);
  background: var(--theme-bg-elevated);
  text-align: left;
  cursor: pointer;
  outline: none;
  transition: border-color 0.18s ease, background 0.18s ease, box-shadow 0.18s ease;
}

.tool-filter__trigger:hover,
.tool-filter__trigger--open {
  border-color: var(--theme-border-strong);
}

.tool-filter__current {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.3;
  color: var(--theme-text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tool-filter__menu {
  position: absolute;
  top: calc(100% + 5px);
  left: 0;
  right: 0;
  z-index: 40;
  max-height: 216px;
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: 4px;
  border-radius: 12px;
  border: 1px solid var(--theme-border-default);
  background: var(--theme-bg-overlay);
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.16);
  backdrop-filter: blur(14px);
}

.tool-filter__menu-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.tool-filter__menu-row {
  display: flex;
  align-items: stretch;
  gap: 4px;
}

.tool-filter__menu-item {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
  padding: 5px 8px;
  border-radius: 8px;
  border: none;
  background: transparent;
  font-size: 11px;
  font-weight: 600;
  line-height: 1.3;
  color: var(--theme-text-secondary);
  text-align: left;
  cursor: pointer;
  transition: background 0.14s ease, color 0.14s ease;
}

.tool-filter__menu-item:hover {
  background: var(--theme-bg-hover);
  color: var(--theme-text-primary);
}

.tool-filter__menu-item--on {
  background: var(--theme-accent-soft);
  color: var(--theme-accent-primary);
}

.tool-filter__menu-item--family {
  flex: 1;
}

.tool-filter__menu-expand {
  width: 28px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--theme-text-tertiary);
  cursor: pointer;
  transition: background 0.14s ease, color 0.14s ease;
}

.tool-filter__menu-expand:hover {
  background: var(--theme-bg-hover);
  color: var(--theme-text-primary);
}

.tool-filter__submenu {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 1px 0 3px;
  padding-left: 12px;
  position: relative;
}

.tool-filter__submenu::before {
  content: '';
  position: absolute;
  left: 4px;
  top: 2px;
  bottom: 2px;
  width: 1px;
  background: color-mix(in srgb, var(--theme-accent-primary) 18%, var(--theme-border-subtle));
}

.tool-filter__menu-item--child {
  font-size: 10px;
  padding: 5px 8px;
}
</style>
