//! 多货币汇率命令

use serde::Deserialize;
use std::collections::HashMap;

/// open.er-api.com 汇率 API 响应结构
#[derive(Debug, Deserialize)]
struct ExchangeRateResponse {
    result: String,
    #[serde(default)]
    rates: HashMap<String, f64>,
}

/// 从 open.er-api.com 获取指定币种的最新汇率
/// 返回以 USD 为基准的汇率：1 USD = rate 目标货币
#[tauri::command]
pub async fn get_exchange_rates(currencies: Vec<String>) -> Result<HashMap<String, f64>, String> {
    if currencies.is_empty() {
        return Ok(HashMap::new());
    }

    let url = "https://open.er-api.com/v6/latest/USD";
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("ERR_EXCHANGE_RATE_FETCH: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "ERR_EXCHANGE_RATE_STATUS: HTTP {}",
            response.status()
        ));
    }

    let data: ExchangeRateResponse = response
        .json()
        .await
        .map_err(|e| format!("ERR_EXCHANGE_RATE_PARSE: {}", e))?;

    if data.result != "success" {
        return Err("ERR_EXCHANGE_RATE_API: API returned non-success result".to_string());
    }

    // 过滤只保留用户选择的币种，USD 固定为 1.0
    let mut result: HashMap<String, f64> = HashMap::new();
    result.insert("USD".to_string(), 1.0);

    for currency in &currencies {
        if currency == "USD" {
            continue;
        }
        if let Some(&rate) = data.rates.get(currency) {
            result.insert(currency.clone(), rate);
        }
    }

    Ok(result)
}
