//! 深度会话活动（M2）IPC 命令。
//!
//! 与 `doc/桌面主应用窗口详细设计.md` 12.6 节 IPC 草案对齐。查询命令
//! 遵守隐私联动：`AppSettings.deep_index_level == "off"` 时深度查询
//! 返回空/None、rebuild 返回空结果（不触发任何深度表写入）；开启时
//! 查询路径先做单会话懒索引（同步、失败不阻断查询，深度解析失败
//! 不影响既有统计）。

use crate::activity::adapter::{SessionActivityAdapter, SessionSourceRef};
use crate::activity::db;
use crate::activity::indexer;
use crate::activity::maintenance;
use crate::activity::model::{
    AgentNodeDto, ContentState, EventStatus, EventsPage, ExportOptions, ExportResult,
    RebuildResult, RedactedPayloadPage, RequestEventLink, RequestLinkStrength, SafeSourceRef,
    SessionActivityCapability, SessionActivitySummary, SessionEventFilter, SessionEventKind,
    ToolInvocationSummary, ToolSummaryRow,
};
use crate::activity::registry::AdapterRegistry;
use crate::models::AppSettings;
use rusqlite::{params, params_from_iter, OptionalExtension};
use std::collections::HashMap;

/// 事件列表页上限（12.6：列表默认 100 事件一页；命令层 clamp 1..=200）。
const EVENTS_PAGE_MAX_LIMIT: i64 = 200;
/// payload 默认读取上限（字节；与适配器默认值一致）。
const PAYLOAD_DEFAULT_MAX_BYTES: usize = 262_144;
/// 导出：单事件 payload 读取上限（字节）。
const EXPORT_EVENT_PAYLOAD_MAX_BYTES: usize = 64 * 1024;
/// 导出：全部事件 payload 累计上限（字节；超过停止读取并置 truncated=true）。
const EXPORT_TOTAL_PAYLOAD_MAX_BYTES: usize = 50 * 1024 * 1024;
/// 导出文件名中 session_key 的消毒后最大长度。
const EXPORT_FILE_KEY_MAX_CHARS: usize = 80;

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
///
/// 档位取**持久化设置**（[`crate::settings::service::persisted_deep_index_level`]），
/// 不信任 IPC 传入的 settings——任意调用方不得以 `"structured"` 等值伪造
/// 开启，绕过隐私关闭（`"off"`）。IPC settings 仍用于其它非门控参数
/// （如 `deep_index_retention_days`）。
fn deep_index_policy() -> db::ActivityPersistencePolicy {
    db::ActivityPersistencePolicy::from_level(&crate::settings::persisted_deep_index_level())
}

