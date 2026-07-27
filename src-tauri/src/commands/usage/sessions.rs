use super::helpers::spawn_background_local_usage_sync;
use super::types::ProxyState;
use crate::models::AppSettings;
use crate::proxy::SessionStats;

/// 获取会话列表（按最后修改时间倒序，支持分页）
/// 数据源逻辑：
/// - JSONL：会话元信息（项目名、主题、token 统计）
/// - session_stats 表：性能指标（速率、TTFT、耗时）
///
/// 快照优先：直接以当前 SQLite 数据回答（缓存命中时为毫秒级），全盘扫描
/// 同步移到后台执行；扫描发现新数据时 emit local_usage_synced，前端据此
/// 静默二次刷新。
#[tauri::command]
pub async fn get_sessions(
    app: tauri::AppHandle,
    limit: i64,
    offset: i64,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<SessionStats>, String> {
    spawn_background_local_usage_sync(app);
    crate::unified_usage::get_merged_sessions_no_sync(&settings, limit, offset).await
}

/// 获取单个会话详情
#[tauri::command]
pub async fn get_session_detail(
    session_id: String,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Option<SessionStats>, String> {
    crate::unified_usage::get_merged_session_detail(&settings, &session_id).await
}

/// 获取项目统计（基于所有会话数据聚合）
///
/// 同 get_sessions：快照优先 + 后台同步。
#[tauri::command]
pub async fn get_project_stats(
    app: tauri::AppHandle,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<crate::proxy::ProjectStats>, String> {
    spawn_background_local_usage_sync(app);
    crate::unified_usage::get_merged_project_stats_no_sync(&settings).await
}
