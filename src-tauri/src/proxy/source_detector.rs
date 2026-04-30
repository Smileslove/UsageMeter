//! API 来源检测器
//!
//! 从请求中提取 API Key 前缀和 Base URL，用于自动识别和注册来源

use crate::commands::load_settings;
use crate::models::{ApiSource, AppSettings, SOURCE_COLORS};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// 计算来源的稳定 ID（基于 key 前缀 + base_url）
///
/// 返回 SHA256 哈希的前 16 位十六进制字符串
pub fn compute_source_id(key_prefix: &str, base_url: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key_prefix.as_bytes());
    hasher.update(b"|");
    if let Some(url) = base_url {
        hasher.update(url.as_bytes());
    }
    let hash = hasher.finalize();
    format!("{:016x}", u64::from_be_bytes(hash[..8].try_into().unwrap()))
}

/// 标准化 base_url：官方 Anthropic 地址返回 None
///
/// Anthropic 官方地址包括：
/// - https://api.anthropic.com
/// - api.anthropic.com (无协议前缀)
pub fn normalize_base_url(url: &str) -> Option<String> {
    let url_lower = url.to_lowercase();
    // 官方 Anthropic 地址返回 None
    if url_lower.contains("api.anthropic.com") {
        None
    } else {
        Some(url.to_string())
    }
}

/// 提取 API Key 的前 N 位作为前缀
///
/// 默认取前 12 位，若 key 长度不足则取全部
pub fn extract_key_prefix(api_key: &str, max_len: usize) -> String {
    let len = api_key.len().min(max_len);
    api_key[..len].to_string()
}

/// 从请求中检测来源信息
///
/// # 参数
/// - `api_key`: x-api-key 头的完整值
/// - `target_base_url`: 请求转发的目标地址
/// - `sources`: 现有来源列表（可变引用，用于更新 last_seen_ms）
///
/// # 返回
/// - `(key_prefix, normalized_base_url, is_new_source)`:
///   - `key_prefix`: 提取的 API Key 前缀（前 12 位）
///   - `normalized_base_url`: 标准化后的 base_url
///   - `is_new_source`: 是否为新发现的来源
pub fn detect_source_info(
    api_key: &str,
    target_base_url: &str,
    sources: &[ApiSource],
) -> (String, Option<String>, bool) {
    let prefix = extract_key_prefix(api_key, 12);
    let base_url = normalize_base_url(target_base_url);

    // 查找匹配的已有来源
    let found = sources
        .iter()
        .any(|s| s.api_key_prefixes.contains(&prefix) && s.base_url == base_url);

    (prefix, base_url, !found)
}

/// 获取当前时间戳（毫秒）
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 创建新的来源对象
///
/// 自动分配颜色并生成 ID
pub fn create_new_source(
    api_key_prefix: String,
    base_url: Option<String>,
    existing_count: usize,
) -> ApiSource {
    let id = compute_source_id(&api_key_prefix, base_url.as_deref());
    let color = SOURCE_COLORS[existing_count % SOURCE_COLORS.len()].to_string();
    let now = now_ms();

    ApiSource {
        id,
        display_name: None,
        base_url,
        api_key_prefixes: vec![api_key_prefix],
        api_key_notes: HashMap::new(),
        color,
        icon: None,
        auto_detected: true,
        first_seen_ms: now,
        last_seen_ms: now,
    }
}