#[cfg(test)]
fn deep_index_disabled() -> bool {
    !deep_index_policy().indexing_enabled()
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

fn ensure_source_indexed_without_db_lock(
    source: &SessionSourceRef,
    policy: db::ActivityPersistencePolicy,
) -> Result<Option<usize>, String> {
    let existing =
        activity_db(|conn| indexer::query_session_fingerprint(conn, &source.session_id))?;
    let Some(prepared) = indexer::prepare_session_index_if_changed(source, policy, existing)?
    else {
        return Ok(None);
    };
    // 设置在解析期间变化时丢弃旧策略产物；下一次查询按新策略重建。
    if deep_index_policy() != policy {
        return Ok(None);
    }
    activity_db(|conn| {
        // 与设置保存的隐私切换临界区使用同一 DB 锁；拿到锁后再次检查，
        // 防止“锁外检查通过 → 等待清理 → 用旧策略提交”的竞态。
        if deep_index_policy() != policy {
            return Ok(None);
        }
        indexer::commit_prepared_session_index(conn, prepared)
    })
}

/// 查询路径的懒索引：DB 锁只用于定位、指纹读取和最终短事务；源文件发现、
/// 解析与指纹复核均在锁外完成。失败必须反馈给调用方，避免把旧快照伪装成最新数据。
pub(super) fn lazy_ensure_indexed_without_db_lock(
    session_key: &str,
    policy: db::ActivityPersistencePolicy,
) -> Result<(), String> {
    let source = activity_db(|conn| indexer::find_source_ref(conn, session_key));
    match source {
        Ok(Some(source)) => ensure_source_indexed_without_db_lock(&source, policy).map(|_| ()),
        Ok(None) => Ok(()),
        Err(error) => Err(error),
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
    let policy = deep_index_policy();
    if !policy.indexing_enabled() {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        if deep_index_policy() != policy {
            return Ok(None);
        }
        let _ = activity_db(|conn| {
            maintenance::maybe_auto_purge(conn, settings.deep_index_retention_days)
        });
        lazy_ensure_indexed_without_db_lock(&session_key, policy)?;
        activity_db(|conn| db::query_activity_summary(conn, &session_key))
    })
    .await
    .map_err(|error| format!("ERR_ACTIVITY_SUMMARY_JOIN: {error}"))?
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
    let policy = deep_index_policy();
    if !policy.indexing_enabled() {
        return Ok(EventsPage {
            items: Vec::new(),
            total: 0,
            has_more: false,
        });
    }
    let limit = limit.clamp(1, EVENTS_PAGE_MAX_LIMIT);
    let offset = offset.max(0);
    tauri::async_runtime::spawn_blocking(move || {
        if deep_index_policy() != policy {
            return Ok(EventsPage {
                items: Vec::new(),
                total: 0,
                has_more: false,
            });
        }
        let _ = activity_db(|conn| {
            maintenance::maybe_auto_purge(conn, settings.deep_index_retention_days)
        });
        lazy_ensure_indexed_without_db_lock(&session_key, policy)?;
        activity_db(|conn| db::query_events(conn, &session_key, filter.as_ref(), offset, limit))
    })
    .await
    .map_err(|error| format!("ERR_ACTIVITY_EVENTS_JOIN: {error}"))?
}

/// 获取会话代理树节点（M2 按已落库的 session_agents 返回）。
#[tauri::command]
pub async fn get_session_agents(
    _app: tauri::AppHandle,
    session_key: String,
) -> Result<Vec<AgentNodeDto>, String> {
    let policy = deep_index_policy();
    if !policy.indexing_enabled() {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        if deep_index_policy() != policy {
            return Ok(Vec::new());
        }
        lazy_ensure_indexed_without_db_lock(&session_key, policy)?;
        activity_db(|conn| db::query_agents(conn, &session_key))
    })
    .await
    .map_err(|error| format!("ERR_ACTIVITY_AGENTS_JOIN: {error}"))?
}

/// 获取会话工具调用汇总。
#[tauri::command]
pub async fn get_session_tool_summary(
    _app: tauri::AppHandle,
    session_key: String,
) -> Result<Vec<ToolSummaryRow>, String> {
    let policy = deep_index_policy();
    if !policy.indexing_enabled() {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        if deep_index_policy() != policy {
            return Ok(Vec::new());
        }
        lazy_ensure_indexed_without_db_lock(&session_key, policy)?;
        activity_db(|conn| db::query_tool_summary(conn, &session_key))
    })
    .await
    .map_err(|error| format!("ERR_ACTIVITY_TOOLS_JOIN: {error}"))?
}

