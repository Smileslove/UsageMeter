use super::types::ProxyState;
use crate::models::AppSettings;
use crate::proxy::SessionStats;

/// 获取会话列表（按最后修改时间倒序，支持分页）
/// 数据源逻辑：
/// - JSONL：会话元信息（项目名、主题、token 统计）
/// - session_stats 表：性能指标（速率、TTFT、耗时）
#[tauri::command]
pub async fn get_sessions(
    limit: i64,
    offset: i64,
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<SessionStats>, String> {
    crate::unified_usage::get_merged_sessions(&settings, limit, offset).await
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
#[tauri::command]
pub async fn get_project_stats(
    settings: AppSettings,
    _proxy_state: tauri::State<'_, ProxyState>,
) -> Result<Vec<crate::proxy::ProjectStats>, String> {
    crate::unified_usage::get_merged_project_stats(&settings).await
}
