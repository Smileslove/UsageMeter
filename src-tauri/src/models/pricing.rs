//! 模型价格配置，用于费用估算
//!
//! 价格数据来源：
//! 1. 用户自定义价格（最高优先级）
//! 2. API 同步的价格（来自 models.dev）
//!
//! 无内置价格数据 - 用户应从 API 同步或添加自定义价格。

use super::ModelPricingConfig;
use serde::{Deserialize, Serialize};

/// 模型价格配置（$/M tokens）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// 输入 Token 价格（$/M）
    pub input: f64,
    /// 输出 Token 价格（$/M）
    pub output: f64,
    /// 缓存写入 5 分钟价格（$/M）
    pub cache_write_5m: f64,
    /// 缓存写入 1 小时价格（$/M）
    pub cache_write_1h: f64,
    /// 缓存读取价格（$/M）
    pub cache_read: f64,
}

impl Default for ModelPricing {
    fn default() -> Self {
        // 无可用价格 - 将导致 $0 费用估算
        Self {
            input: 0.0,
            output: 0.0,
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: 0.0,
        }
    }
}

/// 从数据库/自定义价格配置获取模型价格
///
/// 优先级：custom > api > default (0.0)
pub fn get_pricing(model: &str, pricings: &[ModelPricingConfig], match_mode: &str) -> ModelPricing {
    let normalized_model = normalize_model_id(model);

    // 尝试查找匹配的价格配置
    let matched = if match_mode == "exact" {
        // 精确匹配表示模型 ID 字节级完全相等
        pricings
            .iter()
            .filter(|p| p.model_id == model)
            .min_by_key(|p| source_priority(&p.source))
    } else {
        // 模糊匹配允许供应商前缀、大小写差异和分隔符变体
        pricings
            .iter()
            .filter_map(|p| fuzzy_match_score(model, &normalized_model, p).map(|score| (score, p)))
            .min_by_key(|(score, pricing)| (*score, source_priority(&pricing.source)))
            .map(|(_, pricing)| pricing)
    };

    if let Some(pricing) = matched {
        return ModelPricing {
            input: pricing.input_price,
            output: pricing.output_price,
            // 如果有缓存写入价格则使用，否则为 0（未配置不计费）
            cache_write_5m: pricing.cache_write_price.unwrap_or(0.0),
            cache_write_1h: pricing.cache_write_price.unwrap_or(0.0),
            // 如果有缓存读取价格则使用，否则估算
            cache_read: pricing
                .cache_read_price
                .unwrap_or(pricing.input_price * 0.1),
        };
    }

    // 未找到价格 - 返回默认值 (0.0)
    ModelPricing::default()
}

