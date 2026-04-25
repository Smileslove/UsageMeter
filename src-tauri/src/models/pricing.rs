//! Model pricing for cost estimation
//!
//! Pricing data comes from:
//! 1. User custom pricing (highest priority)
//! 2. API synchronized pricing from models.dev
//!
//! No built-in pricing data - users should sync from API or add custom pricing.

use super::ModelPricingConfig;
use serde::{Deserialize, Serialize};

/// Model pricing configuration ($/M tokens)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Input token price ($/M)
    pub input: f64,
    /// Output token price ($/M)
    pub output: f64,
    /// Cache write 5min price ($/M)
    pub cache_write_5m: f64,
    /// Cache write 1h price ($/M)
    pub cache_write_1h: f64,
    /// Cache read price ($/M)
    pub cache_read: f64,
}

impl Default for ModelPricing {
    fn default() -> Self {
        // No pricing available - will result in $0 cost estimation
        Self {
            input: 0.0,
            output: 0.0,
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
            cache_read: 0.0,
        }
    }
}

/// Get pricing for a model from database/custom pricing configuration
///
/// Priority: custom > api > default (0.0)
pub fn get_pricing(model: &str, pricings: &[ModelPricingConfig], match_mode: &str) -> ModelPricing {
    let normalized_model = normalize_model_id(model);

    // Try to find a matching pricing
    let matched = if match_mode == "exact" {
        // Exact match means byte-for-byte model ID equality.
        pricings
            .iter()
            .filter(|p| p.model_id == model)
            .min_by_key(|p| source_priority(&p.source))
    } else {
        // Fuzzy match tolerates provider prefixes, case differences, and separator variants.
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
            // Use cache_write_price if available, otherwise estimate
            cache_write_5m: pricing
                .cache_write_price
                .unwrap_or(pricing.input_price * 1.25),
            cache_write_1h: pricing
                .cache_write_price
                .unwrap_or(pricing.input_price * 0.5),
            // Use cache_read_price if available, otherwise estimate
            cache_read: pricing
                .cache_read_price
                .unwrap_or(pricing.input_price * 0.1),
        };
    }

    // No pricing found - return default (0.0)
    ModelPricing::default()
}

fn normalize_model_id(model: &str) -> String {
    model
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn fuzzy_match_score(
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
    // Use 1h cache write price as default (more conservative)
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

        // Fuzzy match should find minimax-m2-5 when searching for partial
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

        // Exact match should only match exact string
        let pricing = get_pricing("exact-model", &pricings, "exact");
        assert!((pricing.input - 5.0).abs() < 0.01);

        // Should return default (0.0) for non-exact match
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
        // No pricing = $0 cost
        assert!((cost - 0.0).abs() < 0.01);
    }
}
