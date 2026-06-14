//! 第三方中转 / 余额供应商额度查询（按 base_url 识别）
//!
//! 解析各供应商**自己的**额度/余额接口，按它自己的计费单位返回，
//! 因此无需我们计算 token × 倍率。解析器为纯函数，便于单测。
//!
//! 数据源蓝本：cc-switch `services/coding_plan.rs` + `services/balance.rs`。
//! 凭据（完整 api_key）由调用方提供——其来源在 UsageMeter 架构中待定，
//! 故本模块只负责「给定 base_url + key → 归一化额度」。

use serde_json::Value;

use crate::models::{QuotaKind, QuotaTier, SubscriptionQuota};
use crate::net::HttpClientFactory;

/// 支持的第三方供应商。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RelayProvider {
    Kimi,
    Zhipu,
    ZhipuEn,
    MiniMaxCn,
    MiniMaxGlobal,
    ZenMux,
    DeepSeek,
    StepFun,
    SiliconFlowCn,
    SiliconFlowEn,
    OpenRouter,
    Novita,
}

impl RelayProvider {
    pub fn id(self) -> &'static str {
        match self {
            RelayProvider::Kimi => "kimi",
            RelayProvider::Zhipu | RelayProvider::ZhipuEn => "zhipu",
            RelayProvider::MiniMaxCn | RelayProvider::MiniMaxGlobal => "minimax",
            RelayProvider::ZenMux => "zenmux",
            RelayProvider::DeepSeek => "deepseek",
            RelayProvider::StepFun => "stepfun",
            RelayProvider::SiliconFlowCn | RelayProvider::SiliconFlowEn => "siliconflow",
            RelayProvider::OpenRouter => "openrouter",
            RelayProvider::Novita => "novita",
        }
    }
}

/// 按 base_url 识别供应商。
pub fn detect_relay_provider(base_url: &str) -> Option<RelayProvider> {
    let url = base_url.to_lowercase();
    if url.contains("api.kimi.com/coding") {
        Some(RelayProvider::Kimi)
    } else if url.contains("bigmodel.cn") {
        Some(RelayProvider::Zhipu)
    } else if url.contains("api.z.ai") {
        Some(RelayProvider::ZhipuEn)
    } else if url.contains("api.minimaxi.com") {
        Some(RelayProvider::MiniMaxCn)
    } else if url.contains("api.minimax.io") {
        Some(RelayProvider::MiniMaxGlobal)
    } else if url.contains("zenmux.") {
        Some(RelayProvider::ZenMux)
    } else if url.contains("api.deepseek.com") {
        Some(RelayProvider::DeepSeek)
    } else if url.contains("api.stepfun.ai") || url.contains("api.stepfun.com") {
        Some(RelayProvider::StepFun)
    } else if url.contains("api.siliconflow.cn") {
        Some(RelayProvider::SiliconFlowCn)
    } else if url.contains("api.siliconflow.com") {
        Some(RelayProvider::SiliconFlowEn)
    } else if url.contains("openrouter.ai") {
        Some(RelayProvider::OpenRouter)
    } else if url.contains("api.novita.ai") {
        Some(RelayProvider::Novita)
    } else {
        None
    }
}

// ===== 通用小工具 =====

fn parse_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

fn field_f64(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(parse_f64)
}

/// 把重置时间字段归一化为 ISO8601：数字按毫秒 epoch 转换，字符串原样保留。
fn reset_to_iso(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        let trimmed = s.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    let ms = value.as_i64()?;
    chrono::DateTime::from_timestamp_millis(ms).map(|dt| dt.to_rfc3339())
}

fn window_tier(name: &str, utilization: f64, resets_at: Option<String>) -> QuotaTier {
    QuotaTier {
        name: name.to_string(),
        kind: QuotaKind::Window,
        utilization: utilization.clamp(0.0, 100.0),
        resets_at,
        ..Default::default()
    }
}