/// 按需读取（脱敏后的）事件 payload。
///
/// 定位链：session_events 行（session_key/source_file_path/source_offset）
/// → local_sessions.tool → 注册表适配器 `read_payload`。事件不存在报
/// `ERR_ACTIVITY_EVENT_NOT_FOUND`；会话/适配器缺失或深度索引关闭时诚实
/// 返回 `content_state=Unavailable` 空页，不伪造内容。
///
/// `cursor`：上一页返回的 `next_cursor`（`"B{n}"` 行内字节偏移），
/// `None` 从头读取；`max_bytes` 为单页上限（默认 256KB，适配器内部
/// clamp 1..=1MB）。
#[tauri::command]
pub async fn get_session_event_payload(
    _app: tauri::AppHandle,
    event_key: String,
    section: String,
    max_bytes: Option<usize>,
    cursor: Option<String>,
) -> Result<RedactedPayloadPage, String> {
    if !deep_index_policy().payload_read_enabled() {
        return Ok(unavailable_page());
    }
    let section = if section.trim().is_empty() {
        "summary".to_string()
    } else {
        section.trim().to_string()
    };
    let max_bytes = max_bytes.unwrap_or(PAYLOAD_DEFAULT_MAX_BYTES);
    tauri::async_runtime::spawn_blocking(move || {
        if !deep_index_policy().payload_read_enabled() {
            return Ok(unavailable_page());
        }
        let plan = activity_db(|conn| {
            let event_row: Option<(String, String, Option<i64>, Option<String>)> = conn
                .query_row(
                    "SELECT session_key, source_file_path, source_offset, payload_hash
                 FROM session_events WHERE event_key = ?1",
                    rusqlite::params![event_key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|error| format!("ERR_ACTIVITY_QUERY_EVENT: {error}"))?;
            let Some((session_key, source_file_path, source_offset, payload_hash)) = event_row
            else {
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
                return Ok(None);
            };
            if crate::activity::adapter::validate_payload_source_path(&source_file_path).is_err() {
                return Ok(None);
            }
            Ok(Some((
                tool,
                SafeSourceRef {
                    source_file_id: None,
                    source_file_path,
                    source_offset,
                    fingerprint: payload_hash,
                    event_key: Some(event_key.clone()),
                },
            )))
        })?;
        let Some((tool, source_ref)) = plan else {
            return Ok(unavailable_page());
        };
        if !deep_index_policy().payload_read_enabled() {
            return Ok(unavailable_page());
        }
        let Some(adapter) = registry().get_adapter(&tool) else {
            return Ok(unavailable_page());
        };
        adapter
            .read_payload(&source_ref, &section, max_bytes, cursor)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("ERR_ACTIVITY_PAYLOAD_JOIN: {error}"))?
}

fn rebuild_activity_without_db_lock(
    scope: &str,
    policy: db::ActivityPersistencePolicy,
) -> Result<RebuildResult, String> {
    let sources: Vec<SessionSourceRef> = if scope == "all" {
        activity_db(|conn| indexer::list_indexable_sessions(conn, &registry()))?
            .into_iter()
            .map(|session| SessionSourceRef {
                session_id: session.session_id,
                tool: session.tool,
                primary_file_path: session.primary_file_path,
                source_file_id: None,
            })
            .collect()
    } else if let Some(session_key) = scope.strip_prefix("session:") {
        let session_key = session_key.trim();
        if session_key.is_empty() {
            return Err("ERR_ACTIVITY_INVALID_SCOPE: empty session key".to_string());
        }
        vec![
            activity_db(|conn| indexer::find_source_ref(conn, session_key))?.ok_or_else(|| {
                "ERR_ACTIVITY_SESSION_NOT_FOUND: session not found in local sessions".to_string()
            })?,
        ]
    } else if scope.starts_with("older_than_days:") {
        return Err(
            "ERR_ACTIVITY_UNSUPPORTED_SCOPE: older_than_days is a purge-only scope".to_string(),
        );
    } else {
        return Err(format!(
            "ERR_ACTIVITY_INVALID_SCOPE: unsupported scope: {scope}"
        ));
    };

    let mut result = RebuildResult::default();
    for source in sources {
        if deep_index_policy() != policy {
            result.sessions_failed += 1;
            result
                .errors
                .push("ERR_ACTIVITY_POLICY_CHANGED: deep index policy changed".to_string());
            break;
        }
        match ensure_source_indexed_without_db_lock(&source, policy) {
            Ok(Some(written)) => {
                result.sessions_indexed += 1;
                result.events_written += written as i64;
            }
            Ok(None) => {}
            Err(error) => {
                result.sessions_failed += 1;
                result.errors.push(error);
            }
        }
    }
    Ok(result)
}

/// 重建深度活动索引（scope："all" | "session:<session_key>"）。
///
/// 深度索引关闭时返回空结果（不触发任何索引写入）；开启时在
/// spawn_blocking 后台线程执行，文件扫描与解析不占用全局 SQLite 互斥锁；
/// 每个会话仅在读取指纹和提交事务时短暂持锁。
#[tauri::command]
pub async fn rebuild_session_activity_index(
    _app: tauri::AppHandle,
    scope: String,
) -> Result<RebuildResult, String> {
    let policy = deep_index_policy();
    if !policy.indexing_enabled() {
        return Ok(RebuildResult::default());
    }
    tauri::async_runtime::spawn_blocking(move || rebuild_activity_without_db_lock(&scope, policy))
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
// 导出（M3；21.5：默认不含正文与工具 payload，前端范围预览确认后显式开启）
// ---------------------------------------------------------------------------

/// 会话元信息（local_sessions 聚合行；导出头）。
#[derive(Debug, Clone)]
struct ExportSessionMeta {
    session_key: String,
    tool: String,
    topic: Option<String>,
    session_name: Option<String>,
    project: Option<String>,
}

/// 导出用事件行（含 payload 定位信息；内容全部来自已脱敏字段）。
struct ExportEventRow {
    event_key: String,
    sequence: i64,
    timestamp_ms: Option<i64>,
    kind: SessionEventKind,
    status: Option<EventStatus>,
    summary: Option<String>,
    source_file_path: String,
    source_offset: Option<i64>,
    payload_hash: Option<String>,
    tool: Option<ToolInvocationSummary>,
    request_links: Vec<RequestEventLink>,
    /// include_payloads 时填充 `{section: 脱敏文本}`，否则 Null。
    payload: serde_json::Value,
}

struct PreparedActivityExport {
    meta: ExportSessionMeta,
    events: Vec<ExportEventRow>,
}

/// 会话元信息查询；local_sessions 无该会话返回 None。
/// 行元组：(tool, topic, session_name, project_name, project_key)。
type ExportSessionMetaRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn query_export_session_meta(
    conn: &rusqlite::Connection,
    session_key: &str,
) -> Result<Option<ExportSessionMeta>, String> {
    let row: Option<ExportSessionMetaRow> = conn
        .query_row(
            "SELECT tool, topic, session_name, project_name, project_key
             FROM local_sessions WHERE session_id = ?1",
            params![session_key],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("ERR_ACTIVITY_EXPORT_QUERY_SESSION: {error}"))?;
    Ok(row.map(
        |(tool, topic, session_name, project_name, project_key)| ExportSessionMeta {
            session_key: session_key.to_string(),
            tool,
            topic,
            session_name,
            // project 取 project_name，缺失回退 project_key。
            project: project_name.or(project_key),
        },
    ))
}

/// 按 sequence 升序查询会话全部事件（含工具调用摘要与 payload 定位信息；
/// 使用原始 source_file_path，不做 ~ 显示脱敏——读取路径来自 DB 定位链）。
fn query_export_events(
    conn: &rusqlite::Connection,
    session_key: &str,
) -> Result<Vec<ExportEventRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT e.event_key, e.sequence, e.timestamp_ms, e.kind, e.status,
                    e.summary_redacted, e.source_file_path, e.source_offset, e.payload_hash,
                    ti.invocation_key, ti.tool_name, ti.family, ti.duration_ms,
                    ti.input_bytes, ti.output_bytes, ti.input_keys_json, ti.result_kind
             FROM session_events e
             LEFT JOIN session_tool_invocations ti ON ti.event_key = e.event_key
             WHERE e.session_key = ?1
             ORDER BY e.sequence ASC",
        )
        .map_err(|error| format!("ERR_ACTIVITY_EXPORT_PREPARE_EVENTS: {error}"))?;
    let rows = stmt
        .query_map(params![session_key], |row| {
            let invocation_key: Option<String> = row.get(9)?;
            let tool = match &invocation_key {
                Some(invocation_key) => {
                    let raw_name: String = row.get(10)?;
                    let input_keys_json: String = row.get(15)?;
                    Some(ToolInvocationSummary {
                        invocation_key: invocation_key.clone(),
                        raw_name: raw_name.clone(),
                        normalized_name: raw_name,
                        family: row.get(11)?,
                        duration_ms: row.get(12)?,
                        input_bytes: row.get(13)?,
                        output_bytes: row.get(14)?,
                        input_keys: serde_json::from_str(&input_keys_json).unwrap_or_default(),
                        result_kind: row.get(16)?,
                    })
                }
                None => None,
            };
            Ok(ExportEventRow {
                event_key: row.get(0)?,
                sequence: row.get(1)?,
                timestamp_ms: row.get(2)?,
                kind: SessionEventKind::parse_db(&row.get::<_, String>(3)?),
                status: row
                    .get::<_, Option<String>>(4)?
                    .as_deref()
                    .map(EventStatus::parse_db),
                summary: row.get(5)?,
                source_file_path: row.get(6)?,
                source_offset: row.get(7)?,
                payload_hash: row.get(8)?,
                tool,
                request_links: Vec::new(),
                payload: serde_json::Value::Null,
            })
        })
        .map_err(|error| format!("ERR_ACTIVITY_EXPORT_QUERY_EVENTS: {error}"))?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row.map_err(|error| format!("ERR_ACTIVITY_EXPORT_READ_EVENT: {error}"))?);
    }
    Ok(events)
}

