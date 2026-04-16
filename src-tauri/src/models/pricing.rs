//! Model pricing for cost estimation
//!
//! Pricing data based on Anthropic official pricing as of 2025.
//! Supports Claude 4, Claude 3.5, and Claude 3 models.

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
        // Default to Sonnet pricing
        Self {
            input: 3.0,
            output: 15.0,
            cache_write_5m: 3.75,
            cache_write_1h: 1.50,
            cache_read: 0.30,
        }
    }
}

/// Get pricing for a model by name
pub fn get_pricing(model: &str) -> ModelPricing {
    let model_lower = model.to_lowercase();

    // Claude 4 models
    if model_lower.contains("claude-4") || model_lower.contains("claude4") || model_lower.contains("claude-4-5") {
        if model_lower.contains("opus") {
            return ModelPricing {
                input: 15.0,
                output: 75.0,
                cache_write_5m: 18.75,
                cache_write_1h: 7.50,
                cache_read: 1.50,
            };
        }
        if model_lower.contains("sonnet") {
            return ModelPricing {
                input: 3.0,
                output: 15.0,
                cache_write_5m: 3.75,
                cache_write_1h: 1.50,
                cache_read: 0.30,
            };
        }
        if model_lower.contains("haiku") {
            return ModelPricing {
                input: 0.80,
                output: 4.0,
                cache_write_5m: 1.0,
                cache_write_1h: 0.40,
                cache_read: 0.08,
            };
        }
    }

    // Claude 3.5 models
    if model_lower.contains("claude-3-5") || model_lower.contains("claude3.5") || model_lower.contains("claude-35") {
        if model_lower.contains("sonnet") {
            return ModelPricing {
                input: 3.0,
                output: 15.0,
                cache_write_5m: 3.75,
                cache_write_1h: 1.50,
                cache_read: 0.30,
            };
        }
        if model_lower.contains("haiku") {
            return ModelPricing {
                input: 0.80,
                output: 4.0,
                cache_write_5m: 1.0,
                cache_write_1h: 0.40,
                cache_read: 0.08,
            };
        }
    }

    // Claude 3 models
    if model_lower.contains("claude-3") || model_lower.contains("claude3") {
        if model_lower.contains("opus") {
            return ModelPricing {
                input: 15.0,
                output: 75.0,
                cache_write_5m: 18.75,
                cache_write_1h: 7.50,
                cache_read: 1.50,
            };
        }
        if model_lower.contains("sonnet") {
            return ModelPricing {
                input: 3.0,
                output: 15.0,
                cache_write_5m: 3.75,
                cache_write_1h: 1.50,
                cache_read: 0.30,
            };
        }
        if model_lower.contains("haiku") {
            return ModelPricing {
                input: 0.25,
                output: 1.25,
                cache_write_5m: 0.30,
                cache_write_1h: 0.12,
                cache_read: 0.03,
            };
        }
    }

    // Default pricing (Sonnet-like)
    ModelPricing::default()
}

/// Estimate cost for a session based on token usage
pub fn estimate_session_cost(
    input_tokens: u64,
    output_tokens: u64,
    cache_create_tokens: u64,
    cache_read_tokens: u64,
    model: &str,
) -> f64 {
    let pricing = get_pricing(model);

    let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output;
    let cache_read_cost = (cache_read_tokens as f64 / 1_000_000.0) * pricing.cache_read;
    // Use 1h cache write price as default (more conservative)
    let cache_create_cost = (cache_create_tokens as f64 / 1_000_000.0) * pricing.cache_write_1h;

    input_cost + output_cost + cache_read_cost + cache_create_cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sonnet_pricing() {
        let pricing = get_pricing("claude-3-5-sonnet-20241022");
        assert_eq!(pricing.input, 3.0);
        assert_eq!(pricing.output, 15.0);
    }

    #[test]
    fn test_opus_pricing() {
        let pricing = get_pricing("claude-4-opus-20250514");
        assert_eq!(pricing.input, 15.0);
        assert_eq!(pricing.output, 75.0);
    }

    #[test]
    fn test_cost_estimation() {
        let cost = estimate_session_cost(1_000_000, 500_000, 100_000, 200_000, "claude-3-5-sonnet");
        // input: 1M * 3 = $3
        // output: 0.5M * 15 = $7.5
        // cache_create: 0.1M * 1.5 = $0.15
        // cache_read: 0.2M * 0.3 = $0.06
        // total: $10.71
        assert!((cost - 10.71).abs() < 0.01);
    }
}
