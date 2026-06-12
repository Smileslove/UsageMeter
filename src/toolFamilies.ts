/**
 * 工具家族定义
 *
 * 家族代表 ID 用于 UI 筛选器与聚合显示；
 * 变体 ID 用于数据层精确归因，后端 build_filter() 会自动展开家族过滤。
 */

export interface ToolFamily {
  /** 家族代表 ID，同时也是 profiles 中保留的唯一条目 ID */
  head: string
  /** 家族内所有变体 tool ID（含 head 自身） */
  members: readonly string[]
  /** 各变体的显示子标签，key 为 member tool ID */
  variantLabels: Record<string, string>
}

export const TOOL_FAMILIES: ToolFamily[] = [
  {
    head: 'qoder_ide',
    members: ['qoder_cli', 'qoder_ide', 'qoder_ide_cn', 'qoder_work', 'qoder_work_cn'],
    variantLabels: {
      qoder_cli: 'CLI',
      qoder_ide: 'IDE',
      qoder_ide_cn: 'IDE CN',
      qoder_work: 'Work',
      qoder_work_cn: 'Work CN',
    },
  },
]

/** tool ID → 所属家族（不在任何家族中的 tool 返回 undefined） */
const MEMBER_TO_FAMILY = new Map<string, ToolFamily>(
  TOOL_FAMILIES.flatMap(f => f.members.map(m => [m, f] as [string, ToolFamily]))
)

/** 返回 tool ID 所属家族（若存在） */
export function getFamilyForTool(toolId: string): ToolFamily | undefined {
  return MEMBER_TO_FAMILY.get(toolId)
}

/** 若 tool 是某个家族的变体，返回该家族的 head ID；否则返回原 toolId */
export function getFamilyHead(toolId: string): string {
  return MEMBER_TO_FAMILY.get(toolId)?.head ?? toolId
}

/** 判断两个 tool 是否属于同一个家族 */
export function isSameFamily(a: string, b: string): boolean {
  const fa = MEMBER_TO_FAMILY.get(a)
  return fa != null && fa === MEMBER_TO_FAMILY.get(b)
}

/**
 * 返回 tool 的变体子标签（如 "Work CN"）；
 * 若该 tool 不是家族成员，或者就是 head 本身，则返回 null。
 */
export function getVariantLabel(toolId: string): string | null {
  const family = MEMBER_TO_FAMILY.get(toolId)
  if (!family) return null
  const label = family.variantLabels[toolId] ?? null
  // head 本身不额外显示副标签
  if (toolId === family.head) return null
  return label
}
