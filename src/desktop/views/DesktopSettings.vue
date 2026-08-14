<script setup lang="ts">
/**
 * 桌面主窗口「设置」页（设计文档第 11 章）。
 * 左侧二级目录（应用/外观/数据源/会话与隐私/来源与计费/网关与代理/网络/同步/存储与维护/关于与更新），
 * 右侧分隔区块（label + description + control 网格，不套卡片）。
 * 复用现有组件：GeneralSettingsPanel / ThemeSelector / ApiSourceList / DataNavigationPanel /
 * ModelPricingSettings / CurrencySettings / ProxyControlPanel / CcSwitchCompatPanel /
 * NetworkProxyPanel / SyncSettingsPanel / LocalCachePanel / LocalCacheManagementPanel。
 * 各组件自行保存（store.saveSettings）；「会话与隐私」深度索引为 M2 功能，显示 i18n 占位说明。
 */
import { computed, onMounted, ref, watch } from 'vue'
import { ExternalLink, RefreshCw, Trash2 } from 'lucide-vue-next'
import { useMonitorStore } from '../../stores/monitor'
import { useUpdaterStore } from '../../stores/updater'
import { useDesktopNavigationStore } from '../stores/desktopNavigation'
import { t } from '../../i18n'
import { quitApplication } from '../../utils/appExit'
import { purgeSessionActivityContent } from '../../api/activityApi'
import GeneralSettingsPanel from '../../components/settings/GeneralSettingsPanel.vue'
import DataNavigationPanel from '../../components/settings/DataNavigationPanel.vue'
import LocalCachePanel from '../../components/settings/LocalCachePanel.vue'
import LocalCacheManagementPanel from '../../components/settings/LocalCacheManagementPanel.vue'
import NetworkProxyPanel from '../../components/settings/NetworkProxyPanel.vue'
import SyncSettingsPanel from '../../components/settings/SyncSettingsPanel.vue'
import ProxyControlPanel from '../../components/settings/ProxyControlPanel.vue'
import CcSwitchCompatPanel from '../../components/settings/CcSwitchCompatPanel.vue'
import ConfirmDialog from '../../components/settings/ConfirmDialog.vue'
import ThemeSelector from '../../components/ThemeSelector.vue'
import ApiSourceList from '../../components/ApiSourceList.vue'
import ModelPricingSettings from '../../components/ModelPricingSettings.vue'
import CurrencySettings from '../../components/CurrencySettings.vue'

const store = useMonitorStore()
const updaterStore = useUpdaterStore()
const nav = useDesktopNavigationStore()
const locale = computed(() => store.settings.locale)

// —— 左侧二级目录 ——
type SettingsSection = 'app' | 'appearance' | 'dataSources' | 'privacy' | 'pricing' | 'gateway' | 'network' | 'sync' | 'storage' | 'about'
const sections: Array<{ id: SettingsSection; labelKey: string }> = [
  { id: 'app', labelKey: 'desktop.settings.navApp' },
  { id: 'appearance', labelKey: 'desktop.settings.navAppearance' },
  { id: 'dataSources', labelKey: 'desktop.settings.navDataSources' },
  { id: 'privacy', labelKey: 'desktop.settings.navPrivacy' },
  { id: 'pricing', labelKey: 'desktop.settings.navPricing' },
  { id: 'gateway', labelKey: 'desktop.settings.navGateway' },
  { id: 'network', labelKey: 'desktop.settings.navNetwork' },
  { id: 'sync', labelKey: 'desktop.settings.navSync' },
  { id: 'storage', labelKey: 'desktop.settings.navStorage' },
  { id: 'about', labelKey: 'desktop.settings.navAbout' }
]
const activeSection = ref<SettingsSection>('app')

// —— 来源与计费内嵌子视图（ModelPricingSettings / CurrencySettings 是带 @back 的全页组件） ——
type PricingSubView = 'main' | 'model-pricing' | 'currency'
const pricingSubView = ref<PricingSubView>('main')

