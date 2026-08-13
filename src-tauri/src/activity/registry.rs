//! 会话活动适配器注册表（与 `session::registry` 风格一致）。
//!
//! M2 阶段只注册 Claude Code 与 Codex 两个占位适配器（能力为 none、
//! `index_session` 返回 Unsupported），真实解析实现由后续子代理填充。

use crate::activity::adapter::{ActivityError, ActivityIndexBatch, SessionActivityAdapter};
use crate::activity::model::{
    ActivityCapabilityLevel, AgentRelationLevel, RedactedPayloadPage, SafeSourceRef,
    SessionActivityCapability,
};

/// 未实现解析的占位适配器：诚实上报“无能力”，不伪造数据。
struct PlaceholderAdapter {
    tool: &'static str,
    parser_id: &'static str,
}

impl SessionActivityAdapter for PlaceholderAdapter {
    fn tool_name(&self) -> &'static str {
        self.tool
    }

    fn capability(&self) -> SessionActivityCapability {
        SessionActivityCapability {
            level: ActivityCapabilityLevel::None,
            messages: false,
            tool_invocations: false,
            tool_results: false,
            request_links: false,
            agent_relations: AgentRelationLevel::None,
            content_search: false,
            source_content_available: false,
            parser_id: self.parser_id.to_string(),
            parser_version: 0,
        }
    }

    fn index_session(
        &self,
        _source: &crate::activity::adapter::SessionSourceRef,
    ) -> Result<ActivityIndexBatch, ActivityError> {
        Err(ActivityError::Unsupported(format!(
            "deep activity indexing not implemented for tool {}",
            self.tool
        )))
    }

    fn read_payload(
        &self,
        _source_ref: &SafeSourceRef,
        _section: &str,
        _max_bytes: usize,
    ) -> Result<RedactedPayloadPage, ActivityError> {
        Err(ActivityError::Unsupported(format!(
            "deep activity payload reading not implemented for tool {}",
            self.tool
        )))
    }
}

static CLAUDE_CODE_ADAPTER: PlaceholderAdapter = PlaceholderAdapter {
    tool: "claude_code",
    parser_id: "claude_code_placeholder",
};
static CODEX_ADAPTER: PlaceholderAdapter = PlaceholderAdapter {
    tool: "codex",
    parser_id: "codex_placeholder",
};

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

    #[test]
    fn registry_contains_placeholder_adapters() {
        let registry = AdapterRegistry::new();
        let claude = registry
            .get_adapter("claude_code")
            .expect("claude_code adapter registered");
        assert_eq!(claude.tool_name(), "claude_code");
        assert_eq!(claude.capability().level, ActivityCapabilityLevel::None);
        assert!(!claude.capability().source_content_available);

        let codex = registry
            .get_adapter("codex")
            .expect("codex adapter registered");
        assert_eq!(codex.tool_name(), "codex");

        assert!(registry.get_adapter("unknown_tool").is_none());
        assert_eq!(registry.adapters().len(), 2);
    }

    #[test]
    fn placeholder_index_session_returns_unsupported() {
        let registry = AdapterRegistry::new();
        let claude = registry.get_adapter("claude_code").unwrap();
        let source = crate::activity::adapter::SessionSourceRef {
            session_id: "s1".to_string(),
            tool: "claude_code".to_string(),
            primary_file_path: "/tmp/s.jsonl".to_string(),
            source_file_id: None,
        };
        let err = claude.index_session(&source).unwrap_err();
        assert!(err.to_string().starts_with("ERR_ACTIVITY_UNSUPPORTED"));
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
