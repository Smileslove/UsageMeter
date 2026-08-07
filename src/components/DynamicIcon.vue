<script setup lang="ts">
import { computed, h } from 'vue'
import {
  Activity,
  AlertCircle,
  ArrowDown,
  ArrowUp,
  BarChart3,
  Bot,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  CircleHelp,
  Cloud,
  Copy,
  ExternalLink,
  Eye,
  Globe,
  Info,
  KeyRound,
  Link2,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Trash2,
  Upload,
  X,
  type LucideIcon
} from 'lucide-vue-next'

// Keep this registry explicit. Importing the whole lucide module makes every icon
// part of the initial tray bundle even though this component renders one icon at a time.
const ICON_REGISTRY: Record<string, LucideIcon> = {
  Activity, AlertCircle, ArrowDown, ArrowUp, BarChart3, Bot, Check, CheckCircle2,
  ChevronDown, ChevronLeft, ChevronRight, ChevronUp, CircleHelp, Cloud, Copy,
  ExternalLink, Eye, Globe, Info, KeyRound, Link2, Loader2, Pencil, Plus,
  RefreshCw, Search, Settings, Trash2, Upload, X
}

const props = defineProps<{
  name: string
  size?: number
  color?: string
}>()

const iconVNode = computed(() => {
  const component = ICON_REGISTRY[props.name]
  if (!component && import.meta.env.DEV) {
    console.warn(`[DynamicIcon] Unknown icon: ${props.name}`)
  }
  return h(component ?? Globe, { size: props.size ?? 16, color: props.color })
})
</script>

<template>
  <iconVNode />
</template>
