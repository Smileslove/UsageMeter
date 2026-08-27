import { computed, shallowRef } from 'vue'

export interface ColumnDef<K extends string> {
  key: K
  defaultVisible: boolean
}

export function useColumnConfig<
  K extends string,
  C extends ColumnDef<K>,
>(
  storageKey: string,
  columns: C[],
  minVisible = 1,
) {
  function load(): Set<K> {
    try {
      const saved = localStorage.getItem(storageKey)
      if (saved) {
        const keys = JSON.parse(saved) as string[]
        const valid: string[] = columns.map(c => c.key)
        const filtered = keys.filter((k): k is K => valid.includes(k))
        if (filtered.length > 0) return new Set(filtered)
      }
    } catch { /* ignore */ }
    return new Set(columns.filter(c => c.defaultVisible).map(c => c.key))
  }

  function save(cols: Set<K>) {
    try {
      localStorage.setItem(storageKey, JSON.stringify([...cols]))
    } catch { /* ignore */ }
  }

  const visibleColumns = shallowRef<Set<K>>(load())

  function toggleColumn(key: K) {
    const next = new Set(visibleColumns.value)
    if (next.has(key)) {
      if (next.size <= minVisible) return
      next.delete(key)
    } else {
      next.add(key)
    }
    visibleColumns.value = next
    save(next)
  }

  function isColumnVisible(key: K) {
    return visibleColumns.value.has(key)
  }

  const visibleCols = computed(() => columns.filter(c => isColumnVisible(c.key)))

  return { visibleColumns, toggleColumn, isColumnVisible, visibleCols }
}
