//! API 来源检测器
//!
//! 从请求中提取 API Key 前缀和 Base URL，用于自动识别和注册来源

use crate::models::{ApiSource, AppSettings, SOURCE_COLORS};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub struct SourceRegistrationResult {
    pub is_new: bool,
    pub prefix: String,
    pub base_url: Option<String>,
}

/// 计算来源的稳定 ID（基于 key 前缀 + base_url）
///
/// 返回 SHA256 哈希的前 4 字节十六进制字符串（8 位）。
pub fn compute_source_id(key_prefix: &str, base_url: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key_prefix.as_bytes());
    hasher.update(b"|");
    if let Some(url) = base_url {
        hasher.update(url.as_bytes());
    }
    let hash = hasher.finalize();
    format!("{:08x}", u32::from_be_bytes(hash[..4].try_into().unwrap()))
}

/// 将旧版 16 位十六进制 source id 归一为 8 位新格式。
///
/// 新 id 恰好是旧 id 的前 8 位（同一哈希的截断），因此只需截断即可映射。
/// 仅当输入恰好是 16 个十六进制字符时才截断；其他格式（如 `live:xxx`、
/// 人工命名、测试用 id）原样返回，避免误伤。
pub fn normalize_source_id(id: &str) -> String {
    if id.len() == 16 && id.chars().all(|ch| ch.is_ascii_hexdigit()) {
        id[..8].to_string()
    } else {
        id.to_string()
    }
}

/// 将旧版 handle id（`h_`/`oc_`/`rx_`/`gm_` + 十进制 u64，1..=20 位）归一为
/// 8 位十六进制新格式。
///
/// 旧格式由 `format!("{}", u64::from_be_bytes(hash[..8]))` 生成（十进制），
/// 归一规则为：解析为 `u64` 后取高 32 位（即 `hash[..4]`）输出固定 8 位 hex，
/// 与新建 `compute_handle_id` 完全一致。恰好 8 位的 hex 视为已是新格式，原样
/// 保留（旧 id 恰好 8 位十进制 ⇔ hash 前 38 位全零，概率 2⁻³⁸，可忽略）。
/// 未知前缀或非数字/非 hex 内容原样返回。
pub fn normalize_handle_id(id: &str) -> String {
    for prefix in ["h_", "oc_", "rx_", "gm_"] {
        if let Some(rest) = id.strip_prefix(prefix) {
            if !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_hexdigit()) {
                // 新格式：恰好 8 位 hex → 原样（幂等）。
                if rest.len() == 8 {
                    return id.to_string();
                }
                // 旧格式：纯十进制（无 a-f）→ 高 32 位转 8 位 hex。
                if rest.chars().all(|ch| ch.is_ascii_digit()) {
                    if let Ok(value) = rest.parse::<u64>() {
                        return format!("{prefix}{:08x}", (value >> 32) as u32);
                    }
                }
            }
            return id.to_string();
        }
    }
    id.to_string()
}

/// 标准化 base_url：官方 Anthropic 地址返回 None
///
/// Anthropic 官方地址包括：
/// - https://api.anthropic.com
/// - api.anthropic.com (无协议前缀)
pub fn normalize_base_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let host_port_and_path = without_scheme
        .split_once('/')
        .map(|(host_port, _)| host_port)
        .unwrap_or(without_scheme);
    let host = host_port_and_path
        .split_once('@')
        .map(|(_, host)| host)
        .unwrap_or(host_port_and_path)
        .split_once(':')
        .map(|(host, _)| host)
        .unwrap_or(host_port_and_path)
        .trim_matches(|ch| ch == '[' || ch == ']')
        .to_ascii_lowercase();

    if host == "api.anthropic.com" {
        None
    } else {
        Some(trimmed.to_string())
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
        quota_query: None,
        first_seen_ms: now,
        last_seen_ms: now,
    }
}

