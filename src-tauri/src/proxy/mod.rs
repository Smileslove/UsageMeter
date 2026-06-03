//! 代理模块 - 本地 HTTP 代理，用于拦截 Claude API 请求

mod codex_api;
mod codex_config;
mod collector;
mod config_manager;
mod database;
mod forwarder;
mod openai_forwarder;
mod opencode_config;
mod opencode_protocol;
mod server;
mod source_detector;
mod source_registry;
mod sse;
mod stream_processor;
mod types;

pub use codex_config::{
    codex_snapshot_uses_official_provider, CodexAuthMode, CodexConfigManager, CodexSourceRegistry,
};
pub use collector::UsageCollector;
pub use config_manager::ClaudeConfigManager;
pub use database::{
    PreviewPricingApplyResult, PricingMatchFilter, ProxyDatabase, ProxyDayDependencySnapshot,
    ProxyMergeCacheSignature,
};
pub use opencode_config::{OpenCodeConfigManager, OpenCodeSourceRegistry};
pub use server::ProxyServer;
pub use source_detector::compute_source_id;
pub use types::*;