/// 批量挂载事件-请求关联（一次 IN 查询，避免 N+1）。
fn attach_export_request_links(
    conn: &rusqlite::Connection,
    events: &mut [ExportEventRow],
) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let keys: Vec<String> = events.iter().map(|event| event.event_key.clone()).collect();
    let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT event_key, request_key, strength FROM session_event_request_links
         WHERE event_key IN ({placeholders})"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| format!("ERR_ACTIVITY_EXPORT_PREPARE_LINKS: {error}"))?;
    let link_rows = stmt
        .query_map(params_from_iter(keys.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                RequestEventLink {
                    request_key: row.get(1)?,
                    strength: RequestLinkStrength::parse_db(&row.get::<_, String>(2)?),
                },
            ))
        })
        .map_err(|error| format!("ERR_ACTIVITY_EXPORT_QUERY_LINKS: {error}"))?;
    let mut by_event: HashMap<String, Vec<RequestEventLink>> = HashMap::new();
    for link_row in link_rows {
        let (event_key, link) =
            link_row.map_err(|error| format!("ERR_ACTIVITY_EXPORT_READ_LINK: {error}"))?;
        by_event.entry(event_key).or_default().push(link);
    }
    for event in events.iter_mut() {
        event.request_links = by_event.remove(&event.event_key).unwrap_or_default();
    }
    Ok(())
}