fn balance_tier(currency: &str, remaining: f64, max: Option<f64>, ok: Option<bool>) -> QuotaTier {
    QuotaTier {
        name: currency.to_string(),
        kind: QuotaKind::Balance,
        utilization: 0.0,
        resets_at: None,
        remaining_value: Some(remaining),
        max_value: max,
        currency: Some(currency.to_string()),
        limit_reached: ok.map(|v| !v),
    }
}

// ===== Coding-plan（窗口型）解析器 =====

/// Kimi For Coding：`limits[].detail` → 5h，顶层 `usage` → 周。
pub fn parse_kimi(body: &Value) -> Vec<QuotaTier> {
    let mut tiers = Vec::new();
    if let Some(limits) = body.get("limits").and_then(|v| v.as_array()) {
        for item in limits {
            if let Some(detail) = item.get("detail") {
                let limit = field_f64(detail, "limit").unwrap_or(1.0);
                let remaining = field_f64(detail, "remaining").unwrap_or(0.0);
                let util = if limit > 0.0 {
                    (limit - remaining) / limit * 100.0
                } else {
                    0.0
                };
                tiers.push(window_tier(
                    "five_hour",
                    util,
                    detail.get("resetTime").and_then(reset_to_iso),
                ));
            }
        }
    }
    if let Some(usage) = body.get("usage") {
        let limit = field_f64(usage, "limit").unwrap_or(1.0);
        let remaining = field_f64(usage, "remaining").unwrap_or(0.0);
        let util = if limit > 0.0 {
            (limit - remaining) / limit * 100.0
        } else {
            0.0
        };
        tiers.push(window_tier(
            "seven_day",
            util,
            usage.get("resetTime").and_then(reset_to_iso),
        ));
    }
    tiers
}

/// 智谱 GLM / Z.ai：`data.limits[]` 中 `type == TOKENS_LIMIT`，按序映射 5h/周。
/// 入参为响应体的 `data` 对象。
pub fn parse_zhipu(data: &Value) -> Vec<QuotaTier> {
    let mut found: Vec<(f64, Option<String>)> = Vec::new();
    if let Some(limits) = data.get("limits").and_then(|v| v.as_array()) {
        for item in limits {
            let ltype = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if !ltype.eq_ignore_ascii_case("TOKENS_LIMIT") {
                continue;
            }
            let pct = field_f64(item, "percentage").unwrap_or(0.0);
            let reset = item.get("nextResetTime").and_then(reset_to_iso);
            found.push((pct, reset));
        }
    }
    found
        .into_iter()
        .enumerate()
        .filter_map(|(idx, (pct, reset))| {
            let name = match idx {
                0 => "five_hour",
                1 => "seven_day",
                _ => return None,
            };
            Some(window_tier(name, pct, reset))
        })
        .collect()
}

/// MiniMax：`model_remains` 取 `model_name == general`，
/// `current_interval_remaining_percent` 为剩余% → util = 100 − remaining。
/// 注：周窗口字段（`current_weekly_status == 1` 时）字段名待核实，暂只产出 5h。
pub fn parse_minimax(body: &Value) -> Vec<QuotaTier> {
    let mut tiers = Vec::new();
    if let Some(models) = body.get("model_remains").and_then(|v| v.as_array()) {
        if let Some(general) = models
            .iter()
            .find(|m| m.get("model_name").and_then(|v| v.as_str()) == Some("general"))
        {
            if let Some(remaining_pct) = field_f64(general, "current_interval_remaining_percent") {
                tiers.push(window_tier("five_hour", 100.0 - remaining_pct, None));
            }
        }
    }
    tiers
}

/// ZenMux：`data.quota_5_hour` / `quota_7_day`，含 `used_value_usd` / `max_value_usd`。
/// 入参为响应体的 `data` 对象。
pub fn parse_zenmux(data: &Value) -> Vec<QuotaTier> {
    let mut tiers = Vec::new();
    for (key, name) in [("quota_5_hour", "five_hour"), ("quota_7_day", "seven_day")] {
        if let Some(q) = data.get(key) {
            let pct = field_f64(q, "usage_percentage").unwrap_or(0.0);
            let reset = q
                .get("resets_at")
                .and_then(|v| v.as_str())
                .map(String::from);
            let used = field_f64(q, "used_value_usd");
            let max = field_f64(q, "max_value_usd");
            let mut tier = window_tier(name, pct, reset);
            tier.max_value = max;
            tier.remaining_value = match (used, max) {
                (Some(u), Some(m)) => Some(m - u),
                _ => None,
            };
            tier.currency = Some("USD".to_string());
            tiers.push(tier);
        }
    }
    tiers
}

