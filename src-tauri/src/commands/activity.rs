//! 深度会话活动（M2）IPC 命令。
//!
//! 与 `doc/桌面主应用窗口详细设计.md` 12.6 节 IPC 草案对齐。查询命令
//! 遵守隐私联动：`AppSettings.deep_index_level == "off"` 时深度查询
//! 返回空/None、rebuild 返回空结果（不触发任何深度表写入）；开启时
//! 查询路径先做单会话懒索引（同步、失败不阻断查询，深度解析失败
//! 不影响既有统计）。

use crate::activity::db;
use crate::activity::indexer;
use crate::activity::model::{
    AgentNodeDto, ContentState, EventsPage, RebuildResult, RedactedPayloadPage, SafeSourceRef,
    SessionActivityCapability, SessionActivitySummary, SessionEventFilter, ToolSummaryRow,
};
use crate::activity::registry::AdapterRegistry;
use crate::models::AppSettings;
use rusqlite::OptionalExtension;

/// 事件列表页上限（12.6：列表默认 100 事件一页；命令层 clamp 1..=200）。
const EVENTS_PAGE_MAX_LIMIT: i64 = 200;
/// payload 默认读取上限（字节；与适配器默认值一致）。
const PAYLOAD_DEFAULT_MAX_BYTES: usize = 262_144;

fn registry() -> AdapterRegistry {
    AdapterRegistry::new()
}

fn activity_db<R>(
    f: impl FnOnce(&mut rusqlite::Connection) -> Result<R, String>,
) -> Result<R, String> {
    let db = crate::local_usage::get_local_usage_db()?;
    db.with_conn(f)
}

/// 隐私门：深度索引关闭时不新增任何正文持久化（设计 15.2）。
fn deep_index_disabled(settings: &AppSettings) -> bool {
    settings.deep_index_level == "off"
}

/// 内容不可用的诚实响应（不伪造内容）。
fn unavailable_page() -> RedactedPayloadPage {
    RedactedPayloadPage {
        content: String::new(),
        truncated: false,
        next_cursor: None,
        content_state: ContentState::Unavailable,
    }
}

/// 查询路径的懒索引：同步执行单会话索引（文件小，通常 <100ms）；
/// 失败不阻断查询（返回空 + 错误日志 eprintln），保证「深度解析失败
/// 不影响既有统计」（设计 14.4 降级语义）。
fn lazy_ensure_indexed(conn: &mut rusqlite::Connection, session_key: &str) {
    match indexer::find_source_ref(conn, session_key) {
        Ok(Some(source)) => {
            if let Err(error) = indexer::ensure_session_indexed(conn, &source) {
                eprintln!("[UsageMeter] Deep index lazy sync failed: {error}");
            }
        }
        Ok(None) => {}
        Err(error) => eprintln!("[UsageMeter] Deep index lazy source lookup failed: {error}"),
    }
}

/// 获取各来源工具的深度活动能力（按工具名排序；无注册适配器返回空列表）。
///
/// 能力是“该工具支持什么”的静态快照（运行时返回值，UI 不得按工具名
/// 硬编码），与用户是否开启深度索引无关；开启状态由各查询命令的
/// `deep_index_disabled` 门控体现。
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

/// 获取单会话活动汇总；懒索引后仍无该会话深度索引时返回 None（不伪造数据）。
#[tauri::command]
pub async fn get_session_activity_summary(
    _app: tauri::AppHandle,
    session_key: String,
    settings: AppSettings,
) -> Result<Option<SessionActivitySummary>, String> {
    if deep_index_disabled(&settings) {
        return Ok(None);
    }
    activity_db(|conn| {
        lazy_ensure_indexed(conn, &session_key);
        db::query_activity_summary(conn, &session_key)
    })
}

/// 分页查询会话事件（按 sequence 升序；limit clamp 1..=200）。
#[tauri::command]
pub async fn get_session_events(
    _app: tauri::AppHandle,
    session_key: String,
    filter: Option<SessionEventFilter>,
    offset: i64,
    limit: i64,
    settings: AppSettings,
) -> Result<EventsPage, String> {
    if deep_index_disabled(&settings) {
        return Ok(EventsPage {
            items: Vec::new(),
            total: 0,
            has_more: false,
        });
    }
    let limit = limit.clamp(1, EVENTS_PAGE_MAX_LIMIT);
    let offset = offset.max(0);
    activity_db(|conn| {
        lazy_ensure_indexed(conn, &session_key);
        db::query_events(conn, &session_key, filter.as_ref(), offset, limit)
    })
}

/// 获取会话代理树节点（M2 按已落库的 session_agents 返回）。
#[tauri::command]
pub async fn get_session_agents(
    _app: tauri::AppHandle,
    session_key: String,
    settings: AppSettings,
) -> Result<Vec<AgentNodeDto>, String> {
    if deep_index_disabled(&settings) {
        return Ok(Vec::new());
    }
    activity_db(|conn| {
        lazy_ensure_indexed(conn, &session_key);
        db::query_agents(conn, &session_key)
    })
}

/// 获取会话工具调用汇总。
#[tauri::command]
pub async fn get_session_tool_summary(
    _app: tauri::AppHandle,
    session_key: String,
    settings: AppSettings,
) -> Result<Vec<ToolSummaryRow>, String> {
    if deep_index_disabled(&settings) {
        return Ok(Vec::new());
    }
    activity_db(|conn| {
        lazy_ensure_indexed(conn, &session_key);
        db::query_tool_summary(conn, &session_key)
    })
}