/// 读取单个事件的完整脱敏 payload（cursor 循环翻页；每个 section 预算
/// [`EXPORT_EVENT_PAYLOAD_MAX_BYTES`] 字节，工具事件 input+output 各计
/// 一次）。返回 `(payload 对象, 已读字节)`；内容不可用或读取失败时返回
/// 空对象，不报错。
fn read_event_payload(
    adapter: &dyn SessionActivityAdapter,
    source_ref: &SafeSourceRef,
    kind: SessionEventKind,
    include_summaries: bool,
) -> Result<(serde_json::Map<String, serde_json::Value>, usize), String> {
    let sections: &[&str] = match kind {
        SessionEventKind::ToolInvocation => &["input", "output"],
        SessionEventKind::ToolResult => &["output"],
        // 普通消息事件读正文摘要（与 include_summaries 联动，避免导出
        // 关闭摘要时仍通过 payload 通道带出正文）。
        _ => {
            if include_summaries {
                &["summary"]
            } else {
                &[]
            }
        }
    };
    let mut out = serde_json::Map::new();
    let mut total = 0usize;
    for section in sections {
        let mut cursor: Option<String> = None;
        let mut collected = String::new();
        loop {
            let budget = EXPORT_EVENT_PAYLOAD_MAX_BYTES.saturating_sub(collected.len());
            if budget == 0 {
                break;
            }
            let page = match adapter.read_payload(source_ref, section, budget, cursor.clone()) {
                Ok(page) => page,
                Err(error) => {
                    // 单事件 payload 读取失败（如源行损坏）按不可用跳过，
                    // 不因单个事件中断整个导出。
                    eprintln!("[activity-export] payload read skipped: {error}");
                    break;
                }
            };
            if page.content_state != ContentState::Available {
                break;
            }
            collected.push_str(&page.content);
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        if !collected.is_empty() {
            total += collected.len();
            out.insert(section.to_string(), serde_json::Value::String(collected));
        }
    }
    Ok((out, total))
}

/// session_key → 文件名字段：只保留 `[A-Za-z0-9._-]`，其余替换为 `_`，
/// 超长截断到 80 字符（消毒后全 ASCII，字节截断安全）；空结果回退 "session"。
fn sanitize_session_key(session_key: &str) -> String {
    let mut out = String::with_capacity(session_key.len());
    for ch in session_key.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.len() > EXPORT_FILE_KEY_MAX_CHARS {
        out.truncate(EXPORT_FILE_KEY_MAX_CHARS);
    }
    if out.is_empty() {
        out.push_str("session");
    }
    out
}

/// 事件 → JSON 对象（选项关闭的字段直接省略）。
fn event_to_json(event: &ExportEventRow, options: &ExportOptions) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("sequence".to_string(), serde_json::json!(event.sequence));
    obj.insert("kind".to_string(), serde_json::json!(event.kind.as_str()));
    if let Some(status) = &event.status {
        obj.insert("status".to_string(), serde_json::json!(status.as_str()));
    }
    if let Some(timestamp_ms) = event.timestamp_ms {
        obj.insert("timestampMs".to_string(), serde_json::json!(timestamp_ms));
    }
    if options.include_summaries {
        if let Some(summary) = &event.summary {
            obj.insert("summary".to_string(), serde_json::json!(summary));
        }
    }
    if options.include_tool_summaries {
        if let Some(tool) = &event.tool {
            let mut tool_obj = serde_json::Map::new();
            tool_obj.insert(
                "invocationKey".to_string(),
                serde_json::json!(tool.invocation_key),
            );
            tool_obj.insert(
                "toolName".to_string(),
                serde_json::json!(tool.normalized_name),
            );
            tool_obj.insert("family".to_string(), serde_json::json!(tool.family));
            tool_obj.insert(
                "durationMs".to_string(),
                serde_json::json!(tool.duration_ms),
            );
            tool_obj.insert(
                "inputBytes".to_string(),
                serde_json::json!(tool.input_bytes),
            );
            tool_obj.insert(
                "outputBytes".to_string(),
                serde_json::json!(tool.output_bytes),
            );
            tool_obj.insert("inputKeys".to_string(), serde_json::json!(tool.input_keys));
            tool_obj.insert(
                "resultKind".to_string(),
                serde_json::json!(tool.result_kind),
            );
            obj.insert("tool".to_string(), serde_json::Value::Object(tool_obj));
        }
    }
    if options.include_request_links && !event.request_links.is_empty() {
        let links: Vec<serde_json::Value> = event
            .request_links
            .iter()
            .map(|link| {
                serde_json::json!({
                    "requestKey": link.request_key,
                    "strength": link.strength.as_str(),
                })
            })
            .collect();
        obj.insert("requestLinks".to_string(), serde_json::Value::Array(links));
    }
    if options.include_payloads && !event.payload.is_null() {
        obj.insert("payload".to_string(), event.payload.clone());
    }
    serde_json::Value::Object(obj)
}