// ===== Balance（余额型）解析器 =====

/// DeepSeek：`balance_infos[].{currency,total_balance}` + `is_available`。
pub fn parse_deepseek(body: &Value) -> Vec<QuotaTier> {
    let is_available = body
        .get("is_available")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let mut tiers = Vec::new();
    if let Some(infos) = body.get("balance_infos").and_then(|v| v.as_array()) {
        for info in infos {
            let currency = info
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("CNY");
            let total = field_f64(info, "total_balance").unwrap_or(0.0);
            tiers.push(balance_tier(currency, total, None, Some(is_available)));
        }
    }
    tiers
}

/// StepFun：`/v1/accounts` 的 `balance`（CNY）。
pub fn parse_stepfun(body: &Value) -> Vec<QuotaTier> {
    let balance = field_f64(body, "balance")
        .or_else(|| body.pointer("/data/balance").and_then(parse_f64))
        .unwrap_or(0.0);
    vec![balance_tier("CNY", balance, None, None)]
}

/// SiliconFlow：`data.totalBalance`（国内 CNY / 海外 USD）。
pub fn parse_siliconflow(body: &Value, is_cn: bool) -> Vec<QuotaTier> {
    let data = body.get("data").unwrap_or(body);
    let total = field_f64(data, "totalBalance").unwrap_or(0.0);
    let unit = if is_cn { "CNY" } else { "USD" };
    vec![balance_tier(unit, total, None, None)]
}

/// OpenRouter：`data.{total_credits,total_usage}` → 剩余 = credits − usage（USD）。
pub fn parse_openrouter(body: &Value) -> Vec<QuotaTier> {
    let data = body.get("data").unwrap_or(body);
    let credits = field_f64(data, "total_credits").unwrap_or(0.0);
    let usage = field_f64(data, "total_usage").unwrap_or(0.0);
    vec![balance_tier("USD", credits - usage, Some(credits), None)]
}

/// Novita：`availableBalance`，单位 0.0001 USD → 除以 10000。
pub fn parse_novita(body: &Value) -> Vec<QuotaTier> {
    let available = field_f64(body, "availableBalance").unwrap_or(0.0) / 10000.0;
    vec![balance_tier("USD", available, None, None)]
}

// ===== 取数 + 分发 =====

fn make_quota(provider: &RelayProvider, tiers: Vec<QuotaTier>) -> SubscriptionQuota {
    SubscriptionQuota {
        provider: "relay".to_string(),
        tool: provider.id().to_string(),
        source_tool: None,
        credential_status: "valid".to_string(),
        credential_message: None,
        success: true,
        tiers,
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: None,
        plan_label: None,
        account_label: None,
    }
}