/// 注册新来源到设置
///
/// 如果是新来源，会自动添加到 settings.source_aware.sources 列表
/// 返回 (是否为新来源, 更新后的设置)
pub fn register_source_to_settings(api_key: &str, target_base_url: &str) -> (bool, AppSettings) {
    let mut settings = load_settings().unwrap_or_default();
    let prefix = extract_key_prefix(api_key, 12);
    let base_url = normalize_base_url(target_base_url);

    // 查找匹配的已有来源
    let found = settings
        .source_aware
        .sources
        .iter()
        .any(|s| s.api_key_prefixes.contains(&prefix) && s.base_url == base_url);

    if found {
        // 更新最近使用时间
        for source in settings.source_aware.sources.iter_mut() {
            if source.api_key_prefixes.contains(&prefix) && source.base_url == base_url {
                source.last_seen_ms = now_ms();
                break;
            }
        }
        (false, settings)
    } else {
        // 创建新来源
        let new_source = create_new_source(prefix, base_url, settings.source_aware.sources.len());
        settings.source_aware.sources.push(new_source);
        (true, settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_source_id() {
        let id1 = compute_source_id("sk-ant-api03", Some("https://openrouter.ai/api/v1"));
        let id2 = compute_source_id("sk-ant-api03", None);
        let id3 = compute_source_id("sk-ant-api04", Some("https://openrouter.ai/api/v1"));

        // 相同输入产生相同 ID
        assert_eq!(
            id1,
            compute_source_id("sk-ant-api03", Some("https://openrouter.ai/api/v1"))
        );
        // 不同输入产生不同 ID
        assert_ne!(id1, id2);
        assert_ne!(id1, id3);
        // ID 长度为 16
        assert_eq!(id1.len(), 16);
    }

    #[test]
    fn test_normalize_base_url() {
        // 官方 Anthropic 返回 None
        assert_eq!(normalize_base_url("https://api.anthropic.com"), None);
        assert_eq!(normalize_base_url("api.anthropic.com"), None);
        assert_eq!(normalize_base_url("https://api.anthropic.com/v1"), None);

        // 第三方返回原值
        assert_eq!(
            normalize_base_url("https://openrouter.ai/api/v1"),
            Some("https://openrouter.ai/api/v1".to_string())
        );
        assert_eq!(
            normalize_base_url("https://bedrock.amazonaws.com"),
            Some("https://bedrock.amazonaws.com".to_string())
        );
    }

    #[test]
    fn test_extract_key_prefix() {
        assert_eq!(
            extract_key_prefix("sk-ant-api03-xxxx-yyyy", 12),
            "sk-ant-api03"
        );
        assert_eq!(extract_key_prefix("short", 12), "short");
        assert_eq!(extract_key_prefix("", 12), "");
    }

    #[test]
    fn test_detect_source_info() {
        let sources = vec![ApiSource {
            id: "test-id".to_string(),
            display_name: Some("Test".to_string()),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
            api_key_prefixes: vec!["sk-ant-api03".to_string()],
            api_key_notes: HashMap::new(),
            color: "#3B82F6".to_string(),
            icon: None,
            auto_detected: true,
            first_seen_ms: 1000,
            last_seen_ms: 2000,
        }];

        // 匹配已有来源
        let (prefix, base_url, is_new) = detect_source_info(
            "sk-ant-api03-xxxx",
            "https://openrouter.ai/api/v1",
            &sources,
        );
        assert_eq!(prefix, "sk-ant-api03");
        assert_eq!(base_url, Some("https://openrouter.ai/api/v1".to_string()));
        assert!(!is_new);

        // 新来源
        let (prefix2, _base_url2, is_new2) = detect_source_info(
            "sk-ant-api04-yyyy",
            "https://openrouter.ai/api/v1",
            &sources,
        );
        assert_eq!(prefix2, "sk-ant-api04");
        assert!(is_new2);

        // 官方 Anthropic
        let (_prefix3, base_url3, is_new3) =
            detect_source_info("sk-ant-api05-zzzz", "https://api.anthropic.com", &sources);
        assert_eq!(base_url3, None);
        assert!(is_new3);
    }

    #[test]
    fn test_create_new_source() {
        let source = create_new_source(
            "sk-ant-api03".to_string(),
            Some("https://openrouter.ai/api/v1".to_string()),
            0,
        );

        assert_eq!(source.api_key_prefixes, vec!["sk-ant-api03"]);
        assert_eq!(
            source.base_url,
            Some("https://openrouter.ai/api/v1".to_string())
        );
        assert!(source.auto_detected);
        assert!(source.display_name.is_none());
        // 第一个来源使用第一个颜色
        assert_eq!(source.color, SOURCE_COLORS[0]);
    }
}
