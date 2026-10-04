//! 模型价格相关命令

use crate::models::ModelPricingConfig;
use crate::net::HttpClientFactory;
use crate::proxy::ProxyDatabase;
use std::collections::HashMap;
use std::sync::Arc;

/// 模型价格数据库实例（独立于代理）
static MODEL_PRICING_DB: std::sync::OnceLock<Arc<std::sync::Mutex<Option<ProxyDatabase>>>> =
    std::sync::OnceLock::new();

/// 获取模型价格数据库实例
fn get_pricing_db() -> Result<Arc<std::sync::Mutex<Option<ProxyDatabase>>>, String> {
    let db = MODEL_PRICING_DB.get_or_init(|| match ProxyDatabase::new_pricing_store() {
        Ok(database) => Arc::new(std::sync::Mutex::new(Some(database))),
        Err(e) => {
            eprintln!("[ModelPricing] Failed to create database: {}", e);
            Arc::new(std::sync::Mutex::new(None))
        }
    });
    Ok(db.clone())
}

/// 模型价格搜索结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingSearchResult {
    pub pricings: Vec<ModelPricingConfig>,
    pub total: i64,
}

fn parse_model_date(value: Option<&str>, fallback: i64) -> i64 {
    let Some(value) = value else { return fallback };
    let normalized = match value.len() {
        7 => format!("{value}-01"),
        4 => format!("{value}-01-01"),
        _ => value.to_string(),
    };
    chrono::NaiveDate::parse_from_str(&normalized, "%Y-%m-%d")
        .ok()
        .map(|date| date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
        .unwrap_or_else(|| {
            eprintln!(
                "[ModelPricing] Failed to parse last_updated '{}', using current time",
                value
            );
            fallback
        })
}

fn select_unambiguous_pricings(
    candidates: HashMap<String, Vec<ModelPricingConfig>>,
) -> Vec<ModelPricingConfig> {
    let mut selected = Vec::new();
    for (model_id, mut variants) in candidates {
        variants.sort_by(|a, b| {
            a.last_updated
                .cmp(&b.last_updated)
                .then_with(|| a.display_name.cmp(&b.display_name))
        });
        let Some(first) = variants.first() else {
            continue;
        };
        let same_price = variants.iter().all(|item| {
            item.input_price.to_bits() == first.input_price.to_bits()
                && item.output_price.to_bits() == first.output_price.to_bits()
                && item.cache_read_price.map(f64::to_bits)
                    == first.cache_read_price.map(f64::to_bits)
                && item.cache_write_price.map(f64::to_bits)
                    == first.cache_write_price.map(f64::to_bits)
        });
        if same_price {
            selected.push(variants.pop().unwrap());
        } else {
            eprintln!(
                "[ModelPricing] Skipping ambiguous model '{}' with provider-specific prices",
                model_id
            );
        }
    }
    selected
}

/// 从 models.dev API 同步模型价格到数据库
#[tauri::command]
pub async fn sync_model_pricing_from_api() -> Result<usize, String> {
    // 1. 从 API 获取价格数据
    let client = HttpClientFactory::global().standard();

    let response = client
        .get("https://models.dev/api.json")
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("API returned status: {}", response.status()));
    }

    // 2. 解析响应
    #[derive(Debug, serde::Deserialize)]
    struct ModelsDevResponse {
        #[serde(flatten)]
        providers: std::collections::HashMap<String, ModelsDevProvider>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct ModelsDevProvider {
        #[serde(default)]
        models: std::collections::HashMap<String, ModelsDevModel>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct ModelsDevModel {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        cost: Option<ModelsDevCost>,
        #[serde(default)]
        last_updated: Option<String>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct ModelsDevCost {
        input: Option<f64>,
        output: Option<f64>,
        #[serde(default)]
        cache_read: Option<f64>,
        #[serde(default)]
        cache_write: Option<f64>,
        #[serde(default)]
        tiers: Option<serde_json::Value>,
        #[serde(default)]
        context_over_200k: Option<serde_json::Value>,
        #[serde(default)]
        reasoning: Option<f64>,
    }

    let data: ModelsDevResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse JSON response: {}", e))?;

    let now = chrono::Utc::now().timestamp();
    let mut candidates: HashMap<String, Vec<ModelPricingConfig>> = HashMap::new();

    // 收集所有厂商变体；只有价格完全一致时才合并，避免跨厂商套错价格。
    for (_provider_id, provider) in data.providers {
        for (model_key, model) in provider.models {
            if let Some(cost) = model.cost {
                if cost.tiers.is_some()
                    || cost.context_over_200k.is_some()
                    || cost.reasoning.is_some()
                {
                    eprintln!(
                        "[ModelPricing] Skipping model '{}' with unsupported tiered pricing",
                        model.id.as_str()
                    );
                    continue;
                }
                if let (Some(input), Some(output)) = (cost.input, cost.output) {
                    if !input.is_finite() || !output.is_finite() || input < 0.0 || output < 0.0 {
                        continue;
                    }
                    let model_id = if model.id.is_empty() {
                        model_key
                    } else {
                        model.id
                    };

                    // 解析模型的 last_updated 日期作为时间戳
                    let model_last_updated = parse_model_date(model.last_updated.as_deref(), now);

                    let new_pricing = ModelPricingConfig {
                        model_id: model_id.clone(),
                        display_name: model.name,
                        input_price: input,
                        output_price: output,
                        cache_write_price: cost.cache_write,
                        cache_read_price: cost.cache_read,
                        source: "api".to_string(),
                        last_updated: model_last_updated,
                    };

                    candidates.entry(model_id).or_default().push(new_pricing);
                }
            }
        }
    }

    // 转换为向量并按模型 ID 排序
    let mut pricings = select_unambiguous_pricings(candidates);
    pricings.sort_by(|a, b| a.model_id.cmp(&b.model_id));

    // 3. 存入数据库（使用 tauri async_runtime spawn_blocking 避免阻塞异步运行时）
    let db_arc = get_pricing_db()?;
    let count = tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            // 确保表存在
            database.create_model_pricing_table()?;
            database.replace_api_model_pricings(&pricings)
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))??;

    Ok(count)
}

