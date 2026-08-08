//! Shared domain primitives that are independent of Tauri and persistence.

mod identity;

pub(crate) use identity::{ProviderId, SourceId, ToolId};
