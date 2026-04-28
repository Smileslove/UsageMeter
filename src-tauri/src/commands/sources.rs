//! API 来源管理相关 Tauri 命令

use crate::commands::{load_settings, save_settings};
use crate::models::ApiSource;

/// 重命名 API 来源
#[tauri::command]
pub async fn rename_api_source(source_id: String, name: String) -> Result<(), String> {
    let mut settings = load_settings()?;

    if let Some(source) = settings
        .source_aware
        .sources
        .iter_mut()
        .find(|s| s.id == source_id)
    {
        source.display_name = if name.is_empty() { None } else { Some(name) };
        source.auto_detected = false; // 用户已手动编辑
        save_settings(settings)?;
        Ok(())
    } else {
        Err(format!("Source not found: {}", source_id))
    }
}

/// 删除 API 来源
///
/// # 参数
/// - `source_id`: 要删除的来源 ID
/// - `also_delete_records`: 是否同时删除数据库中关联的历史请求记录
#[tauri::command]
pub async fn delete_api_source(source_id: String, also_delete_records: bool) -> Result<(), String> {
    let mut settings = load_settings()?;

    // 查找来源
    let source = settings
        .source_aware
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("Source not found: {}", source_id))?
        .clone();

    // 从设置中移除
    settings.source_aware.sources.retain(|s| s.id != source_id);

    // 如果当前激活的过滤器是被删除的来源，清除过滤器
    if settings.source_aware.active_source_filter.as_ref() == Some(&source_id) {
        settings.source_aware.active_source_filter = None;
    }

    save_settings(settings)?;

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

    let mut settings = load_settings()?;

    // 获取源来源
    let source_from = settings
        .source_aware
        .sources
        .iter()
        .find(|s| s.id == source_id_from)
        .ok_or_else(|| format!("Source not found: {}", source_id_from))?
        .clone();

    let target_base_url = settings
        .source_aware
        .sources
        .iter()
        .find(|s| s.id == source_id_into)
        .ok_or_else(|| format!("Target source not found: {}", source_id_into))?
        .base_url
        .clone();

    if source_from.base_url != target_base_url {
        return Err("Cannot merge sources with different base URLs".to_string());
    }

    // 获取目标来源
    let source_into = settings
        .source_aware
        .sources
        .iter_mut()
        .find(|s| s.id == source_id_into)
        .ok_or_else(|| format!("Target source not found: {}", source_id_into))?;

    // 合并 Key 前缀（去重）
    for prefix in source_from.api_key_prefixes {
        if !source_into.api_key_prefixes.contains(&prefix) {
            source_into.api_key_prefixes.push(prefix);
        }
    }

    // 更新最近使用时间
    source_into.last_seen_ms = source_from.last_seen_ms.max(source_into.last_seen_ms);
    source_into.auto_detected = false; // 用户已手动编辑

    // 删除源来源
    settings
        .source_aware
        .sources
        .retain(|s| s.id != source_id_from);

    // 如果当前激活的过滤器是被合并的来源，切换到目标来源
    if settings.source_aware.active_source_filter.as_ref() == Some(&source_id_from) {
        settings.source_aware.active_source_filter = Some(source_id_into);
    }

    save_settings(settings)?;

    Ok(())
}

/// 手动添加 Key 前缀到来源
#[tauri::command]
pub async fn add_key_prefix_to_source(source_id: String, key_prefix: String) -> Result<(), String> {
    if key_prefix.len() < 8 {
        return Err("Key prefix must be at least 8 characters".to_string());
    }

    let mut settings = load_settings()?;

    let target_base_url = settings
        .source_aware
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("Source not found: {}", source_id))?
        .base_url
        .clone();

    // 检查同一 base_url 下的前缀是否已被其他来源使用
    for source in &settings.source_aware.sources {
        if source.id != source_id
            && source.base_url == target_base_url
            && source.api_key_prefixes.contains(&key_prefix)
        {
            return Err(format!("Key prefix already used by source: {}", source.id));
        }
    }

    // 添加前缀到目标来源
    let source = settings
        .source_aware
        .sources
        .iter_mut()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("Source not found: {}", source_id))?;

    if !source.api_key_prefixes.contains(&key_prefix) {
        source.api_key_prefixes.push(key_prefix);
        source.auto_detected = false;
        save_settings(settings)?;
    }

    Ok(())
}

/// 更新 API Key 前缀备注
#[tauri::command]
pub async fn update_api_source_key_note(
    source_id: String,
    key_prefix: String,
    note: String,
) -> Result<(), String> {
    let mut settings = load_settings()?;

    let source = settings
        .source_aware
        .sources
        .iter_mut()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("Source not found: {}", source_id))?;

    if !source.api_key_prefixes.contains(&key_prefix) {
        return Err(format!("Key prefix not found: {}", key_prefix));
    }

    let note = note.trim().to_string();
    if note.is_empty() {
        source.api_key_notes.remove(&key_prefix);
    } else {
        source.api_key_notes.insert(key_prefix, note);
    }
    source.auto_detected = false;

    save_settings(settings)?;
    Ok(())
}

/// 设置当前激活的来源过滤器
#[tauri::command]
pub async fn set_active_source_filter(source_id: Option<String>) -> Result<(), String> {
    let mut settings = load_settings()?;

    // 验证 source_id 有效性（如果不是 None 或 "__unknown__"）
    if let Some(ref id) = source_id {
        if id != "__unknown__" {
            let exists = settings.source_aware.sources.iter().any(|s| &s.id == id);
            if !exists {
                return Err(format!("Source not found: {}", id));
            }
        }
    }

    settings.source_aware.active_source_filter = source_id;
    save_settings(settings)?;

    Ok(())
}

/// 获取所有来源列表
#[tauri::command]
pub async fn get_api_sources() -> Result<Vec<ApiSource>, String> {
    let settings = load_settings()?;
    Ok(settings.source_aware.sources)
}
