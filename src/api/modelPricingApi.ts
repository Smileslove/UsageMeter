import { invoke } from '@tauri-apps/api/core'
import type { ModelPricingConfig } from '../types'

export interface ModelPricingSearchResult {
  pricings: ModelPricingConfig[]
  total: number
}

export interface ModelPricingApplyFilter {
  matchMode: 'fuzzy' | 'exact'
  timeRangeStart: number | null
  timeRangeEnd: number | null
  clientToolFilter: string | null
  apiSourceKeyPrefixes: string[] | null
}

export interface ModelPricingPreviewResult {
  matchedCount: number
  totalCurrentCost: number
  modelCounts: Array<{
    model: string
    count: number
  }>
}

export async function getCustomModelPricings(query: string | null): Promise<ModelPricingConfig[]> {
  const result = await invoke<string>('get_custom_model_pricings', { query })
  return (JSON.parse(result) as ModelPricingConfig[] | null) ?? []
}

export function countSyncedModelPricings(query: string | null): Promise<number> {
  return invoke('count_synced_model_pricings', { query })
}

export async function searchModelPricing(
  query: string | null,
  limit: number,
  offset: number
): Promise<ModelPricingSearchResult> {
  const result = await invoke<string>('search_model_pricing', { query, limit, offset })
  return JSON.parse(result) as ModelPricingSearchResult
}

export function syncModelPricingFromApi(): Promise<number> {
  return invoke('sync_model_pricing_from_api')
}

export function clearSyncedModelPricings(): Promise<number> {
  return invoke('clear_synced_model_pricings')
}

export function addCustomModelPricing(pricing: ModelPricingConfig): Promise<void> {
  return invoke('add_custom_model_pricing', { pricing })
}

export function updateCustomModelPricing(pricing: ModelPricingConfig): Promise<void> {
  return invoke('update_custom_model_pricing', { pricing })
}

export function deleteModelPricing(modelId: string): Promise<void> {
  return invoke('delete_model_pricing', { modelId })
}

export function previewModelPricingApply(
  modelId: string,
  filter: ModelPricingApplyFilter
): Promise<ModelPricingPreviewResult> {
  return invoke('preview_pricing_apply', { modelId, ...filter })
}

export function applyModelPricingToRecords(
  modelId: string,
  pricing: ModelPricingConfig,
  filter: ModelPricingApplyFilter
): Promise<number> {
  return invoke('apply_pricing_to_records', { modelId, pricing, ...filter })
}