/// 标准化模型 ID（移除所有非字母数字字符并转小写）
///
/// 用于模糊匹配时的模型 ID 比较，忽略分隔符和大小写差异。
/// 例如：`MiniMax-M2.5` -> `minimaxm25`
///
/// 此函数公开以支持 `proxy::database` 中的批量价格应用逻辑。
pub fn normalize_model_id(model: &str) -> String {
    model
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

/// 计算模型与价格配置的模糊匹配分数
///
/// 返回 `Some(score)` 表示匹配成功，分数越小匹配度越高：
/// - `0`: 完全相等
/// - `1`: 忽略大小写相等
/// - `2`: 互相包含（大小写）
/// - `3`: 标准化后互相包含
///
/// 返回 `None` 表示不匹配。
///
/// 此函数公开以支持 `proxy::database` 中的批量价格应用逻辑。
pub fn fuzzy_match_score(
    model: &str,
    normalized_model: &str,
    pricing: &ModelPricingConfig,
) -> Option<u8> {
    let pricing_id = pricing.model_id.as_str();
    if model == pricing_id {
        return Some(0);
    }
    if model.eq_ignore_ascii_case(pricing_id) {
        return Some(1);
    }

    let model_lower = model.to_ascii_lowercase();
    let pricing_lower = pricing_id.to_ascii_lowercase();
    if model_lower.contains(&pricing_lower) || pricing_lower.contains(&model_lower) {
        return Some(2);
    }

    let normalized_pricing = normalize_model_id(pricing_id);
    if normalized_model.contains(&normalized_pricing)
        || normalized_pricing.contains(normalized_model)
    {
        return Some(3);
    }

    None
}

fn source_priority(source: &str) -> u8 {
    if source == "custom" {
        0
    } else {
        1
    }
}

/// 估算会话费用（基于 token 使用量）
pub fn estimate_session_cost(
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    model: &str,
    pricings: &[ModelPricingConfig],
    match_mode: &str,
) -> f64 {
    let pricing = get_pricing(model, pricings, match_mode);

    let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output;
    let cache_read_cost = (cache_read_tokens as f64 / 1_000_000.0) * pricing.cache_read;
    // 使用 1 小时缓存写入价格作为默认值（更保守）
    let cache_create_cost = (cache_create_tokens as f64 / 1_000_000.0) * pricing.cache_write_1h;

    input_cost + output_cost + cache_read_cost + cache_create_cost
}

/// 模型用量分布条目（用于费用计算）
#[allow(dead_code)]
pub struct ModelUsageCost {
    pub model_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_create_tokens: u64,
    pub cache_read_tokens: u64,
}

/// 计算模型分布的总费用
///
/// 遍历每个模型的 token 使用量，根据价格配置计算总费用
#[allow(dead_code)]
pub fn estimate_total_cost(
    model_usages: &[ModelUsageCost],
    pricings: &[ModelPricingConfig],
    match_mode: &str,
) -> f64 {
    model_usages
        .iter()
        .map(|usage| {
            estimate_session_cost(
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_create_tokens,
                usage.cache_read_tokens,
                &usage.model_name,
                pricings,
                match_mode,
            )
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_pricing_returns_zero() {
        let pricings: Vec<ModelPricingConfig> = vec![];
        let pricing = get_pricing("claude-3-5-sonnet-20241022", &pricings, "fuzzy");
        assert_eq!(pricing.input, 0.0);
        assert_eq!(pricing.output, 0.0);
    }

    #[test]
    fn test_custom_pricing_fuzzy_match() {
        let pricings = vec![ModelPricingConfig {
            model_id: "minimax-m2-5".to_string(),
            display_name: None,
            input_price: 0.33,
            output_price: 1.32,
            cache_write_price: None,
            cache_read_price: None,
            source: "api".to_string(),
            last_updated: 0,
        }];

        // 模糊匹配应能在搜索部分字符串时找到 minimax-m2-5
        let pricing = get_pricing("minimax-m2-5", &pricings, "fuzzy");
        assert!((pricing.input - 0.33).abs() < 0.01);
        assert!((pricing.output - 1.32).abs() < 0.01);
    }

    #[test]
    fn test_custom_pricing_exact_match() {
        let pricings = vec![ModelPricingConfig {
            model_id: "exact-model".to_string(),
            display_name: None,
            input_price: 5.0,
            output_price: 10.0,
            cache_write_price: None,
            cache_read_price: None,
            source: "custom".to_string(),
            last_updated: 0,
        }];

        // 精确匹配应只匹配完全相同的字符串
        let pricing = get_pricing("exact-model", &pricings, "exact");
        assert!((pricing.input - 5.0).abs() < 0.01);

        // 对于非精确匹配应返回默认值 (0.0)
        let pricing = get_pricing("exact-model-v2", &pricings, "exact");
        assert!((pricing.input - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_exact_match_requires_identical_model_id() {
        let pricings = vec![ModelPricingConfig {
            model_id: "glm-5".to_string(),
            display_name: None,
            input_price: 1.0,
            output_price: 3.2,
            cache_write_price: None,
            cache_read_price: None,
            source: "api".to_string(),
            last_updated: 0,
        }];

        let pricing = get_pricing("GLM-5", &pricings, "exact");
        assert!((pricing.input - 0.0).abs() < 0.01);

        let pricing = get_pricing("glm-5", &pricings, "exact");
        assert!((pricing.input - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_fuzzy_match_tolerates_case_and_separators() {
        let pricings = vec![ModelPricingConfig {
            model_id: "MiniMax-M2.5".to_string(),
            display_name: None,
            input_price: 0.3,
            output_price: 1.2,
            cache_write_price: None,
            cache_read_price: None,
            source: "api".to_string(),
            last_updated: 0,
        }];

        let pricing = get_pricing("minimax-m2-5", &pricings, "fuzzy");
        assert!((pricing.input - 0.3).abs() < 0.01);
        assert!((pricing.output - 1.2).abs() < 0.01);
    }

    #[test]
    fn test_fuzzy_match_prioritizes_custom_pricing() {
        let pricings = vec![
            ModelPricingConfig {
                model_id: "GLM-5".to_string(),
                display_name: None,
                input_price: 1.0,
                output_price: 3.2,
                cache_write_price: None,
                cache_read_price: None,
                source: "api".to_string(),
                last_updated: 0,
            },
            ModelPricingConfig {
                model_id: "GLM-5".to_string(),
                display_name: None,
                input_price: 0.59,
                output_price: 2.64,
                cache_write_price: None,
                cache_read_price: None,
                source: "custom".to_string(),
                last_updated: 0,
            },
        ];

        let pricing = get_pricing("GLM-5", &pricings, "fuzzy");
        assert!((pricing.input - 0.59).abs() < 0.01);
        assert!((pricing.output - 2.64).abs() < 0.01);
    }

    #[test]
    fn test_cost_estimation_with_pricing() {
        let pricings = vec![ModelPricingConfig {
            model_id: "claude-3-5-sonnet".to_string(),
            display_name: None,
            input_price: 3.0,
            output_price: 15.0,
            cache_write_price: Some(1.50),
            cache_read_price: Some(0.30),
            source: "api".to_string(),
            last_updated: 0,
        }];

        let cost = estimate_session_cost(
            1_000_000,
            500_000,
            100_000,
            200_000,
            "claude-3-5-sonnet",
            &pricings,
            "fuzzy",
        );
        // input: 1M * 3 = $3
        // output: 0.5M * 15 = $7.5
        // cache_create: 0.1M * 1.5 = $0.15
        // cache_read: 0.2M * 0.3 = $0.06
        // total: $10.71
        assert!((cost - 10.71).abs() < 0.01);
    }

    #[test]
    fn test_cost_estimation_without_pricing() {
        let pricings: Vec<ModelPricingConfig> = vec![];
        let cost = estimate_session_cost(
            1_000_000,
            500_000,
            100_000,
            200_000,
            "unknown-model",
            &pricings,
            "fuzzy",
        );
        // 无价格配置 = $0 费用
        assert!((cost - 0.0).abs() < 0.01);
    }
}