fn render_json_export(
    meta: &ExportSessionMeta,
    events: &[ExportEventRow],
    options: &ExportOptions,
) -> String {
    let mut session = serde_json::Map::new();
    session.insert(
        "sessionKey".to_string(),
        serde_json::json!(meta.session_key),
    );
    session.insert("tool".to_string(), serde_json::json!(meta.tool));
    if let Some(topic) = &meta.topic {
        session.insert("topic".to_string(), serde_json::json!(topic));
    }
    if let Some(session_name) = &meta.session_name {
        session.insert("sessionName".to_string(), serde_json::json!(session_name));
    }
    if let Some(project) = &meta.project {
        session.insert("project".to_string(), serde_json::json!(project));
    }
    let mut root = serde_json::Map::new();
    root.insert("session".to_string(), serde_json::Value::Object(session));
    root.insert(
        "events".to_string(),
        serde_json::Value::Array(
            events
                .iter()
                .map(|event| event_to_json(event, options))
                .collect(),
        ),
    );
    serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .unwrap_or_else(|_| "{}".to_string())
}

/// CSV 字段转义（RFC 4180：含逗号/引号/换行时双引号包裹，内部引号加倍）。
fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn render_csv_export(
    _meta: &ExportSessionMeta,
    events: &[ExportEventRow],
    options: &ExportOptions,
) -> String {
    let mut out = String::new();
    let mut header = vec![
        "sequence",
        "kind",
        "status",
        "timestamp_ms",
        "summary",
        "tool_name",
        "family",
        "request_links",
    ];
    if options.include_payloads {
        header.push("payload");
    }
    out.push_str(&header.join(","));
    out.push('\n');
    for event in events {
        let mut row: Vec<String> = Vec::with_capacity(header.len());
        row.push(event.sequence.to_string());
        row.push(event.kind.as_str().to_string());
        row.push(
            event
                .status
                .map(|status| status.as_str().to_string())
                .unwrap_or_default(),
        );
        row.push(
            event
                .timestamp_ms
                .map(|value| value.to_string())
                .unwrap_or_default(),
        );
        row.push(if options.include_summaries {
            event.summary.clone().unwrap_or_default()
        } else {
            String::new()
        });
        let (tool_name, family) = match (&event.tool, options.include_tool_summaries) {
            (Some(tool), true) => (tool.normalized_name.clone(), tool.family.clone()),
            _ => (String::new(), String::new()),
        };
        row.push(tool_name);
        row.push(family);
        row.push(if options.include_request_links {
            event
                .request_links
                .iter()
                .map(|link| format!("{}:{}", link.request_key, link.strength.as_str()))
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            String::new()
        });
        if options.include_payloads {
            row.push(if event.payload.is_null() {
                String::new()
            } else {
                serde_json::to_string(&event.payload).unwrap_or_default()
            });
        }
        let escaped: Vec<String> = row.iter().map(|field| csv_escape(field)).collect();
        out.push_str(&escaped.join(","));
        out.push('\n');
    }
    out
}

