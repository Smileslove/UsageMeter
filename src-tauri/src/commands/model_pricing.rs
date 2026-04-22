//! 模型价格相关命令

use crate::models::ModelPricingConfig;
use crate::proxy::ProxyDatabase;
use std::sync::Arc;

/// 模型价格数据库实例（独立于代理）
static MODEL_PRICING_DB: std::sync::OnceLock<Arc<std::sync::Mutex<Option<ProxyDatabase>>>> =
    std::sync::OnceLock::new();

/// 获取模型价格数据库实例
fn get_pricing_db() -> Result<Arc<std::sync::Mutex<Option<ProxyDatabase>>>, String> {
    let db = MODEL_PRICING_DB.get_or_init(|| match ProxyDatabase::new() {
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

/// 从 models.dev API 同步模型价格到数据库
#[tauri::command]
pub async fn sync_model_pricing_from_api() -> Result<usize, String> {
    // 1. 从 API 获取价格数据
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("UsageMeter/1.0")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

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
    }

    #[derive(Debug, serde::Deserialize)]
    struct ModelsDevCost {
        #[serde(default)]
        input: f64,
        #[serde(default)]
        output: f64,
        #[serde(default)]
        cache_read: Option<f64>,
        #[serde(default)]
        cache_write: Option<f64>,
    }

    let data: ModelsDevResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse JSON response: {}", e))?;

    let now = chrono::Utc::now().timestamp();
    let mut pricings = Vec::new();

    // 遍历所有厂商和模型
    for (_provider_id, provider) in data.providers {
        for (model_id, model) in provider.models {
            if let Some(cost) = model.cost {
                if cost.input > 0.0 && cost.output > 0.0 {
                    pricings.push(ModelPricingConfig {
                        model_id: if model.id.is_empty() {
                            model_id
                        } else {
                            model.id
                        },
                        display_name: model.name,
                        input_price: cost.input,
                        output_price: cost.output,
                        cache_write_price: cost.cache_write,
                        cache_read_price: cost.cache_read,
                        source: "api".to_string(),
                        last_updated: now,
                    });
                }
            }
        }
    }

    // 按模型 ID 排序
    pricings.sort_by(|a, b| a.model_id.cmp(&b.model_id));

    // 3. 存入数据库（使用 tauri async_runtime spawn_blocking 避免阻塞异步运行时）
    let db_arc = get_pricing_db()?;
    let count = tauri::async_runtime::spawn_blocking(move || {
        let db_guard = db_arc.lock().map_err(|e| format!("Lock error: {}", e))?;

        if let Some(database) = db_guard.as_ref() {
            // 确保表存在
            database.create_model_pricing_table()?;
            database.upsert_model_pricings(&pricings)
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
            let total = database.count_model_pricings(query_clone.as_deref())?;

            Ok(ModelPricingSearchResult { pricings, total })
        } else {
            Err("Database not available".to_string())
        }
    })
    .await
    .map_err(|e| format!("Task error: {}", e))??;

    serde_json::to_string(&result).map_err(|e| format!("Serialization error: {}", e))
}

/// 添加自定义模型价格
#[tauri::command]
pub async fn add_custom_model_pricing(pricing: ModelPricingConfig) -> Result<(), String> {
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
