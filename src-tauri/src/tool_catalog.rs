//! Stable tool identity metadata shared by local-session and proxy adapters.
//!
//! The catalog deliberately contains no parsing or protocol behavior. Adapters
//! remain owned by their respective subsystems; this module is the single place
//! for cross-cutting tool identity and capability discovery.

use crate::session::constants::{
    TOOL_CLAUDE_CODE, TOOL_CODEX, TOOL_COPILOT, TOOL_DEEPSEEK_HARNESS, TOOL_GEMINI, TOOL_HERMES,
    TOOL_OPENCLAW, TOOL_OPENCODE, TOOL_PI, TOOL_QODER_CLI, TOOL_QODER_IDE, TOOL_QODER_IDE_CN,
    TOOL_QODER_WORK, TOOL_QODER_WORK_CN, TOOL_REASONIX,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolDescriptor {
    pub id: &'static str,
    pub path_prefix: &'static str,
    pub has_local_sessions: bool,
    pub supports_proxy: bool,
    /// Whether local observations and proxy observations can be reconciled
    /// without treating a free-form Gateway client label as proof of identity.
    pub supports_reconciliation: bool,
}

const TOOLS: &[ToolDescriptor] = &[
    ToolDescriptor {
        id: TOOL_CLAUDE_CODE,
        path_prefix: "claude-code",
        has_local_sessions: true,
        supports_proxy: true,
        supports_reconciliation: true,
    },
    ToolDescriptor {
        id: TOOL_CODEX,
        path_prefix: "codex",
        has_local_sessions: true,
        supports_proxy: true,
        supports_reconciliation: true,
    },
    ToolDescriptor {
        id: TOOL_DEEPSEEK_HARNESS,
        path_prefix: "deepseek-harness",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_OPENCLAW,
        path_prefix: "openclaw",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_OPENCODE,
        path_prefix: "opencode",
        has_local_sessions: true,
        supports_proxy: true,
        supports_reconciliation: true,
    },
    ToolDescriptor {
        id: TOOL_REASONIX,
        path_prefix: "reasonix",
        // ReasonX 本地会话只有会话级累计 telemetry（无逐请求明细），按 end_time
        // 整段归入统计窗口导致数据失真。已移除本地扫描链路；仅保留代理采集
        // （supports_proxy），ReasonX 数据统一由代理/网关提供。
        has_local_sessions: false,
        supports_proxy: true,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_GEMINI,
        path_prefix: "gemini",
        has_local_sessions: true,
        supports_proxy: true,
        supports_reconciliation: true,
    },
    ToolDescriptor {
        id: TOOL_HERMES,
        path_prefix: "hermes",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_COPILOT,
        path_prefix: "copilot",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_PI,
        path_prefix: "pi",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_QODER_IDE,
        path_prefix: "qoder_ide",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_QODER_IDE_CN,
        path_prefix: "qoder_ide_cn",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_QODER_CLI,
        path_prefix: "qoder_cli",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_QODER_WORK,
        path_prefix: "qoder_work",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
    ToolDescriptor {
        id: TOOL_QODER_WORK_CN,
        path_prefix: "qoder_work_cn",
        has_local_sessions: true,
        supports_proxy: false,
        supports_reconciliation: false,
    },
];

pub fn find(id: &str) -> Option<&'static ToolDescriptor> {
    TOOLS.iter().find(|descriptor| descriptor.id == id)
}

pub fn proxy_tools() -> impl Iterator<Item = &'static ToolDescriptor> {
    TOOLS.iter().filter(|descriptor| descriptor.supports_proxy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_ids_and_proxy_prefixes_are_unique() {
        for (index, descriptor) in TOOLS.iter().enumerate() {
            assert!(TOOLS[index + 1..]
                .iter()
                .all(|other| other.id != descriptor.id));
            if descriptor.supports_proxy {
                assert!(TOOLS[index + 1..]
                    .iter()
                    .all(|other| !other.supports_proxy
                        || other.path_prefix != descriptor.path_prefix));
            }
        }
    }

    #[test]
    fn catalog_exposes_expected_capabilities() {
        assert_eq!(find("claude_code").unwrap().path_prefix, "claude-code");
        assert!(find("pi").unwrap().has_local_sessions);
        assert!(!find("pi").unwrap().supports_proxy);
        assert!(find("codex").unwrap().supports_proxy);
        assert!(!find("qoder_cli").unwrap().supports_proxy);
        assert!(find("claude_code").unwrap().supports_reconciliation);
        assert!(find("codex").unwrap().supports_reconciliation);
        assert!(find("opencode").unwrap().supports_reconciliation);
        assert!(find("gemini").unwrap().supports_reconciliation);
        assert!(!find("reasonix").unwrap().supports_reconciliation);
        assert!(!find("qoder_cli").unwrap().supports_reconciliation);
        assert!(!find("copilot").unwrap().supports_reconciliation);
        assert!(!find("hermes").unwrap().supports_reconciliation);
        assert!(!find("openclaw").unwrap().supports_reconciliation);
    }
}
