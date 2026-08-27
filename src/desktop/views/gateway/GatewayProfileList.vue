<script setup lang="ts">
/**
 * 桌面网关页左侧 profile 列表（320px）：
 * 渲染 profiles，提供选择 / 新增 / 启用切换 / 删除 / 复制地址 / 复制本地 Key 的行内操作。
 * 所有操作仅上抛事件，由父组件执行（涉及 confirm、feedback、status 刷新等编排逻辑）。
 */
import { computed } from 'vue'
import { KeyRound, Link2, Plus, RadioTower, Trash2 } from 'lucide-vue-next'
import { useMonitorStore } from '../../../stores/monitor'
import { t } from '../../../i18n'
import { compactProtocolLabel } from './protocolOptions'
import type { GatewayProfile, GatewayUpstreamKey } from '../../../types'
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue'

defineProps<{
  profiles: GatewayProfile[]
  selectedId: string | null
}>()

const emit = defineEmits<{
  (e: 'select', profile: GatewayProfile): void
  (e: 'create'): void
  (e: 'toggle', profile: GatewayProfile): void
  (e: 'delete', profile: GatewayProfile): void
  (e: 'copyAddress', profile: GatewayProfile): void
  (e: 'copyApiKey', profile: GatewayProfile): void
}>()

const store = useMonitorStore()
const locale = computed(() => store.settings.locale)

// profile 列表项：最后活动（本地/上游 Key 最近使用时间；无则显示“从未”）
const lastActiveOf = (profile: GatewayProfile): number | null => {
  const times: number[] = []
  for (const key of profile.localKeys) {
    if (key.lastUsedAtMs) times.push(key.lastUsedAtMs)
  }
  for (const key of profile.upstreamKeys) {
    if ('lastUsedAtMs' in key && (key as GatewayUpstreamKey & { lastUsedAtMs?: number }).lastUsedAtMs) {
      times.push((key as GatewayUpstreamKey & { lastUsedAtMs?: number }).lastUsedAtMs as number)
    }
  }
  return times.length > 0 ? Math.max(...times) : null
}
const formatLastActive = (epochMs: number | null) => {
  if (!epochMs) return t(locale.value, 'desktop.gateway.lastActiveNever')
  const diffMs = Date.now() - epochMs
  const minutes = Math.floor(diffMs / 60000)
  if (minutes < 1) return t(locale.value, 'common.justNow')
  if (minutes < 60) return t(locale.value, 'desktop.gateway.lastActiveMinutes', { count: minutes })
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return t(locale.value, 'desktop.gateway.lastActiveHours', { count: hours })
  const days = Math.floor(hours / 24)
  return t(locale.value, 'desktop.gateway.lastActiveDays', { count: days })
}

// 行键盘操作：Enter/Space 选中 profile（与 click 一致；行内操作按钮聚焦时
// keydown 冒泡被 target !== currentTarget 检查排除，避免误触选择）
const handleRowKeydown = (event: KeyboardEvent, profile: GatewayProfile) => {
  if (event.target !== event.currentTarget) return
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    emit('select', profile)
  }
}
</script>