/// 搜索模型价格（返回 JSON 字符串以绕过 Tauri 序列化问题）
#[tauri::command]
pub async fn search_model_pricing(
    query: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<String, String> {
    let limit = limit.unwrap_or(100);
    let offset = offset.unwrap_or(0);

    let db_arc = get_pricing_db()?;

    // 使用 tauri async_runtime spawn_blocking 避免阻塞异步运行时
    let db_arc_clone = db_arc.clone();
    let query_clone = query.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc_clone
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            // 确保 model_pricing 表存在
            database.create_model_pricing_table()?;

            let pricings = database.search_model_pricings(query_clone.as_deref(), limit, offset)?;
            let total = database.count_synced_model_pricings(query_clone.as_deref())?;

            Ok(ModelPricingSearchResult { pricings, total })
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))??;

    serde_json::to_string(&result).map_err(|e| format!("Serialization error: {}", e))
}

/// 获取自定义模型价格列表（支持搜索）
#[tauri::command]
pub async fn get_custom_model_pricings(query: Option<String>) -> Result<String, String> {
    let db_arc = get_pricing_db()?;

    let query_clone = query.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            database.create_model_pricing_table()?;
            database.get_custom_model_pricings(query_clone.as_deref())
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))??;

    serde_json::to_string(&result).map_err(|e| format!("Serialization error: {}", e))
}

/// 获取同步模型总数
#[tauri::command]
pub async fn count_synced_model_pricings(query: Option<String>) -> Result<i64, String> {
    let db_arc = get_pricing_db()?;

    let query_clone = query.clone();
    let count = tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            database.create_model_pricing_table()?;
            database.count_synced_model_pricings(query_clone.as_deref())
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))??;

    Ok(count)
}

/// 添加自定义模型价格
#[tauri::command]
pub async fn add_custom_model_pricing(pricing: ModelPricingConfig) -> Result<(), String> {
    crate::models::validate_model_pricing(&pricing)?;
    let db_arc = get_pricing_db()?;

    tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            // 确保 model_pricing 表存在
            database.create_model_pricing_table()?;

            let mut pricing = pricing;
            pricing.source = "custom".to_string();
            pricing.last_updated = chrono::Utc::now().timestamp();

            database.add_custom_pricing(&pricing)
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))?
}

/// 更新自定义模型价格
#[tauri::command]
pub async fn update_custom_model_pricing(pricing: ModelPricingConfig) -> Result<(), String> {
    crate::models::validate_model_pricing(&pricing)?;
    let db_arc = get_pricing_db()?;

    tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            let mut pricing = pricing;
            pricing.last_updated = chrono::Utc::now().timestamp();

            database.update_custom_pricing(&pricing)
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))?
}