fn make_error(provider: &str, msg: String) -> SubscriptionQuota {
    SubscriptionQuota {
        provider: "relay".to_string(),
        tool: provider.to_string(),
        source_tool: None,
        credential_status: "queryFailed".to_string(),
        credential_message: Some(msg.clone()),
        success: false,
        tiers: Vec::new(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        from_cache: false,
        error: Some(msg),
        plan_label: None,
        account_label: None,
    }
}

/// 各供应商的查询端点。
fn endpoint(provider: RelayProvider, base_url: &str) -> String {
    match provider {
        RelayProvider::Kimi => "https://api.kimi.com/coding/v1/usages".into(),
        RelayProvider::Zhipu => "https://open.bigmodel.cn/api/monitor/usage/quota/limit".into(),
        RelayProvider::ZhipuEn => "https://api.z.ai/api/monitor/usage/quota/limit".into(),
        RelayProvider::MiniMaxCn => {
            "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains".into()
        }
        RelayProvider::MiniMaxGlobal => {
            "https://api.minimax.io/v1/api/openplatform/coding_plan/remains".into()
        }
        // ZenMux 直接对 base_url 本身发起请求。
        RelayProvider::ZenMux => base_url.to_string(),
        RelayProvider::DeepSeek => "https://api.deepseek.com/user/balance".into(),
        RelayProvider::StepFun => "https://api.stepfun.com/v1/accounts".into(),
        RelayProvider::SiliconFlowCn => "https://api.siliconflow.cn/v1/user/info".into(),
        RelayProvider::SiliconFlowEn => "https://api.siliconflow.com/v1/user/info".into(),
        RelayProvider::OpenRouter => "https://openrouter.ai/api/v1/credits".into(),
        RelayProvider::Novita => "https://api.novita.ai/v3/user/balance".into(),
    }
}

fn parse_for(provider: RelayProvider, body: &Value) -> Vec<QuotaTier> {
    match provider {
        RelayProvider::Kimi => parse_kimi(body),
        RelayProvider::Zhipu | RelayProvider::ZhipuEn => {
            parse_zhipu(body.get("data").unwrap_or(body))
        }
        RelayProvider::MiniMaxCn | RelayProvider::MiniMaxGlobal => parse_minimax(body),
        RelayProvider::ZenMux => parse_zenmux(body.get("data").unwrap_or(body)),
        RelayProvider::DeepSeek => parse_deepseek(body),
        RelayProvider::StepFun => parse_stepfun(body),
        RelayProvider::SiliconFlowCn => parse_siliconflow(body, true),
        RelayProvider::SiliconFlowEn => parse_siliconflow(body, false),
        RelayProvider::OpenRouter => parse_openrouter(body),
        RelayProvider::Novita => parse_novita(body),
    }
}

/// 给定 base_url + 完整 api_key，查询并归一化为统一额度。
pub async fn fetch_relay_quota_for_provider(
    provider: RelayProvider,
    base_url: &str,
    api_key: &str,
) -> SubscriptionQuota {
    let url = endpoint(provider, base_url);
    let client = HttpClientFactory::global().standard();
    let mut req = client.get(&url).header("Accept", "application/json");
    // 智谱不加 Bearer 前缀，其余统一 Bearer。
    req = match provider {
        RelayProvider::Zhipu | RelayProvider::ZhipuEn => {
            req.header("Authorization", api_key.to_string())
        }
        _ => req.header("Authorization", format!("Bearer {api_key}")),
    };

    let response = match req.send().await {
        Ok(r) => r,
        Err(e) => return make_error(provider.id(), format!("Network error: {e}")),
    };
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return make_error(provider.id(), format!("HTTP {status}: {text}"));
    }
    let body: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return make_error(provider.id(), format!("Parse error: {e}")),
    };

    make_quota(&provider, parse_for(provider, &body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detect_by_base_url() {
        assert_eq!(
            detect_relay_provider("https://api.kimi.com/coding/v1"),
            Some(RelayProvider::Kimi)
        );
        assert_eq!(
            detect_relay_provider("https://open.bigmodel.cn/api/paas/v4"),
            Some(RelayProvider::Zhipu)
        );
        assert_eq!(
            detect_relay_provider("https://api.z.ai/api/anthropic"),
            Some(RelayProvider::ZhipuEn)
        );
        assert_eq!(
            detect_relay_provider("https://openrouter.ai/api/v1"),
            Some(RelayProvider::OpenRouter)
        );
        assert_eq!(detect_relay_provider("https://example.com/v1"), None);
    }

    #[test]
    fn kimi_window_tiers() {
        let body = json!({
            "limits": [{ "detail": { "limit": 1000, "remaining": 250, "resetTime": "2026-06-08T10:00:00Z" } }],
            "usage": { "limit": 50000, "remaining": 40000, "resetTime": 1717800000000_i64 }
        });
        let tiers = parse_kimi(&body);
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0].name, "five_hour");
        assert_eq!(tiers[0].kind, QuotaKind::Window);
        assert!((tiers[0].utilization - 75.0).abs() < 1e-9); // (1000-250)/1000
        assert_eq!(tiers[0].resets_at.as_deref(), Some("2026-06-08T10:00:00Z"));
        assert_eq!(tiers[1].name, "seven_day");
        assert!((tiers[1].utilization - 20.0).abs() < 1e-9); // (50000-40000)/50000
        assert!(tiers[1].resets_at.is_some()); // 毫秒被转成 ISO
    }

    #[test]
    fn zhipu_only_tokens_limit_in_order() {
        let data = json!({
            "limits": [
                { "type": "REQUESTS_LIMIT", "percentage": 10 },
                { "type": "TOKENS_LIMIT", "percentage": 42, "nextResetTime": 1717800000000_i64 },
                { "type": "tokens_limit", "percentage": 17 }
            ]
        });
        let tiers = parse_zhipu(&data);
        assert_eq!(tiers.len(), 2); // 仅两条 TOKENS_LIMIT（大小写不敏感）
        assert_eq!(tiers[0].name, "five_hour");
        assert_eq!(tiers[0].utilization, 42.0);
        assert_eq!(tiers[1].name, "seven_day");
        assert_eq!(tiers[1].utilization, 17.0);
    }

    #[test]
    fn minimax_five_hour_from_remaining() {
        let body = json!({
            "model_remains": [
                { "model_name": "abab", "current_interval_remaining_percent": 90 },
                { "model_name": "general", "current_interval_remaining_percent": 30 }
            ]
        });
        let tiers = parse_minimax(&body);
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0].utilization, 70.0); // 100 - 30
    }

    #[test]
    fn zenmux_window_with_balance() {
        let data = json!({
            "quota_5_hour": { "usage_percentage": 55, "resets_at": "2026-06-08T12:00:00Z", "used_value_usd": 5.5, "max_value_usd": 10.0 },
            "quota_7_day": { "usage_percentage": 33 }
        });
        let tiers = parse_zenmux(&data);
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0].utilization, 55.0);
        assert_eq!(tiers[0].max_value, Some(10.0));
        assert_eq!(tiers[0].remaining_value, Some(4.5)); // 10 - 5.5
        assert_eq!(tiers[0].currency.as_deref(), Some("USD"));
    }

    #[test]
    fn deepseek_balance() {
        let body = json!({
            "is_available": true,
            "balance_infos": [{ "currency": "CNY", "total_balance": "88.80" }]
        });
        let tiers = parse_deepseek(&body);
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0].kind, QuotaKind::Balance);
        assert_eq!(tiers[0].name, "CNY");
        assert_eq!(tiers[0].remaining_value, Some(88.80));
        assert_eq!(tiers[0].limit_reached, Some(false));
    }

    #[test]
    fn openrouter_remaining_is_credits_minus_usage() {
        let body = json!({ "data": { "total_credits": 20.0, "total_usage": 7.5 } });
        let tiers = parse_openrouter(&body);
        assert_eq!(tiers[0].remaining_value, Some(12.5));
        assert_eq!(tiers[0].max_value, Some(20.0));
        assert_eq!(tiers[0].currency.as_deref(), Some("USD"));
    }

    #[test]
    fn novita_divides_by_10000() {
        let body = json!({ "availableBalance": 123456 });
        let tiers = parse_novita(&body);
        assert_eq!(tiers[0].remaining_value, Some(12.3456));
    }

    #[test]
    fn siliconflow_currency_by_region() {
        let body = json!({ "data": { "totalBalance": "5.00" } });
        assert_eq!(
            parse_siliconflow(&body, true)[0].currency.as_deref(),
            Some("CNY")
        );
        assert_eq!(
            parse_siliconflow(&body, false)[0].currency.as_deref(),
            Some("USD")
        );
    }

    #[test]
    fn stepfun_reads_balance_nested_or_top() {
        assert_eq!(
            parse_stepfun(&json!({ "balance": 12.0 }))[0].remaining_value,
            Some(12.0)
        );
        assert_eq!(
            parse_stepfun(&json!({ "data": { "balance": 9.0 } }))[0].remaining_value,
            Some(9.0)
        );
    }
}
