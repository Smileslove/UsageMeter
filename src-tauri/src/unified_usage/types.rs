use crate::proxy::UsageRecord;
use crate::session::{LocalRequestRecord, SessionMeta};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMode {
    LocalOnly,
    /// 历史上当 source_aware 启用过滤时进入的分支，现已收敛到
    /// `ProxyWithLocalFallback`；保留枚举值是为了未来可能再次启用或单测覆盖。
    #[allow(dead_code)]
    ProxyOnly,
    ProxyWithLocalFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageOrigin {
    ProxyOnly,
    LocalOnly,
    MergedProxyPreferred,
}

#[derive(Debug, Clone, Default)]
pub struct MergedCoverage {
    pub proxy_backed_requests: u64,
    pub local_only_requests: u64,
    pub merged_overlap_requests: u64,
    pub has_partial_status_coverage: bool,
    pub has_partial_performance_coverage: bool,
}

#[derive(Debug, Clone)]
pub struct MergedRequestFact {
    pub session_id: String,
    pub project_name: Option<String>,
    pub project_path: Option<String>,
    pub api_key_prefix: Option<String>,
    pub request_base_url: Option<String>,
    pub tool: String,
    pub timestamp_sec: i64,
    pub timestamp_ms: i64,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost: f64,
    pub coverage_origin: CoverageOrigin,
    pub status_code: Option<u16>,
    pub duration_ms: Option<u64>,
    pub output_tokens_per_second: Option<f64>,
    pub ttft_ms: Option<u64>,
    /// 用于「按来源分桶」展示的标签：
    /// - 优先取 `api_key_prefix`（最具区分度）
    /// - 否则取 `request_base_url`
    /// - 两者都没有 → None，表示「未识别来源」（仅本地补全的请求会出现这种情况）
    ///
    /// 该字段当前由后端写入、前端展示时消费；编译期标注 dead_code 是为了
    /// 在前端 UI 阶段（阶段 3）尚未接入时不报警。
    #[allow(dead_code)]
    pub source_label: Option<String>,
}

/// 根据 proxy 字段派生面向 UI 的来源标签。
/// 与 `SourceFilter` 的匹配规则保持同源：先看 api_key_prefix，再看 base_url。
fn derive_source_label(
    api_key_prefix: Option<&str>,
    request_base_url: Option<&str>,
) -> Option<String> {
    if let Some(prefix) = api_key_prefix {
        let trimmed = prefix.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(url) = request_base_url {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

impl MergedRequestFact {
    pub fn from_local(record: &LocalRequestRecord, meta: Option<&SessionMeta>, cost: f64) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());

        Self {
            session_id: record.session_id.clone(),
            project_name,
            project_path,
            api_key_prefix: None,
            request_base_url: None,
            tool: record.tool.clone(),
            timestamp_sec: record.timestamp,
            timestamp_ms: record.timestamp.saturating_mul(1000),
            model: record.model.clone(),
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cache_create_tokens: record.cache_create_tokens,
            cache_read_tokens: record.cache_read_tokens,
            total_tokens: record.total_tokens,
            estimated_cost: cost,
            coverage_origin: CoverageOrigin::LocalOnly,
            status_code: None,
            duration_ms: None,
            output_tokens_per_second: None,
            ttft_ms: None,
            // local 无 source 维度——明确标 None 表示「未识别来源」桶
            source_label: None,
        }
    }

    pub fn from_proxy(record: &UsageRecord, meta: Option<&SessionMeta>) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());
        let source_label = derive_source_label(
            record.api_key_prefix.as_deref(),
            record.request_base_url.as_deref(),
        );

        Self {
            session_id: record.session_id.clone().unwrap_or_default(),
            project_name,
            project_path,
            api_key_prefix: record.api_key_prefix.clone(),
            request_base_url: record.request_base_url.clone(),
            tool: record.client_tool.clone(),
            timestamp_sec: record.timestamp / 1000,
            timestamp_ms: record.timestamp,
            model: record.model.clone(),
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cache_create_tokens: record.cache_create_tokens,
            cache_read_tokens: record.cache_read_tokens,
            total_tokens: record.total_tokens,
            estimated_cost: record.estimated_cost,
            coverage_origin: CoverageOrigin::ProxyOnly,
            status_code: Some(record.status_code),
            duration_ms: Some(record.duration_ms),
            output_tokens_per_second: record.output_tokens_per_second,
            ttft_ms: record.ttft_ms,
            source_label,
        }
    }

    /// 合并 proxy 与 local 的同一条请求事实。
    ///
    /// 字段优先级（按字段类别分桶，而不是一刀切「proxy 非零优先」）：
    ///
    /// - 身份字段
    ///   - `session_id` / `project_name` / `project_path`：**local 优先**（transcript 是会话归属的自然事实源）
    ///   - `tool`：local 非空则 local，否则 proxy
    ///   - `api_key_prefix` / `request_base_url`：**proxy 独有**
    /// - 用量字段
    ///   - `input_tokens` / `output_tokens`：**proxy 优先**（响应头/body 最权威）
    ///   - `cache_create_tokens` / `cache_read_tokens`：**local 优先**（JSONL 解析更全，
    ///     proxy 流式 SSE 经常拿到 0）
    ///   - `total_tokens`：**重新计算 = input + output + cache_create + cache_read**，
    ///     避免任一方少算导致 total 漂移
    /// - 时间字段
    ///   - `timestamp_sec`：local 优先（JSONL ISO 时间稳定）
    ///   - `timestamp_ms`：proxy 优先（毫秒精度）
    /// - 性能字段：**仅 proxy**，从不伪造
    /// - 成本字段：proxy `cost_locked = true` 时用 proxy；否则用 local 实时估算
    pub fn merge_proxy_preferred(
        proxy: &UsageRecord,
        local: &LocalRequestRecord,
        meta: Option<&SessionMeta>,
        fallback_cost: f64,
    ) -> Self {
        let project_name = meta.and_then(|m| m.project_name.clone());
        let project_path = meta.and_then(|m| m.cwd.clone());

        let session_id = if !local.session_id.trim().is_empty() {
            local.session_id.clone()
        } else {
            proxy.session_id.clone().unwrap_or_default()
        };
        let tool = if !local.tool.trim().is_empty() {
            local.tool.clone()
        } else {
            proxy.client_tool.clone()
        };
        let model = if !proxy.model.trim().is_empty() {
            proxy.model.clone()
        } else {
            local.model.clone()
        };

        // 用量：proxy 优先 input/output，local 优先 cache_*
        let input_tokens = if proxy.input_tokens > 0 {
            proxy.input_tokens
        } else {
            local.input_tokens
        };
        let output_tokens = if proxy.output_tokens > 0 {
            proxy.output_tokens
        } else {
            local.output_tokens
        };
        let cache_create_tokens = if local.cache_create_tokens > 0 {
            local.cache_create_tokens
        } else {
            proxy.cache_create_tokens
        };
        let cache_read_tokens = if local.cache_read_tokens > 0 {
            local.cache_read_tokens
        } else {
            proxy.cache_read_tokens
        };
        // total 显式重新计算，不取任一方的旧值——避免任一方丢字段导致 total 漂移
        let total_tokens = input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_create_tokens)
            .saturating_add(cache_read_tokens);

        // 成本：cost_locked 表示用户/系统已经按"当时价格"冻结过这条记录，
        // 不能被实时估算覆盖；未 lock 的 proxy cost 与 local 估算同源，
        // 用 local 反而能在用户改价格表后立刻生效。
        let estimated_cost = if proxy.cost_locked {
            proxy.estimated_cost
        } else {
            fallback_cost
        };

        Self {
            session_id,
            project_name,
            project_path,
            api_key_prefix: proxy.api_key_prefix.clone(),
            request_base_url: proxy.request_base_url.clone(),
            tool,
            timestamp_sec: local.timestamp,
            timestamp_ms: proxy.timestamp,
            model,
            input_tokens,
            output_tokens,
            cache_create_tokens,
            cache_read_tokens,
            total_tokens,
            estimated_cost,
            coverage_origin: CoverageOrigin::MergedProxyPreferred,
            status_code: Some(proxy.status_code),
            duration_ms: Some(proxy.duration_ms),
            output_tokens_per_second: proxy.output_tokens_per_second,
            ttft_ms: proxy.ttft_ms,
            // 同一请求 proxy 也有 → source 标签从 proxy 派生
            source_label: derive_source_label(
                proxy.api_key_prefix.as_deref(),
                proxy.request_base_url.as_deref(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_with(
        input: u64,
        output: u64,
        cc: u64,
        cr: u64,
        cost: f64,
        cost_locked: bool,
    ) -> UsageRecord {
        UsageRecord {
            timestamp: 1_700_000_000_500,
            message_id: "msg-1".to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_create_tokens: cc,
            cache_read_tokens: cr,
            total_tokens: input + output + cc + cr,
            model: "claude-3-5-sonnet".to_string(),
            session_id: Some("proxy-session".to_string()),
            status_code: 200,
            duration_ms: 5_000,
            output_tokens_per_second: Some(20.0),
            ttft_ms: Some(800),
            estimated_cost: cost,
            cost_locked,
            api_key_prefix: Some("sk-xxxxxxxxxxxx".to_string()),
            request_base_url: Some("https://api.anthropic.com".to_string()),
            client_tool: "claude_code".to_string(),
            ..Default::default()
        }
    }

    fn local_with(
        input: u64,
        output: u64,
        cc: u64,
        cr: u64,
        session_id: &str,
        timestamp: i64,
    ) -> LocalRequestRecord {
        LocalRequestRecord {
            session_id: session_id.to_string(),
            tool: "claude_code".to_string(),
            timestamp,
            message_id: "msg-1".to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_create_tokens: cc,
            cache_read_tokens: cr,
            total_tokens: input + output + cc + cr,
            model: "claude-3-5-sonnet".to_string(),
            is_subagent: false,
            ..Default::default()
        }
    }

    #[test]
    fn merge_cache_tokens_prefer_local() {
        // proxy 流式响应没拿到 cache，local 解析 JSONL 拿到了——应该用 local 的
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 500, 700, "sess-real", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.123);
        assert_eq!(merged.cache_create_tokens, 500);
        assert_eq!(merged.cache_read_tokens, 700);
    }

    #[test]
    fn merge_input_output_tokens_prefer_proxy() {
        let proxy = proxy_with(150, 250, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess-real", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.input_tokens, 150);
        assert_eq!(merged.output_tokens, 250);
    }

    #[test]
    fn merge_total_tokens_recomputed_from_parts() {
        // proxy 缺 cache、local 缺 input/output 时，total 必须按合并后字段重算
        let proxy = proxy_with(150, 250, 0, 0, 0.0, false);
        let local = local_with(0, 0, 800, 900, "sess-real", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.input_tokens, 150);
        assert_eq!(merged.output_tokens, 250);
        assert_eq!(merged.cache_create_tokens, 800);
        assert_eq!(merged.cache_read_tokens, 900);
        assert_eq!(merged.total_tokens, 150 + 250 + 800 + 900);
    }

    #[test]
    fn merge_estimated_cost_respects_cost_locked() {
        // cost_locked=true：用 proxy 冻结值
        let proxy_locked = proxy_with(100, 200, 0, 0, 0.99, true);
        let local = local_with(100, 200, 100, 100, "sess-real", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy_locked, &local, None, 0.42);
        assert!((merged.estimated_cost - 0.99).abs() < 1e-9);

        // cost_locked=false：忽略 proxy.estimated_cost，使用 local 实时估算（fallback_cost）
        let proxy_unlocked = proxy_with(100, 200, 0, 0, 0.55, false);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy_unlocked, &local, None, 0.42);
        assert!(
            (merged.estimated_cost - 0.42).abs() < 1e-9,
            "unlocked proxy cost should be replaced by local estimate"
        );
    }

    #[test]
    fn merge_session_id_prefers_local_when_present() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        // proxy 提供了一个 session_id（可能是 legacy fallback），但 local 也有 → 用 local
        let local = local_with(100, 200, 0, 0, "real-session-uuid", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.session_id, "real-session-uuid");

        // local 没 session_id → 回退到 proxy
        let local_empty = local_with(100, 200, 0, 0, "", 1_700_000_000);
        let merged_empty =
            MergedRequestFact::merge_proxy_preferred(&proxy, &local_empty, None, 0.0);
        assert_eq!(merged_empty.session_id, "proxy-session");
    }

    #[test]
    fn merge_carries_proxy_performance_fields() {
        // 性能字段绝不应该被「合并」逻辑伪造或丢失——proxy 有就用，local 无能力提供
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.status_code, Some(200));
        assert_eq!(merged.duration_ms, Some(5_000));
        assert_eq!(merged.ttft_ms, Some(800));
        assert_eq!(merged.output_tokens_per_second, Some(20.0));
        assert!(matches!(
            merged.coverage_origin,
            CoverageOrigin::MergedProxyPreferred
        ));
    }

    #[test]
    fn merge_carries_proxy_only_identity_fields() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        assert_eq!(merged.api_key_prefix.as_deref(), Some("sk-xxxxxxxxxxxx"));
        assert_eq!(
            merged.request_base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
    }

    #[test]
    fn merge_uses_local_session_meta_when_available() {
        // SessionMeta 提供 project 信息 → 合并结果 project_name / project_path 应来自它
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let meta = SessionMeta {
            session_id: "sess".to_string(),
            tool: "claude_code".to_string(),
            cwd: Some("/Users/me/work".to_string()),
            project_name: Some("MyProject".to_string()),
            ..Default::default()
        };
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, Some(&meta), 0.0);
        assert_eq!(merged.project_name.as_deref(), Some("MyProject"));
        assert_eq!(merged.project_path.as_deref(), Some("/Users/me/work"));
    }

    #[test]
    fn local_only_has_no_status_code_or_performance() {
        // 验证 from_local 不伪造 proxy 独有的能力字段——这是诚实表达覆盖的底线
        let local = local_with(100, 200, 50, 60, "sess", 1_700_000_000);
        let fact = MergedRequestFact::from_local(&local, None, 0.05);
        assert!(matches!(fact.coverage_origin, CoverageOrigin::LocalOnly));
        assert_eq!(fact.status_code, None);
        assert_eq!(fact.duration_ms, None);
        assert_eq!(fact.ttft_ms, None);
        assert_eq!(fact.output_tokens_per_second, None);
        assert!(fact.api_key_prefix.is_none());
    }

    #[test]
    fn local_only_has_no_source_label() {
        // 本地 transcript 没有 source 维度，必须明确为 None 进入「未识别来源」桶
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let fact = MergedRequestFact::from_local(&local, None, 0.0);
        assert_eq!(fact.source_label, None);
    }

    #[test]
    fn from_proxy_derives_source_label_from_api_key_prefix() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let fact = MergedRequestFact::from_proxy(&proxy, None);
        // proxy_with helper 设置了 api_key_prefix=Some("sk-xxxxxxxxxxxx")
        assert_eq!(fact.source_label.as_deref(), Some("sk-xxxxxxxxxxxx"));
    }

    #[test]
    fn from_proxy_falls_back_to_base_url_when_no_prefix() {
        let proxy = UsageRecord {
            api_key_prefix: None,
            request_base_url: Some("https://custom.endpoint".to_string()),
            ..proxy_with(100, 200, 0, 0, 0.0, false)
        };
        let fact = MergedRequestFact::from_proxy(&proxy, None);
        assert_eq!(
            fact.source_label.as_deref(),
            Some("https://custom.endpoint")
        );
    }

    #[test]
    fn from_proxy_returns_none_label_when_both_missing() {
        let proxy = UsageRecord {
            api_key_prefix: None,
            request_base_url: None,
            ..proxy_with(100, 200, 0, 0, 0.0, false)
        };
        let fact = MergedRequestFact::from_proxy(&proxy, None);
        assert_eq!(fact.source_label, None);
    }

    #[test]
    fn from_proxy_treats_empty_prefix_as_missing() {
        // 防御性：空白/空字符串前缀不应被视为有效来源标签
        let proxy = UsageRecord {
            api_key_prefix: Some("   ".to_string()),
            request_base_url: Some("https://fallback".to_string()),
            ..proxy_with(100, 200, 0, 0, 0.0, false)
        };
        let fact = MergedRequestFact::from_proxy(&proxy, None);
        assert_eq!(fact.source_label.as_deref(), Some("https://fallback"));
    }

    #[test]
    fn merge_proxy_preferred_carries_source_label_from_proxy() {
        let proxy = proxy_with(100, 200, 0, 0, 0.0, false);
        let local = local_with(100, 200, 0, 0, "sess", 1_700_000_000);
        let merged = MergedRequestFact::merge_proxy_preferred(&proxy, &local, None, 0.0);
        // 合并时 source 永远跟 proxy 走——local 本就无 source 维度
        assert_eq!(merged.source_label.as_deref(), Some("sk-xxxxxxxxxxxx"));
    }
}
