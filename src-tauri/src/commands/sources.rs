//! API 来源管理相关 Tauri 命令

use crate::models::{ApiSource, SourceQuotaBindingConfig};
use crate::settings::{load_settings_blocking as load_settings, update_settings_internal};

/// 重命名 API 来源
#[tauri::command]
pub async fn rename_api_source(source_id: String, name: String) -> Result<(), String> {
    update_settings_internal(move |settings| {
        let source = settings
            .source_aware
            .sources
            .iter_mut()
            .find(|s| s.id == source_id)
            .ok_or_else(|| "Source not found".to_string())?;
        source.display_name = if name.is_empty() { None } else { Some(name) };
        source.auto_detected = false;
        Ok(())
    })
    .map(|_| ())
    .map_err(String::from)
}

/// 删除 API 来源
///
/// # 参数
/// - `source_id`: 要删除的来源 ID
/// - `also_delete_records`: 是否同时删除数据库中关联的历史请求记录
#[tauri::command]
pub async fn delete_api_source(source_id: String, also_delete_records: bool) -> Result<(), String> {
    let (source, _) = update_settings_internal(move |settings| {
        let source = settings
            .source_aware
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .ok_or_else(|| format!("Source not found: {source_id}"))?
            .clone();
        settings.source_aware.sources.retain(|s| s.id != source_id);
        if settings.source_aware.active_source_filter.as_ref() == Some(&source_id) {
            settings.source_aware.active_source_filter = None;
        }
        Ok(source)
    })
    .map_err(String::from)?;

    // 如果需要删除关联的数据库记录
    if also_delete_records {
        delete_source_records(&source).await?;
    }

    Ok(())
}

/// 删除数据库中关联的请求记录
async fn delete_source_records(source: &ApiSource) -> Result<(), String> {
    use crate::proxy::ProxyDatabase;

    let db = ProxyDatabase::new().map_err(|e| format!("Failed to open database: {}", e))?;

    db.delete_records_by_source(&source.api_key_prefixes, source.base_url.as_deref())
        .await
}

/// 合并两个来源（密钥轮换场景）
///
/// 将 `source_id_from` 的 Key 前缀合并到 `source_id_into`
#[tauri::command]
pub async fn merge_api_source(
    source_id_from: String,
    source_id_into: String,
) -> Result<(), String> {
    if source_id_from == source_id_into {
        return Err("Cannot merge source into itself".to_string());
    }

    update_settings_internal(move |settings| {
        let source_from = settings
            .source_aware
            .sources
            .iter()
            .find(|s| s.id == source_id_from)
            .ok_or_else(|| format!("Source not found: {source_id_from}"))?
            .clone();
        let target_base_url = settings
            .source_aware
            .sources
            .iter()
            .find(|s| s.id == source_id_into)
            .ok_or_else(|| format!("Target source not found: {source_id_into}"))?
            .base_url
            .clone();
        if source_from.base_url != target_base_url {
            return Err("Cannot merge sources with different base URLs".to_string());
        }
        let source_into = settings
            .source_aware
            .sources
            .iter_mut()
            .find(|s| s.id == source_id_into)
            .ok_or_else(|| "Target source not found".to_string())?;
        for prefix in source_from.api_key_prefixes {
            if !source_into.api_key_prefixes.contains(&prefix) {
                source_into.api_key_prefixes.push(prefix);
            }
        }
        source_into.last_seen_ms = source_from.last_seen_ms.max(source_into.last_seen_ms);
        source_into.auto_detected = false;
        settings
            .source_aware
            .sources
            .retain(|s| s.id != source_id_from);
        if settings.source_aware.active_source_filter.as_ref() == Some(&source_id_from) {
            settings.source_aware.active_source_filter = Some(source_id_into);
        }
        Ok(())
    })
    .map(|_| ())
    .map_err(String::from)
}