/// 按需读取（脱敏后的）事件 payload。
///
/// 定位链：session_events 行（session_key/source_file_path/source_offset）
/// → local_sessions.tool → 注册表适配器 `read_payload`。事件不存在报
/// `ERR_ACTIVITY_EVENT_NOT_FOUND`；会话/适配器缺失或深度索引关闭时诚实
/// 返回 `content_state=Unavailable` 空页，不伪造内容。
#[tauri::command]
pub async fn get_session_event_payload(
    _app: tauri::AppHandle,
    event_key: String,
    section: String,
    max_bytes: Option<usize>,
    settings: AppSettings,
) -> Result<RedactedPayloadPage, String> {
    if deep_index_disabled(&settings) {
        return Ok(unavailable_page());
    }
    let section = if section.trim().is_empty() {
        "summary"
    } else {
        section.trim()
    };
    let max_bytes = max_bytes.unwrap_or(PAYLOAD_DEFAULT_MAX_BYTES);
    activity_db(|conn| {
        let event_row: Option<(String, String, Option<i64>, Option<String>)> = conn
            .query_row(
                "SELECT session_key, source_file_path, source_offset, payload_hash
                 FROM session_events WHERE event_key = ?1",
                rusqlite::params![event_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| format!("ERR_ACTIVITY_QUERY_EVENT: {error}"))?;
        let Some((session_key, source_file_path, source_offset, payload_hash)) = event_row else {
            return Err("ERR_ACTIVITY_EVENT_NOT_FOUND: no such event key".to_string());
        };
        // session_events 无 tool 列：经 session_key 查 local_sessions.tool。
        let tool: Option<String> = conn
            .query_row(
                "SELECT tool FROM local_sessions WHERE session_id = ?1",
                rusqlite::params![session_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("ERR_ACTIVITY_QUERY_TOOL: {error}"))?;
        let Some(tool) = tool else {
            // 会话行缺失（local_sessions 清理后深度表残留）：不伪造可读内容。
            return Ok(unavailable_page());
        };
        let Some(adapter) = registry().get_adapter(&tool) else {
            return Ok(unavailable_page());
        };
        let source_ref = SafeSourceRef {
            source_file_id: None,
            source_file_path,
            source_offset,
            // M2 适配器未回填 payload_hash，指纹校验留给 M3。
            fingerprint: payload_hash,
        };
        adapter
            .read_payload(&source_ref, section, max_bytes)
            .map_err(|error| error.to_string())
    })
}

/// 重建深度活动索引（scope："all" | "session:<session_key>"）。
///
/// 深度索引关闭时返回空结果（不触发任何索引写入）；开启时在
/// spawn_blocking 后台线程执行，避免阻塞 UI 线程。重建期间的数据库
/// 访问与既有同步共用同一互斥连接（本地会话规模小、用户显式触发，
/// M2 可接受；单会话失败已隔离在 indexer 内部）。
#[tauri::command]
pub async fn rebuild_session_activity_index(
    _app: tauri::AppHandle,
    scope: String,
    settings: AppSettings,
) -> Result<RebuildResult, String> {
    if deep_index_disabled(&settings) {
        return Ok(RebuildResult::default());
    }
    tauri::async_runtime::spawn_blocking(move || {
        activity_db(|conn| indexer::rebuild_scope(conn, &scope))
    })
    .await
    .map_err(|error| format!("ERR_ACTIVITY_REBUILD_JOIN: {error}"))?
}

/// 清理深度活动内容（scope："all" | "session:<session_key>" |
/// "older_than_days:<n>"）；返回删除的行数（5 张深度表之和）。
///
/// 只影响深度索引，不删除原工具会话文件、不影响 `local_request_facts`
/// 等既有用量表口径。清理操作不受深度索引开关限制（关闭时清理残留
/// 数据同样合法）。
#[tauri::command]
pub async fn purge_session_activity_content(
    _app: tauri::AppHandle,
    scope: String,
    _settings: AppSettings,
) -> Result<usize, String> {
    activity_db(|conn| indexer::purge_scope(conn, &scope))
}

// ---------------------------------------------------------------------------
// 单元测试：scope 解析、clamp 与注册表接线
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::constants::TOOL_CLAUDE_CODE;

    #[test]
    fn claude_code_tool_name_matches_real_adapter() {
        // 真实适配器工具名与既有 session 常量一致。
        assert_eq!(TOOL_CLAUDE_CODE, "claude_code");
        let adapter = registry().get_adapter(TOOL_CLAUDE_CODE);
        assert!(adapter.is_some());
        assert_eq!(adapter.unwrap().tool_name(), TOOL_CLAUDE_CODE);
    }

    #[test]
    fn deep_index_disabled_matches_off_value() {
        let settings = AppSettings {
            deep_index_level: "off".to_string(),
            ..AppSettings::default()
        };
        assert!(deep_index_disabled(&settings));
        for level in ["structured", "fulltext", "ondemand"] {
            let settings = AppSettings {
                deep_index_level: level.to_string(),
                ..AppSettings::default()
            };
            assert!(
                !deep_index_disabled(&settings),
                "level {level} enables deep index"
            );
        }
    }

    #[test]
    fn events_page_limit_clamped_to_1_200() {
        assert_eq!(EVENTS_PAGE_MAX_LIMIT, 200);
        let clamped = 500i64.clamp(1, EVENTS_PAGE_MAX_LIMIT);
        assert_eq!(clamped, 200);
        let clamped = 0i64.clamp(1, EVENTS_PAGE_MAX_LIMIT);
        assert_eq!(clamped, 1);
    }
}