/// 导出核心实现（与 Tauri 解耦，便于单元测试）：查询已脱敏的会话事件、
/// 可选按需读取脱敏 payload（cursor 翻页读全，单事件 64KB、累计 50MB），
/// 写入 `~/.usagemeter/exports/activity-{sanitized}-{ts}.{json|csv}`。
///
/// 隐私铁律：导出内容全部来自已脱敏字段（summary_redacted 已脱敏；
/// payload 经适配器脱敏）；`include_payloads` 默认 false，后端只执行选项。
fn prepare_session_activity_export(
    conn: &rusqlite::Connection,
    session_key: &str,
) -> Result<PreparedActivityExport, String> {
    let meta = query_export_session_meta(conn, session_key)?.ok_or_else(|| {
        "ERR_ACTIVITY_EXPORT_SESSION_NOT_FOUND: session not found in local sessions".to_string()
    })?;
    let mut events = query_export_events(conn, session_key)?;
    attach_export_request_links(conn, &mut events)?;
    Ok(PreparedActivityExport { meta, events })
}

fn finish_session_activity_export(
    mut prepared: PreparedActivityExport,
    session_key: &str,
    options: &ExportOptions,
) -> Result<ExportResult, String> {
    let meta = &prepared.meta;
    let events = &mut prepared.events;
    let mut payload_included = false;
    let mut truncated = false;
    let mut payload_total = 0usize;
    if options.include_payloads {
        if let Some(adapter) = registry().get_adapter(&meta.tool) {
            for event in events.iter_mut() {
                if payload_total >= EXPORT_TOTAL_PAYLOAD_MAX_BYTES {
                    truncated = true;
                    break;
                }
                let source_ref = SafeSourceRef {
                    source_file_id: None,
                    source_file_path: event.source_file_path.clone(),
                    source_offset: event.source_offset,
                    fingerprint: event.payload_hash.clone(),
                    event_key: Some(event.event_key.clone()),
                };
                if crate::activity::adapter::validate_payload_source_path(&event.source_file_path)
                    .is_err()
                {
                    continue;
                }
                let (payload, bytes) = read_event_payload(
                    adapter,
                    &source_ref,
                    event.kind,
                    options.include_summaries,
                )?;
                if !payload.is_empty() {
                    payload_included = true;
                    event.payload = serde_json::Value::Object(payload);
                    payload_total += bytes;
                }
            }
        }
    }

    let format = options.format.trim().to_ascii_lowercase();
    let (content, ext) = match format.as_str() {
        "json" => (render_json_export(meta, events, options), "json"),
        "csv" => (render_csv_export(meta, events, options), "csv"),
        other => {
            return Err(format!(
                "ERR_ACTIVITY_EXPORT_FORMAT: unsupported format: {other}"
            ));
        }
    };

    let export_dir = crate::utils::usagemeter_dir()?.join("exports");
    std::fs::create_dir_all(&export_dir)
        .map_err(|error| format!("ERR_ACTIVITY_EXPORT_MKDIR: {error}"))?;
    let file_name = format!(
        "activity-{}-{}.{ext}",
        sanitize_session_key(session_key),
        chrono::Utc::now().timestamp()
    );
    let file_path = export_dir.join(file_name);
    std::fs::write(&file_path, content)
        .map_err(|error| format!("ERR_ACTIVITY_EXPORT_WRITE: {error}"))?;
    Ok(ExportResult {
        file_path: file_path.to_string_lossy().to_string(),
        row_count: events.len() as i64,
        payload_included,
        truncated,
    })
}

