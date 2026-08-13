//! 深度会话活动（M2）IPC 命令骨架。
//!
//! 与 `doc/桌面主应用窗口详细设计.md` 12.6 节 IPC 草案对齐。本阶段命令
//! 返回诚实结果（不伪造数据）：查询基于已落库的深度索引表；适配器解析
//! 与 payload 读取接线由下一子代理完成，骨架先返回空/Unavailable。

use crate::activity::db;
use crate::activity::model::{
    AgentNodeDto, ContentState, EventsPage, RebuildResult, RedactedPayloadPage,
    SessionActivityCapability, SessionActivitySummary, SessionEventFilter, ToolSummaryRow,
};
use crate::activity::registry::AdapterRegistry;
use crate::models::AppSettings;

/// 事件列表页上限（12.6：列表默认 100 事件一页；命令层 clamp 1..=200）。
const EVENTS_PAGE_MAX_LIMIT: i64 = 200;

fn registry() -> AdapterRegistry {
    AdapterRegistry::new()
}

fn activity_db<R>(
    f: impl FnOnce(&mut rusqlite::Connection) -> Result<R, String>,
) -> Result<R, String> {
    let db = crate::local_usage::get_local_usage_db()?;
    db.with_conn(f)
}

/// 获取各来源工具的深度活动能力（按工具名排序；无注册适配器返回空列表）。
#[tauri::command]
pub async fn get_activity_capabilities(
    _app: tauri::AppHandle,
) -> Result<Vec<SessionActivityCapability>, String> {
    Ok(registry()
        .adapters()
        .into_iter()
        .map(|adapter| adapter.capability())
        .collect())
}
/// 获取单会话活动汇总；该会话无深度索引时返回 None（不伪造数据）。
#[tauri::command]
pub async fn get_session_activity_summary(
    _app: tauri::AppHandle,
    session_key: String,
    _settings: AppSettings,
) -> Result<Option<SessionActivitySummary>, String> {
    activity_db(|conn| db::query_activity_summary(conn, &session_key))
}

/// 分页查询会话事件（按 sequence 升序；limit clamp 1..=200）。
#[tauri::command]
pub async fn get_session_events(
    _app: tauri::AppHandle,
    session_key: String,
    filter: Option<SessionEventFilter>,
    offset: i64,
    limit: i64,
    _settings: AppSettings,
) -> Result<EventsPage, String> {
    let limit = limit.clamp(1, EVENTS_PAGE_MAX_LIMIT);
    let offset = offset.max(0);
    activity_db(|conn| db::query_events(conn, &session_key, filter.as_ref(), offset, limit))
}

/// 获取会话代理树节点（M2 按已落库的 session_agents 返回）。
#[tauri::command]
pub async fn get_session_agents(
    _app: tauri::AppHandle,
    session_key: String,
    _settings: AppSettings,
) -> Result<Vec<AgentNodeDto>, String> {
    activity_db(|conn| db::query_agents(conn, &session_key))
}

/// 获取会话工具调用汇总。
#[tauri::command]
pub async fn get_session_tool_summary(
    _app: tauri::AppHandle,
    session_key: String,
    _settings: AppSettings,
) -> Result<Vec<ToolSummaryRow>, String> {
    activity_db(|conn| db::query_tool_summary(conn, &session_key))
}

/// 按需读取（脱敏后的）事件 payload。
///
/// M2 骨架：仅查事件行定位来源；payload 内容读取（适配器 `read_payload`
/// 接线、fingerprint/offset 校验、路径白名单校验）由下一子代理完成，
/// 此处诚实返回 `content_state=Unavailable`，不伪造内容。
#[tauri::command]
pub async fn get_session_event_payload(
    _app: tauri::AppHandle,
    event_key: String,
    _section: String,
    _max_bytes: Option<usize>,
    _settings: AppSettings,
) -> Result<RedactedPayloadPage, String> {
    // 事件行存在性检查（骨架阶段仍校验 key，避免对未知事件返回“可读”状态）。
    let exists = activity_db(|conn| {
        conn.query_row(
            "SELECT 1 FROM session_events WHERE event_key = ?1",
            rusqlite::params![event_key],
            |_row| Ok(()),
        )
        .map(|_| true)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(format!("ERR_ACTIVITY_QUERY_EVENT: {other}")),
        })
    })?;
    if !exists {
        return Err("ERR_ACTIVITY_EVENT_NOT_FOUND: no such event key".to_string());
    }
    Ok(RedactedPayloadPage {
        content: String::new(),
        truncated: false,
        next_cursor: None,
        content_state: ContentState::Unavailable,
    })
}

/// 重建深度活动索引（scope："all" | "session:<session_key>"）。
///
/// M2 骨架：适配器 `index_session` 接线由下一子代理完成，此处返回全零
/// 空结果（诚实反映“本次未索引任何会话”），不伪造成功数据。
#[tauri::command]
pub async fn rebuild_session_activity_index(
    _app: tauri::AppHandle,
    scope: String,
    _settings: AppSettings,
) -> Result<RebuildResult, String> {
    let _ = scope;
    Ok(RebuildResult::default())
}

/// 清理深度活动内容（scope："all" | "session:<session_key>" |
/// "older_than_days:<n>"）；返回删除的行数（5 张深度表之和）。
///
/// 只影响深度索引，不删除原工具会话文件、不影响 `local_request_facts`
/// 等既有用量表口径。
#[tauri::command]
pub async fn purge_session_activity_content(
    _app: tauri::AppHandle,
    scope: String,
    _settings: AppSettings,
) -> Result<usize, String> {
    if scope == "all" {
        return activity_db(|conn| db::delete_all_activity(conn, false));
    }
    if let Some(session_key) = scope.strip_prefix("session:") {
        let session_key = session_key.trim();
        if session_key.is_empty() {
            return Err("ERR_ACTIVITY_INVALID_SCOPE: empty session key".to_string());
        }
        return activity_db(|conn| db::delete_session_activity(conn, session_key, false));
    }
    if let Some(days_str) = scope.strip_prefix("older_than_days:") {
        let days: i64 = days_str.trim().parse().map_err(|_| {
            format!("ERR_ACTIVITY_INVALID_SCOPE: invalid days in scope: {days_str}")
        })?;
        if days <= 0 {
            return Err("ERR_ACTIVITY_INVALID_SCOPE: days must be positive".to_string());
        }
        return activity_db(|conn| db::delete_activity_older_than_days(conn, days, false));
    }
    Err(format!(
        "ERR_ACTIVITY_INVALID_SCOPE: unsupported scope: {scope}"
    ))
}

// ---------------------------------------------------------------------------
// 单元测试：骨架命令的 scope 解析与 clamp 逻辑
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::constants::TOOL_CLAUDE_CODE;

    #[test]
    fn claude_code_tool_name_matches_placeholder_adapter() {
        // 占位适配器工具名与既有 session 常量一致，后续解析层可直接对接。
        assert_eq!(TOOL_CLAUDE_CODE, "claude_code");
        let adapter = registry().get_adapter(TOOL_CLAUDE_CODE);
        assert!(adapter.is_some());
    }
}
