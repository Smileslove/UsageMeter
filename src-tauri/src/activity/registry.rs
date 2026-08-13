//! 会话活动适配器注册表（与 `session::registry` 风格一致）。
//!
//! M2 阶段注册 Claude Code 与 Codex 两个真实解析适配器（`adapters/claude.rs`
//! 与 `adapters/codex.rs`），能力由各适配器诚实上报；无占位、无伪数据。

use crate::activity::adapter::SessionActivityAdapter;
use crate::activity::adapters::claude::ClaudeAdapter;
use crate::activity::adapters::codex::CodexAdapter;

static CLAUDE_CODE_ADAPTER: ClaudeAdapter = ClaudeAdapter;
static CODEX_ADAPTER: CodexAdapter = CodexAdapter;

/// 会话活动适配器注册表。
#[derive(Default)]
pub struct AdapterRegistry {
    adapters: Vec<&'static dyn SessionActivityAdapter>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.register(&CLAUDE_CODE_ADAPTER);
        registry.register(&CODEX_ADAPTER);
        registry
    }

    /// 注册适配器（后注册同名工具覆盖先注册者，便于测试替换）。
    pub fn register(&mut self, adapter: &'static dyn SessionActivityAdapter) {
        self.adapters
            .retain(|existing| existing.tool_name() != adapter.tool_name());
        self.adapters.push(adapter);
    }

    /// 按工具名取适配器。
    pub fn get_adapter(&self, tool: &str) -> Option<&'static dyn SessionActivityAdapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.tool_name() == tool)
            .copied()
    }

    /// 全部已注册适配器（按工具名排序，供能力查询与遍历）。
    pub fn adapters(&self) -> Vec<&'static dyn SessionActivityAdapter> {
        let mut adapters = self.adapters.clone();
        adapters.sort_by_key(|adapter| adapter.tool_name());
        adapters
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::model::{ActivityCapabilityLevel, AgentRelationLevel};

    #[test]
    fn registry_contains_real_adapters() {
        let registry = AdapterRegistry::new();
        let claude = registry
            .get_adapter("claude_code")
            .expect("claude_code adapter registered");
        assert_eq!(claude.tool_name(), "claude_code");
        // 真实 Claude 适配器：结构化能力 + 根归组子代理关系 + 源内容可用。
        assert_eq!(
            claude.capability().level,
            ActivityCapabilityLevel::Structured
        );
        assert_eq!(
            claude.capability().agent_relations,
            AgentRelationLevel::RootGrouped
        );
        assert!(claude.capability().source_content_available);
        assert!(claude.capability().tool_invocations);
        assert!(claude.capability().request_links);

        let codex = registry
            .get_adapter("codex")
            .expect("codex adapter registered");
        assert_eq!(codex.tool_name(), "codex");
        assert_eq!(
            codex.capability().level,
            ActivityCapabilityLevel::Structured
        );
        assert_eq!(
            codex.capability().agent_relations,
            AgentRelationLevel::FlagOnly
        );

        assert!(registry.get_adapter("unknown_tool").is_none());
        assert_eq!(registry.adapters().len(), 2);
    }

    #[test]
    fn real_index_session_reports_io_error_without_path() {
        // 真实适配器已接线：对不存在的源文件返回 IO 错误（而非 Unsupported）。
        let registry = AdapterRegistry::new();
        let claude = registry.get_adapter("claude_code").unwrap();
        let source = crate::activity::adapter::SessionSourceRef {
            session_id: "s1".to_string(),
            tool: "claude_code".to_string(),
            primary_file_path: "/nonexistent/claude-x.jsonl".to_string(),
            source_file_id: None,
        };
        let err = claude.index_session(&source).unwrap_err();
        assert!(err.to_string().starts_with("ERR_ACTIVITY_IO"));
        assert!(
            !err.to_string().contains("nonexistent"),
            "error must not leak path"
        );
    }

    #[test]
    fn adapters_sorted_by_tool_name() {
        let registry = AdapterRegistry::new();
        let names: Vec<&str> = registry
            .adapters()
            .iter()
            .map(|adapter| adapter.tool_name())
            .collect();
        assert_eq!(names, vec!["claude_code", "codex"]);
    }
}