/// 导出会话深度活动（M3；21.5：导出高风险内容前前端负责范围预览，
/// 默认不选择正文与工具 payload）。
///
/// 深度索引关闭时返回明确错误（不导出不存在/残留的深度数据）。
#[tauri::command]
pub async fn export_session_activity(
    _app: tauri::AppHandle,
    session_key: String,
    options: ExportOptions,
) -> Result<ExportResult, String> {
    let policy = deep_index_policy();
    if !policy.indexing_enabled() {
        return Err("ERR_ACTIVITY_EXPORT_DISABLED: deep index is off".to_string());
    }
    if options.include_payloads && !policy.payload_read_enabled() {
        return Err("ERR_ACTIVITY_EXPORT_PAYLOAD_DISABLED: on-demand content is off".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let current_policy = deep_index_policy();
        if !current_policy.indexing_enabled() {
            return Err("ERR_ACTIVITY_EXPORT_DISABLED: deep index is off".to_string());
        }
        if options.include_payloads && !current_policy.payload_read_enabled() {
            return Err(
                "ERR_ACTIVITY_EXPORT_PAYLOAD_DISABLED: on-demand content is off".to_string(),
            );
        }
        let prepared = activity_db(|conn| prepare_session_activity_export(conn, &session_key))?;
        let current_policy = deep_index_policy();
        if !current_policy.indexing_enabled()
            || (options.include_payloads && !current_policy.payload_read_enabled())
        {
            return Err("ERR_ACTIVITY_EXPORT_DISABLED: privacy level changed".to_string());
        }
        finish_session_activity_export(prepared, &session_key, &options)
    })
    .await
    .map_err(|error| format!("ERR_ACTIVITY_EXPORT_JOIN: {error}"))?
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
    fn deep_index_gate_uses_persisted_level_not_ipc_settings() {
        use crate::test_support::env_lock;
        let _guard = env_lock();
        let previous_home = std::env::var_os("HOME");
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("HOME", dir.path());
        let settings_dir = dir.path().join(".usagemeter");
        std::fs::create_dir_all(&settings_dir).expect("create settings dir");

        // 无任何持久化设置：默认 off → 深度索引关闭。
        assert!(deep_index_disabled());

        // 持久化档位 structured（偏好文件为权威落点）→ 门控打开。
        std::fs::write(
            settings_dir.join("settings.json"),
            serde_json::json!({ "settingsVersion": 3, "deepIndexLevel": "structured" }).to_string(),
        )
        .expect("write preferences");
        assert!(
            !deep_index_disabled(),
            "persisted structured level must open the gate"
        );
        assert!(!deep_index_policy().payload_read_enabled());

        std::fs::write(
            settings_dir.join("settings.json"),
            serde_json::json!({ "settingsVersion": 3, "deepIndexLevel": "ondemand" }).to_string(),
        )
        .expect("write on-demand preferences");
        assert!(deep_index_policy().payload_read_enabled());

        // 持久化档位 off → 门控关闭（IPC settings 不再参与判定，任意调用方
        // 无法以传参伪造开启）。
        std::fs::write(
            settings_dir.join("settings.json"),
            serde_json::json!({ "settingsVersion": 3, "deepIndexLevel": "off" }).to_string(),
        )
        .expect("write preferences");
        assert!(deep_index_disabled());

        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
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