// —— 深度索引（M2/M3：radio 生效；fulltext 支持会话内与跨会话搜索） ——
type DeepIndexLevel = 'off' | 'structured' | 'fulltext' | 'ondemand'
const deepIndexLevel = ref<DeepIndexLevel>(
  (store.settings.deepIndexLevel as DeepIndexLevel) || 'off'
)
const deepIndexOptions: Array<{ id: DeepIndexLevel; labelKey: string; descKey: string }> = [
  { id: 'off', labelKey: 'desktop.settings.deepIndexOff', descKey: 'desktop.settings.deepIndexOffDesc' },
  { id: 'structured', labelKey: 'desktop.settings.deepIndexStructured', descKey: 'desktop.settings.deepIndexStructuredDesc' },
  { id: 'fulltext', labelKey: 'desktop.settings.deepIndexFullText', descKey: 'desktop.settings.deepIndexFullTextDesc' },
  { id: 'ondemand', labelKey: 'desktop.settings.deepIndexOnDemand', descKey: 'desktop.settings.deepIndexOnDemandDesc' }
]
const retentionDays = ref<number>(store.settings.deepIndexRetentionDays ?? 90)
const purgeBusy = ref(false)
const purgeResult = ref('')
const purgeDialogOpen = ref(false)

const selectDeepIndexLevel = async (level: DeepIndexLevel) => {
  deepIndexLevel.value = level
  store.settings.deepIndexLevel = level
  try {
    await store.saveSettings()
  } catch {
    // 保存失败保留本地选择，错误由 store.error 呈现
  }
}

const saveRetentionDays = async () => {
  const clamped = Math.min(3650, Math.max(1, Math.round(retentionDays.value || 90)))
  retentionDays.value = clamped
  store.settings.deepIndexRetentionDays = clamped
  try {
    await store.saveSettings()
  } catch {
    // 同上
  }
}

const runPurge = async () => {
  purgeDialogOpen.value = false
  purgeBusy.value = true
  purgeResult.value = ''
  try {
    const removed = await purgeSessionActivityContent(store.settings, 'all')
    purgeResult.value = t(locale.value, 'desktop.settings.purgeDone', { count: String(removed) })
  } catch {
    purgeResult.value = t(locale.value, 'desktop.settings.purgeFailed')
  } finally {
    purgeBusy.value = false
  }
}
const openPurgeDialog = () => {
  purgeResult.value = ''
  purgeDialogOpen.value = true
}
const closePurgeDialog = () => {
  purgeDialogOpen.value = false
}

// —— 关于与更新 ——
const appVersion = ref('')
const checkUpdateFlash = ref(false)
let checkUpdateFlashTimer: ReturnType<typeof setTimeout> | null = null

// —— 外部定位分组（openSettingsSection 写入；同页深链时页面不重挂载，需 watch 补消费） ——
const sectionFlash = ref<SettingsSection | null>(null)
let sectionFlashTimer: ReturnType<typeof setTimeout> | null = null

function consumeSettingsTargetSection() {
  const target = nav.settingsTargetSection
  if (!target) return
  if (sections.some(s => s.id === target)) {
    activeSection.value = target as SettingsSection
    // 短暂高亮提示定位到该分组
    sectionFlash.value = target as SettingsSection
    if (sectionFlashTimer) clearTimeout(sectionFlashTimer)
    sectionFlashTimer = setTimeout(() => { sectionFlash.value = null }, 1400)
  }
  // 无论是否合法都清空，防残留（下次进入设置页不再重复定位）
  nav.settingsTargetSection = null
}

watch(
  () => nav.settingsTargetSection,
  () => consumeSettingsTargetSection()
)

onMounted(async () => {
  consumeSettingsTargetSection()
  try {
    const { getVersion } = await import('@tauri-apps/api/app')
    appVersion.value = await getVersion()
  } catch {
    appVersion.value = ''
  }
})

const handleCheckUpdate = async () => {
  if (updaterStore.status === 'checking') return
  if (updaterStore.hasUpdate) {
    updaterStore.openDialog()
    return
  }
  await updaterStore.checkForUpdate()
  if (updaterStore.status === 'idle') {
    checkUpdateFlash.value = true
    if (checkUpdateFlashTimer) clearTimeout(checkUpdateFlashTimer)
    checkUpdateFlashTimer = setTimeout(() => { checkUpdateFlash.value = false }, 2000)
  }
}