/// 手动添加 Key 前缀到来源
#[tauri::command]
pub async fn add_key_prefix_to_source(source_id: String, key_prefix: String) -> Result<(), String> {
    if key_prefix.len() < 8 {
        return Err("Key prefix must be at least 8 characters".to_string());
    }

    update_settings_internal(move |settings| {
        let target_base_url = settings
            .source_aware
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .ok_or_else(|| format!("Source not found: {source_id}"))?
            .base_url
            .clone();
        if settings.source_aware.sources.iter().any(|source| {
            source.id != source_id
                && source.base_url == target_base_url
                && source.api_key_prefixes.contains(&key_prefix)
        }) {
            return Err("Key prefix already used by another source".to_string());
        }
        let source = settings
            .source_aware
            .sources
            .iter_mut()
            .find(|s| s.id == source_id)
            .ok_or_else(|| format!("Source not found: {source_id}"))?;
        if !source.api_key_prefixes.contains(&key_prefix) {
            source.api_key_prefixes.push(key_prefix);
            source.auto_detected = false;
        }
        Ok(())
    })
    .map(|_| ())
    .map_err(String::from)
}

/// 更新 API Key 前缀备注
#[tauri::command]
pub async fn update_api_source_key_note(
    source_id: String,
    key_prefix: String,
    note: String,
) -> Result<(), String> {
    update_settings_internal(move |settings| {
        let source = settings
            .source_aware
            .sources
            .iter_mut()
            .find(|s| s.id == source_id)
            .ok_or_else(|| format!("Source not found: {source_id}"))?;
        if !source.api_key_prefixes.contains(&key_prefix) {
            return Err(format!("Key prefix not found: {key_prefix}"));
        }
        let note = note.trim().to_string();
        if note.is_empty() {
            source.api_key_notes.remove(&key_prefix);
        } else {
            source.api_key_notes.insert(key_prefix, note);
        }
        source.auto_detected = false;
        Ok(())
    })
    .map(|_| ())
    .map_err(String::from)
}

/// 设置当前激活的来源过滤器
#[tauri::command]
pub async fn set_active_source_filter(source_id: Option<String>) -> Result<(), String> {
    update_settings_internal(move |settings| {
        if let Some(ref id) = source_id {
            if id != "__unknown__"
                && id != crate::models::OFFICIAL_OPENAI_OAUTH_SOURCE_ID
                && id != crate::models::OFFICIAL_GOOGLE_GEMINI_OAUTH_SOURCE_ID
                && id != crate::models::OFFICIAL_ANTHROPIC_CLAUDE_OAUTH_SOURCE_ID
                && id != crate::models::DEEPSEEK_HARNESS_ACCOUNT_SOURCE_ID
                && !settings.source_aware.sources.iter().any(|s| &s.id == id)
            {
                return Err(format!("Source not found: {id}"));
            }
        }
        settings.source_aware.active_source_filter = source_id;
        Ok(())
    })
    .map(|_| ())
    .map_err(String::from)
}

/// 设置当前激活的工具过滤器。过滤器属于实体配置，不能由旧的完整前端快照覆盖。
#[tauri::command]
pub async fn set_active_tool_filter(tool_id: Option<String>) -> Result<(), String> {
    update_settings_internal(move |settings| {
        if let Some(ref id) = tool_id {
            if !settings
                .client_tools
                .profiles
                .iter()
                .any(|profile| &profile.tool == id)
            {
                return Err(format!("Tool not found: {id}"));
            }
        }
        settings.client_tools.active_tool_filter = tool_id;
        Ok(())
    })
    .map(|_| ())
    .map_err(String::from)
}

#[tauri::command]
pub async fn update_api_source_icon(source_id: String, icon: Option<String>) -> Result<(), String> {
    update_settings_internal(move |settings| {
        let source = settings
            .source_aware
            .sources
            .iter_mut()
            .find(|s| s.id == source_id)
            .ok_or_else(|| "Source not found".to_string())?;
        source.icon = icon.filter(|value| !value.trim().is_empty());
        source.auto_detected = false;
        Ok(())
    })
    .map(|_| ())
    .map_err(String::from)
}

#[tauri::command]
pub async fn update_api_source_quota_query(
    source_id: String,
    quota_query: Option<SourceQuotaBindingConfig>,
) -> Result<(), String> {
    update_settings_internal(move |settings| {
        let source = settings
            .source_aware
            .sources
            .iter_mut()
            .find(|s| s.id == source_id)
            .ok_or_else(|| "Source not found".to_string())?;
        source.quota_query = quota_query;
        source.auto_detected = false;
        Ok(())
    })
    .map(|_| ())
    .map_err(String::from)
}

/// 获取所有来源列表
#[tauri::command]
pub async fn get_api_sources() -> Result<Vec<ApiSource>, String> {
    let settings = load_settings()?;
    Ok(settings.source_aware.sources)
}