<template>
  <aside class="flex w-72 shrink-0 flex-col overflow-hidden rounded-lg border" style="background: var(--theme-surface-gradient)">
    <div class="flex shrink-0 items-center justify-between border-b border-[var(--theme-border-default)] px-3 py-2">
      <div class="flex items-baseline gap-2">
        <h2 class="text-xs font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'gateway.profiles') }}</h2>
        <p class="text-xs text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.profileCount', { count: profiles.length }) }}</p>
      </div>
      <button
        type="button"
        class="theme-icon-button rounded-lg p-1.5"
        :title="t(locale, 'gateway.newProfile')"
        :aria-label="t(locale, 'gateway.newProfile')"
        @click="emit('create')"
      >
        <Plus class="h-4 w-4" aria-hidden="true" />
      </button>
    </div>

    <div v-if="profiles.length === 0" class="flex flex-1 flex-col items-center justify-center px-4 py-10 text-center">
      <RadioTower class="h-6 w-6 text-[var(--theme-text-quaternary)]" aria-hidden="true" />
      <p class="mt-2 text-xs font-semibold text-[var(--theme-text-secondary)]">{{ t(locale, 'gateway.emptyTitle') }}</p>
      <p class="mx-auto mt-1 max-w-[220px] text-xs leading-snug text-[var(--theme-text-tertiary)]">{{ t(locale, 'gateway.emptyBody') }}</p>
    </div>

    <div v-else class="min-h-0 flex-1 overflow-y-auto">
      <article
        v-for="profile in profiles"
        :key="profile.id"
        tabindex="0"
        class="relative cursor-pointer border-b border-[var(--theme-border-subtle)] px-3 py-2.5 transition-colors last:border-0 hover:bg-[var(--theme-bg-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-ring-focus)]"
        :class="selectedId === profile.id ? 'bg-[var(--theme-accent-soft)]' : ''"
        @click="emit('select', profile)"
        @keydown="handleRowKeydown($event, profile)"
      >
        <span
          v-if="selectedId === profile.id"
          class="absolute left-0 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-r-full"
          style="background: var(--theme-accent-primary)"
        ></span>
        <div class="flex items-start gap-2">
          <div class="min-w-0 flex-1">
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="truncate text-xs font-semibold text-[var(--theme-text-primary)]">{{ profile.name }}</span>
              <span class="shrink-0 rounded border border-[var(--theme-border-default)] bg-[var(--theme-bg-hover)] px-1 py-px text-xs font-medium text-[var(--theme-text-tertiary)]">{{ compactProtocolLabel(locale, profile.protocol) }}</span>
            </div>
            <p class="mt-1 flex items-center gap-1.5 text-xs text-[var(--theme-text-tertiary)]">
              <span class="inline-flex items-center gap-1">
                <span class="h-1.5 w-1.5 rounded-full" :class="profile.enabled ? 'bg-emerald-500' : 'bg-[var(--theme-border-strong)]'"></span>
                {{ profile.enabled ? t(locale, 'desktop.gateway.profileEnabled') : t(locale, 'desktop.gateway.profileDisabled') }}
              </span>
              <span>·</span>
              <span>{{ t(locale, 'desktop.gateway.modelCount', { count: profile.upstreamModels.length }) }}</span>
            </p>
            <p class="mt-0.5 text-xs text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.gateway.lastActivity') }} {{ formatLastActive(lastActiveOf(profile)) }}</p>
          </div>
          <div class="flex shrink-0 items-center gap-0.5">
            <button type="button" class="theme-icon-button rounded-md p-1" :title="t(locale, 'gateway.copyAddress')" :aria-label="t(locale, 'gateway.copyAddress')" @click.stop="emit('copyAddress', profile)"><Link2 class="h-3.5 w-3.5" aria-hidden="true" /></button>
            <button type="button" class="theme-icon-button rounded-md p-1" :title="t(locale, 'gateway.copyLocalKey')" :aria-label="t(locale, 'gateway.copyLocalKey')" :disabled="profile.localKeys.length === 0" @click.stop="emit('copyApiKey', profile)"><KeyRound class="h-3.5 w-3.5" aria-hidden="true" /></button>
            <button type="button" class="theme-icon-button rounded-md p-1" :title="t(locale, 'gateway.delete')" :aria-label="t(locale, 'gateway.delete')" @click.stop="emit('delete', profile)"><Trash2 class="h-3.5 w-3.5 text-red-500" aria-hidden="true" /></button>
            <SettingsSwitch compact :checked="profile.enabled" :aria-label="t(locale, 'gateway.enabled')" @toggle="emit('toggle', profile)" />
          </div>
        </div>
        <p v-if="profile.credentialRecovery.upstreamKeyRequired" class="mt-1 text-xs text-amber-600 dark:text-amber-300">{{ t(locale, 'gateway.upstreamKeyRecoveryNotice') }}</p>
        <p v-if="profile.credentialRecovery.localKeyRotationRecommended" class="mt-1 text-xs text-amber-600 dark:text-amber-300">{{ t(locale, 'gateway.localKeyRecoveryNotice') }}</p>
      </article>
    </div>
  </aside>
</template>