// —— 退出应用（需确认；不可逆操作） ——
const quitDialogOpen = ref(false)
const quitBusy = ref(false)
const quitFailed = ref(false)
const openQuitDialog = () => {
  quitFailed.value = false
  quitDialogOpen.value = true
}
const closeQuitDialog = () => {
  if (quitBusy.value) return
  quitDialogOpen.value = false
}
const confirmQuit = async () => {
  if (quitBusy.value) return
  quitBusy.value = true
  quitFailed.value = false
  try {
    await quitApplication(store)
  } catch (error) {
    console.error('[DesktopSettings] Failed to quit app:', error)
    quitFailed.value = true
  } finally {
    quitBusy.value = false
  }
}
</script>

<template>
  <div class="flex items-start gap-5 pb-4">
    <!-- 左侧二级目录（窄屏转顶部横向滚动条） -->
    <nav
      class="flex shrink-0 gap-1 overflow-x-auto rounded-xl border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] p-1 lg:w-44 lg:flex-col lg:overflow-visible"
      :aria-label="t(locale, 'desktop.settings.navLabel')"
    >
      <button
        v-for="section in sections"
        :key="section.id"
        type="button"
        class="whitespace-nowrap rounded-lg px-3 py-1.5 text-left text-[12px] font-semibold transition-colors lg:w-full"
        :class="activeSection === section.id ? 'bg-[var(--theme-accent-primary)] text-[var(--theme-accent-contrast)]' : 'text-[var(--theme-text-tertiary)] hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)]'"
        :aria-current="activeSection === section.id ? 'page' : undefined"
        @click="activeSection = section.id"
      >
        {{ t(locale, section.labelKey) }}
      </button>
    </nav>

    <!-- 右侧内容区（普通分隔区块，不套卡片） -->
    <div
      class="min-w-0 flex-1 space-y-5"
      :class="sectionFlash ? 'settings-section-flash' : ''"
    >
      <!-- 应用 -->
      <section v-if="activeSection === 'app'" class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionApp') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionAppDesc') }}</p>
        <GeneralSettingsPanel />
      </section>

      <!-- 外观 -->
      <section v-else-if="activeSection === 'appearance'" class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionAppearance') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionAppearanceDesc') }}</p>
        <ThemeSelector />
      </section>

      <!-- 数据源 -->
      <section v-else-if="activeSection === 'dataSources'" class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionDataSources') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionDataSourcesDesc') }}</p>
        <!-- 桌面设置内嵌数据源管理（组件自带返回按钮，这里 no-op：由目录切换承担返回语义） -->
        <ApiSourceList @back="() => {}" />
      </section>

      <!-- 会话与隐私（M2 占位） -->
      <section v-else-if="activeSection === 'privacy'" class="space-y-4">
        <div class="space-y-1.5">
          <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionPrivacy') }}</h3>
          <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionPrivacyDesc') }}</p>
        </div>

        <!-- 深度索引级别：不能用单个开关混淆多种风险，使用 radio group -->
        <div class="space-y-1.5">
          <h4 class="px-1 text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.settings.deepIndexLevel') }}</h4>
          <div class="space-y-2">
            <label
              v-for="option in deepIndexOptions"
              :key="option.id"
              class="flex cursor-pointer items-start gap-3 rounded-lg border border-[var(--theme-border-subtle)] bg-[var(--theme-bg-surface)] px-3 py-2.5"
            >
              <input
                type="radio"
                name="deep-index-level"
                class="mt-0.5 h-3.5 w-3.5 accent-[var(--theme-accent-primary)]"
                :checked="deepIndexLevel === option.id"
                :aria-label="t(locale, option.labelKey)"
                @change="selectDeepIndexLevel(option.id)"
              />
              <span class="min-w-0">
                <span class="block text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, option.labelKey) }}</span>
                <span class="mt-0.5 block text-[10.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, option.descKey) }}</span>
              </span>
            </label>
          </div>
        </div>

        <!-- 保留期限与清理 -->
        <div class="space-y-1.5">
          <h4 class="px-1 text-[12px] font-semibold text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.settings.privacyRetention') }}</h4>
          <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.privacyRetentionDesc') }}</p>
          <div class="flex flex-wrap items-center gap-2 px-1">
            <label class="flex items-center gap-2 text-[11px] text-[var(--theme-text-secondary)]">
              {{ t(locale, 'desktop.settings.retentionDaysLabel') }}
              <input
                v-model.number="retentionDays"
                type="number"
                min="1"
                max="3650"
                class="w-20 rounded-md border border-[var(--theme-border-default)] bg-[var(--theme-bg-surface)] px-2 py-1 text-[12px] text-[var(--theme-text-primary)] outline-none focus:ring-2 focus:ring-[var(--theme-accent-primary)]"
                @change="saveRetentionDays()"
              />
              {{ t(locale, 'desktop.settings.retentionDaysUnit') }}
            </label>
            <button
              type="button"
              class="inline-flex items-center gap-1.5 rounded-md border border-[var(--theme-border-default)] px-2.5 py-1 text-[11px] font-medium text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-bg-hover)] disabled:opacity-60"
              :disabled="purgeBusy"
              @click="openPurgeDialog()"
            >
              <Trash2 v-if="!purgeBusy" class="h-3.5 w-3.5" :aria-hidden="true" />
              <RefreshCw v-else class="h-3.5 w-3.5 animate-spin" :aria-hidden="true" />
              {{ t(locale, 'desktop.settings.purgeNow') }}
            </button>
          </div>
          <p v-if="purgeResult" class="px-1 text-[11px]" role="status">
            {{ purgeResult }}
          </p>
        </div>
      </section>

      <!-- 来源与计费 -->
      <section v-else-if="activeSection === 'pricing'" class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionPricing') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionPricingDesc') }}</p>
        <DataNavigationPanel
          @open-api-sources="activeSection = 'dataSources'"
          @open-model-pricing="pricingSubView = 'model-pricing'"
          @open-currency="pricingSubView = 'currency'"
        />
        <ModelPricingSettings v-if="pricingSubView === 'model-pricing'" @back="pricingSubView = 'main'" />
        <CurrencySettings v-else-if="pricingSubView === 'currency'" @back="pricingSubView = 'main'" />
      </section>

      <!-- 网关与代理 -->
      <section v-else-if="activeSection === 'gateway'" class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionGateway') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionGatewayDesc') }}</p>
        <ProxyControlPanel />
        <CcSwitchCompatPanel />
      </section>

      <!-- 网络 -->
      <section v-else-if="activeSection === 'network'" class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionNetwork') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionNetworkDesc') }}</p>
        <NetworkProxyPanel />
      </section>

      <!-- 同步 -->
      <section v-else-if="activeSection === 'sync'" class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionSync') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionSyncDesc') }}</p>
        <SyncSettingsPanel />
      </section>

      <!-- 存储与维护 -->
      <section v-else-if="activeSection === 'storage'" class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionStorage') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionStorageDesc') }}</p>
        <LocalCachePanel />
        <LocalCacheManagementPanel />
      </section>

      <!-- 关于与更新 -->
      <section v-else class="space-y-1.5">
        <h3 class="px-1 text-xs font-semibold uppercase tracking-wider text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.sectionAbout') }}</h3>
        <p class="px-1 text-[11px] leading-relaxed text-[var(--theme-text-quaternary)]">{{ t(locale, 'desktop.settings.sectionAboutDesc') }}</p>

        <!-- 版本与更新（普通分隔区块，label + description + control 网格） -->
        <div class="border-b border-[var(--theme-border-default)] py-3">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <div class="min-w-0">
              <div class="flex items-center gap-2 text-[12.5px] font-medium text-[var(--theme-text-primary)]">
                <span>{{ t(locale, 'desktop.settings.aboutApp') }}</span>
                <span class="font-mono text-[12px] text-[var(--theme-text-secondary)]">v{{ appVersion || '—' }}</span>
              </div>
              <p class="mt-0.5 text-[10.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.aboutAppDesc') }}</p>
              <p v-if="updaterStore.hasUpdate && updaterStore.updateInfo" class="mt-1 text-[10.5px] font-medium text-[var(--theme-status-info-fg)]">
                {{ t(locale, 'settings.update.newVersionReady', { version: updaterStore.updateInfo.version }) }}
              </p>
            </div>
            <button
              type="button"
              class="theme-button-secondary inline-flex h-8 items-center gap-1.5 rounded-lg px-3 text-[11.5px] font-semibold disabled:opacity-50"
              :disabled="updaterStore.status === 'checking'"
              @click="handleCheckUpdate"
            >
              <RefreshCw class="h-3.5 w-3.5" :class="{ 'animate-spin': updaterStore.status === 'checking' }" aria-hidden="true" />
              <span v-if="updaterStore.status === 'checking'">{{ t(locale, 'settings.update.checking') }}</span>
              <span v-else-if="updaterStore.hasUpdate">{{ t(locale, 'settings.update.viewUpdate') }}</span>
              <span v-else-if="checkUpdateFlash">✓ {{ t(locale, 'settings.update.upToDate') }}</span>
              <span v-else-if="updaterStore.status === 'error'" class="text-red-400">{{ t(locale, 'settings.update.checkFailed') }}</span>
              <span v-else>{{ t(locale, 'settings.update.checkNow') }}</span>
            </button>
          </div>
        </div>

        <!-- 项目主页 -->
        <div class="border-b border-[var(--theme-border-default)] py-3">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <div class="min-w-0">
              <div class="text-[12.5px] font-medium text-[var(--theme-text-primary)]">{{ t(locale, 'desktop.settings.aboutHomepage') }}</div>
              <p class="mt-0.5 text-[10.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'desktop.settings.aboutHomepageDesc') }}</p>
            </div>
            <a
              href="https://github.com/smileslove/UsageMeter"
              target="_blank"
              rel="noreferrer"
              class="theme-button-secondary inline-flex h-8 items-center gap-1.5 rounded-lg px-3 text-[11.5px] font-semibold"
            >
              <ExternalLink class="h-3.5 w-3.5" aria-hidden="true" />
              GitHub
            </a>
          </div>
        </div>

        <!-- 退出应用（危险操作，需确认） -->
        <div class="py-3">
          <div class="flex flex-wrap items-center justify-between gap-3">
            <div class="min-w-0">
              <div class="text-[12.5px] font-medium text-red-500">{{ t(locale, 'settings.quitApp') }}</div>
              <p class="mt-0.5 text-[10.5px] leading-relaxed text-[var(--theme-text-tertiary)]">{{ t(locale, 'settings.quitAppDesc') }}</p>
            </div>
            <button
              type="button"
              class="shrink-0 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-1.5 text-[11px] font-semibold text-red-500 transition-colors hover:bg-red-500/15 disabled:cursor-not-allowed disabled:opacity-60"
              :disabled="quitBusy"
              @click="openQuitDialog"
            >
              {{ t(locale, 'settings.quitApp') }}
            </button>
          </div>
          <p v-if="quitFailed" class="mt-1 text-[11px] text-red-500">{{ t(locale, 'settings.quitAppFailed') }}</p>
        </div>
      </section>

      <!-- 保存状态 -->
      <div v-if="store.saving" class="px-1 text-[11px] text-[var(--theme-text-tertiary)]">{{ t(locale, 'common.saving') }}</div>
      <div v-if="store.error" class="px-1 text-[11px] text-red-500">{{ store.error }}</div>
    </div>
  </div>

  <ConfirmDialog
    :open="quitDialogOpen"
    :title="t(locale, 'settings.quitAppConfirmTitle')"
    :body="t(locale, 'settings.quitAppConfirmBody')"
    :confirm-label="t(locale, 'settings.quitApp')"
    :cancel-label="t(locale, 'common.cancel')"
    :busy="quitBusy"
    tone="danger"
    @cancel="closeQuitDialog"
    @confirm="confirmQuit"
  />

  <ConfirmDialog
    :open="purgeDialogOpen"
    :title="t(locale, 'desktop.settings.purgeConfirmTitle')"
    :body="t(locale, 'desktop.settings.purgeConfirmBody')"
    :confirm-label="t(locale, 'desktop.settings.purgeNow')"
    :cancel-label="t(locale, 'common.cancel')"
    :busy="purgeBusy"
    tone="danger"
    @cancel="closePurgeDialog"
    @confirm="runPurge"
  />
</template>

<style scoped>
/* 外部定位分组（openSettingsSection）后的短暂高亮提示 */
.settings-section-flash {
  border-radius: 0.75rem;
  animation: settings-section-flash 1.2s ease-out;
}

@keyframes settings-section-flash {
  0% {
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--theme-accent-primary) 40%, transparent);
  }
  100% {
    box-shadow: 0 0 0 3px transparent;
  }
}
</style>