/// 在已有设置快照中注册来源。
///
/// - 新来源会追加到 `settings.source_aware.sources`
/// - 已有来源只刷新内存中的 `last_seen_ms`
pub fn register_source_to_settings(
    settings: &mut AppSettings,
    api_key: &str,
    target_base_url: &str,
) -> SourceRegistrationResult {
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
        SourceRegistrationResult {
            is_new: false,
            prefix,
            base_url,
        }
    } else {
        // 创建新来源
        let new_source = create_new_source(
            prefix.clone(),
            base_url.clone(),
            settings.source_aware.sources.len(),
        );
        settings.source_aware.sources.push(new_source);
        SourceRegistrationResult {
            is_new: true,
            prefix,
            base_url,
        }
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
        // ID 长度为 8（新版缩短格式）
        assert_eq!(id1.len(), 8);
        // 新 id 是旧 16 位 id 的前 8 位：归一函数可将旧 id 映射回新 id
        let legacy = format!("{:016x}", u64::from_be_bytes([0; 8]));
        assert_eq!(normalize_source_id(&legacy), "00000000");
        // 非 16 hex 格式不受归一影响
        assert_eq!(
            normalize_source_id("live:https://example.com"),
            "live:https://example.com"
        );
        assert_eq!(normalize_source_id("test-id"), "test-id");
        assert_eq!(normalize_source_id("12345678"), "12345678");
    }

    #[test]
    fn test_normalize_handle_id() {
        // 旧格式为十进制 u64（format!("{}", u64)），取高 32 位转 8 位 hex。
        // u64 = 1 → 高 32 位 = 0
        assert_eq!(normalize_handle_id("h_1"), "h_00000000");
        // u64 = 2^32 → 高 32 位 = 1
        assert_eq!(normalize_handle_id("h_4294967296"), "h_00000001");
        // u64::MAX → 高 32 位全 1
        assert_eq!(normalize_handle_id("h_18446744073709551615"), "h_ffffffff");
        // 真实规模旧 id（19-20 位十进制）
        assert_eq!(normalize_handle_id("h_183789673287381192"), "h_028cf3bb");
        // 各前缀
        assert_eq!(normalize_handle_id("oc_4294967296"), "oc_00000001");
        assert_eq!(normalize_handle_id("rx_4294967296"), "rx_00000001");
        assert_eq!(normalize_handle_id("gm_4294967296"), "gm_00000001");
        // 已是 8 位 hex（新格式，含全数字 8 位）→ 原样，且幂等
        assert_eq!(normalize_handle_id("h_028cfc5c"), "h_028cfc5c");
        assert_eq!(normalize_handle_id("h_12345678"), "h_12345678");
        assert_eq!(
            normalize_handle_id(&normalize_handle_id("h_183789673287381192")),
            "h_028cf3bb"
        );
        // 非数字 / 非 hex / 未知前缀 → 原样
        assert_eq!(normalize_handle_id("h_zzzz"), "h_zzzz");
        assert_eq!(
            normalize_handle_id("h_a3b4c5d6e7f8091a"),
            "h_a3b4c5d6e7f8091a"
        );
        assert_eq!(
            normalize_handle_id("live:https://example.com"),
            "live:https://example.com"
        );
        assert_eq!(normalize_handle_id("custom"), "custom");
    }

    #[test]
    fn test_normalize_base_url() {
        // 官方 Anthropic 返回 None
        assert_eq!(normalize_base_url("https://api.anthropic.com"), None);
        assert_eq!(normalize_base_url("api.anthropic.com"), None);
        assert_eq!(normalize_base_url("https://api.anthropic.com/v1"), None);
        assert_eq!(normalize_base_url("https://API.ANTHROPIC.COM:443/v1"), None);

        // 第三方返回原值
        assert_eq!(
            normalize_base_url("https://openrouter.ai/api/v1"),
            Some("https://openrouter.ai/api/v1".to_string())
        );
        assert_eq!(
            normalize_base_url("https://bedrock.amazonaws.com"),
            Some("https://bedrock.amazonaws.com".to_string())
        );
        assert_eq!(
            normalize_base_url("https://api.anthropic.com.evil.example/v1"),
            Some("https://api.anthropic.com.evil.example/v1".to_string())
        );
        assert_eq!(
            normalize_base_url("https://proxy.example.com/forward/api.anthropic.com"),
            Some("https://proxy.example.com/forward/api.anthropic.com".to_string())
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
            quota_query: None,
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

    #[test]
    fn test_register_source_to_settings_updates_existing_source_without_duplication() {
        let mut settings = AppSettings::default();
        settings.source_aware.sources.push(ApiSource {
            id: "test-id".to_string(),
            display_name: Some("Test".to_string()),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
            api_key_prefixes: vec!["sk-ant-api03".to_string()],
            api_key_notes: HashMap::new(),
            color: "#3B82F6".to_string(),
            icon: None,
            auto_detected: true,
            quota_query: None,
            first_seen_ms: 1000,
            last_seen_ms: 1000,
        });

        let result = register_source_to_settings(
            &mut settings,
            "sk-ant-api03-xxxx",
            "https://openrouter.ai/api/v1",
        );

        assert!(!result.is_new);
        assert_eq!(result.prefix, "sk-ant-api03");
        assert_eq!(
            result.base_url,
            Some("https://openrouter.ai/api/v1".to_string())
        );
        assert_eq!(settings.source_aware.sources.len(), 1);
        assert!(settings.source_aware.sources[0].last_seen_ms >= 1000);
    }

    #[test]
    fn test_register_source_to_settings_adds_new_source() {
        let mut settings = AppSettings::default();

        let result = register_source_to_settings(
            &mut settings,
            "sk-ant-api04-yyyy",
            "https://openrouter.ai/api/v1",
        );

        assert!(result.is_new);
        assert_eq!(result.prefix, "sk-ant-api04");
        assert_eq!(
            result.base_url,
            Some("https://openrouter.ai/api/v1".to_string())
        );
        assert_eq!(settings.source_aware.sources.len(), 1);
    }
}
