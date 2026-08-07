/** Cross-feature tool identity metadata. Protocol and UI behavior stay local. */
export interface ToolDescriptor {
  id: string
  pathPrefix: string
  hasLocalSessions: boolean
  supportsProxy: boolean
}

export const TOOL_CATALOG: readonly ToolDescriptor[] = [
  { id: 'claude_code', pathPrefix: 'claude-code', hasLocalSessions: true, supportsProxy: true },
  { id: 'codex', pathPrefix: 'codex', hasLocalSessions: true, supportsProxy: true },
  { id: 'openclaw', pathPrefix: 'openclaw', hasLocalSessions: true, supportsProxy: false },
  { id: 'opencode', pathPrefix: 'opencode', hasLocalSessions: true, supportsProxy: true },
  { id: 'reasonix', pathPrefix: 'reasonix', hasLocalSessions: true, supportsProxy: true },
  { id: 'gemini', pathPrefix: 'gemini', hasLocalSessions: true, supportsProxy: true },
  { id: 'hermes', pathPrefix: 'hermes', hasLocalSessions: true, supportsProxy: false },
  { id: 'copilot', pathPrefix: 'copilot', hasLocalSessions: true, supportsProxy: false },
  { id: 'qoder_ide', pathPrefix: 'qoder_ide', hasLocalSessions: true, supportsProxy: false },
  { id: 'qoder_ide_cn', pathPrefix: 'qoder_ide_cn', hasLocalSessions: true, supportsProxy: false },
  { id: 'qoder_cli', pathPrefix: 'qoder_cli', hasLocalSessions: true, supportsProxy: false },
  { id: 'qoder_work', pathPrefix: 'qoder_work', hasLocalSessions: true, supportsProxy: false },
  { id: 'qoder_work_cn', pathPrefix: 'qoder_work_cn', hasLocalSessions: true, supportsProxy: false },
]

export const SESSION_SOURCE_TOOLS = new Set(
  TOOL_CATALOG.filter(tool => tool.hasLocalSessions).map(tool => tool.id)
)

export const PROXY_TOOL_IDS = new Set(
  TOOL_CATALOG.filter(tool => tool.supportsProxy).map(tool => tool.id)
)