/// 删除模型价格
#[tauri::command]
pub async fn delete_model_pricing(model_id: String) -> Result<(), String> {
    let db_arc = get_pricing_db()?;

    tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            database.delete_model_pricing(&model_id)
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))?
}

/// 清空所有同步的模型价格（保留自定义模型）
#[tauri::command]
pub async fn clear_synced_model_pricings() -> Result<usize, String> {
    let db_arc = get_pricing_db()?;

    tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            database.clear_synced_model_pricings()
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))?
}

/// 获取所有模型价格配置（用于费用计算）
#[tauri::command]
pub async fn get_all_model_pricings() -> Result<Vec<ModelPricingConfig>, String> {
    let db_arc = get_pricing_db()?;

    tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            database.get_all_model_pricings()
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))?
}

/// 预览价格应用：返回匹配的记录数、当前费用、匹配的模型列表
#[tauri::command]
pub async fn preview_pricing_apply(
    model_id: String,
    match_mode: String,
    time_range_start: Option<i64>,
    time_range_end: Option<i64>,
    client_tool_filter: Option<String>,
    api_source_key_prefixes: Option<Vec<String>>,
) -> Result<crate::proxy::PreviewPricingApplyResult, String> {
    let db = crate::proxy::ProxyDatabase::get_global()
        .ok_or_else(|| "Proxy database not available".to_string())?;

    let filter = crate::proxy::PricingMatchFilter {
        model_id: &model_id,
        match_mode: &match_mode,
        time_range_start,
        time_range_end,
        client_tool_filter: client_tool_filter.as_deref(),
        api_source_key_prefixes: api_source_key_prefixes.as_deref(),
    };

    db.preview_pricing_apply(&filter).await
}

/// 将指定价格应用到匹配的历史记录
#[tauri::command]
pub async fn apply_pricing_to_records(
    model_id: String,
    pricing: ModelPricingConfig,
    match_mode: String,
    time_range_start: Option<i64>,
    time_range_end: Option<i64>,
    client_tool_filter: Option<String>,
    api_source_key_prefixes: Option<Vec<String>>,
) -> Result<i64, String> {
    let db = crate::proxy::ProxyDatabase::get_global()
        .ok_or_else(|| "Proxy database not available".to_string())?;

    let filter = crate::proxy::PricingMatchFilter {
        model_id: &model_id,
        match_mode: &match_mode,
        time_range_start,
        time_range_end,
        client_tool_filter: client_tool_filter.as_deref(),
        api_source_key_prefixes: api_source_key_prefixes.as_deref(),
    };

    db.apply_pricing_to_records(&pricing, &filter).await
}

#[cfg(test)]
mod tests {
    use super::{parse_model_date, select_unambiguous_pricings};
    use crate::models::ModelPricingConfig;
    use std::collections::HashMap;

    fn pricing(model_id: &str, input: f64, output: f64, updated: i64) -> ModelPricingConfig {
        ModelPricingConfig {
            model_id: model_id.to_string(),
            display_name: None,
            input_price: input,
            output_price: output,
            cache_read_price: None,
            cache_write_price: None,
            source: "api".to_string(),
            last_updated: updated,
        }
    }

    #[test]
    fn parses_month_and_day_dates() {
        assert!(parse_model_date(Some("2026-01"), 0) > 0);
        assert!(parse_model_date(Some("2026-01-02"), 0) > parse_model_date(Some("2026-01"), 0));
        assert_eq!(parse_model_date(Some("bad"), 42), 42);
    }

    #[test]
    fn skips_provider_price_conflicts_but_keeps_equal_variants() {
        let mut candidates = HashMap::new();
        candidates.insert(
            "ambiguous".to_string(),
            vec![
                pricing("ambiguous", 1.0, 2.0, 10),
                pricing("ambiguous", 3.0, 4.0, 20),
            ],
        );
        candidates.insert(
            "stable".to_string(),
            vec![
                pricing("stable", 1.0, 2.0, 10),
                pricing("stable", 1.0, 2.0, 20),
            ],
        );

        let selected = select_unambiguous_pricings(candidates);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].model_id, "stable");
        assert_eq!(selected[0].last_updated, 20);
    }
}
