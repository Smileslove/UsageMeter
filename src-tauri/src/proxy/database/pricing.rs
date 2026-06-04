use crate::models::ModelPricingConfig;
use serde::{Deserialize, Serialize};

use super::ProxyDatabase;

#[derive(Debug, Clone, Default)]
pub struct PricingMatchFilter<'a> {
    pub model_id: &'a str,
    pub match_mode: &'a str,
    pub time_range_start: Option<i64>,
    pub time_range_end: Option<i64>,
    pub client_tool_filter: Option<&'a str>,
    pub api_source_key_prefixes: Option<&'a [String]>,
}

struct PricingMatchQuery {
    matched_models: Vec<String>,
    where_clause: String,
    params: Vec<Box<dyn rusqlite::types::ToSql>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMatchCount {
    pub model: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPricingApplyResult {
    pub matched_count: i64,
    pub total_current_cost: f64,
    pub model_counts: Vec<ModelMatchCount>,
}

impl ProxyDatabase {
    // ========== 模型价格相关操作 ==========

    /// 创建模型价格表
    pub fn create_model_pricing_table(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        Self::create_model_pricing_table_static(&conn)
    }

    /// 批量插入/更新模型价格（用于同步 API 数据）
    pub fn upsert_model_pricings(&self, pricings: &[ModelPricingConfig]) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut count = 0;

        for pricing in pricings {
            let result = conn.execute(
                r#"
                INSERT INTO model_pricing (model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(model_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    input_price = excluded.input_price,
                    output_price = excluded.output_price,
                    cache_read_price = excluded.cache_read_price,
                    cache_write_price = excluded.cache_write_price,
                    source = excluded.source,
                    last_updated = excluded.last_updated
                WHERE source != 'custom'
                "#,
                rusqlite::params![
                    pricing.model_id,
                    pricing.display_name,
                    pricing.input_price,
                    pricing.output_price,
                    pricing.cache_read_price,
                    pricing.cache_write_price,
                    pricing.source,
                    pricing.last_updated,
                ],
            );
            if result.is_ok() {
                count += 1;
            }
        }

        Ok(count)
    }

    /// 搜索模型价格（支持分页和关键词搜索）
    /// 用于搜索同步模型（排除自定义模型）
    pub fn search_model_pricings(
        &self,
        query: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let pricings = if let Some(q) = query {
            let search_pattern = format!("%{}%", q.to_lowercase());
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE source != 'custom' AND (model_id LIKE ?1 OR LOWER(display_name) LIKE ?1)
                ORDER BY model_id
                LIMIT ?2 OFFSET ?3
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt
                .query_map(rusqlite::params![search_pattern, limit, offset], |row| {
                    Ok(ModelPricingConfig {
                        model_id: row.get(0)?,
                        display_name: row.get(1)?,
                        input_price: row.get(2)?,
                        output_price: row.get(3)?,
                        cache_read_price: row.get(4)?,
                        cache_write_price: row.get(5)?,
                        source: row.get(6)?,
                        last_updated: row.get(7)?,
                    })
                })
                .map_err(|e| format!("Failed to search model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE source != 'custom'
                ORDER BY model_id
                LIMIT ?1 OFFSET ?2
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt
                .query_map(rusqlite::params![limit, offset], |row| {
                    Ok(ModelPricingConfig {
                        model_id: row.get(0)?,
                        display_name: row.get(1)?,
                        input_price: row.get(2)?,
                        output_price: row.get(3)?,
                        cache_read_price: row.get(4)?,
                        cache_write_price: row.get(5)?,
                        source: row.get(6)?,
                        last_updated: row.get(7)?,
                    })
                })
                .map_err(|e| format!("Failed to query model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        };

        Ok(pricings)
    }

    /// 获取自定义模型价格列表（支持搜索）
    pub fn get_custom_model_pricings(
        &self,
        query: Option<&str>,
    ) -> Result<Vec<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let pricings = if let Some(q) = query {
            let search_pattern = format!("%{}%", q.to_lowercase());
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE source = 'custom' AND (model_id LIKE ?1 OR LOWER(display_name) LIKE ?1)
                ORDER BY model_id
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt
                .query_map(rusqlite::params![search_pattern], |row| {
                    Ok(ModelPricingConfig {
                        model_id: row.get(0)?,
                        display_name: row.get(1)?,
                        input_price: row.get(2)?,
                        output_price: row.get(3)?,
                        cache_read_price: row.get(4)?,
                        cache_write_price: row.get(5)?,
                        source: row.get(6)?,
                        last_updated: row.get(7)?,
                    })
                })
                .map_err(|e| format!("Failed to query custom model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        } else {
            let mut stmt = conn.prepare(
                r#"
                SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated
                FROM model_pricing
                WHERE source = 'custom'
                ORDER BY model_id
                "#
            ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(ModelPricingConfig {
                        model_id: row.get(0)?,
                        display_name: row.get(1)?,
                        input_price: row.get(2)?,
                        output_price: row.get(3)?,
                        cache_read_price: row.get(4)?,
                        cache_write_price: row.get(5)?,
                        source: row.get(6)?,
                        last_updated: row.get(7)?,
                    })
                })
                .map_err(|e| format!("Failed to query custom model pricings: {}", e))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect results: {}", e))?
        };

        Ok(pricings)
    }

    /// 获取同步模型总数
    pub fn count_synced_model_pricings(&self, query: Option<&str>) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;

        let count = if let Some(q) = query {
            let search_pattern = format!("%{}%", q.to_lowercase());
            conn.query_row(
                "SELECT COUNT(*) FROM model_pricing WHERE source != 'custom' AND (model_id LIKE ?1 OR LOWER(display_name) LIKE ?1)",
                rusqlite::params![search_pattern],
                |row| row.get(0),
            )
        } else {
            conn.query_row("SELECT COUNT(*) FROM model_pricing WHERE source != 'custom'", [], |row| row.get(0))
        }
        .map_err(|e| format!("Failed to count synced model pricings: {}", e))?;

        Ok(count)
    }

    /// 添加自定义模型价格（使用 UPSERT，如果已存在则更新）
    pub fn add_custom_pricing(&self, pricing: &ModelPricingConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute(
            r#"
            INSERT INTO model_pricing (model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(model_id) DO UPDATE SET
                display_name = excluded.display_name,
                input_price = excluded.input_price,
                output_price = excluded.output_price,
                cache_read_price = excluded.cache_read_price,
                cache_write_price = excluded.cache_write_price,
                source = excluded.source,
                last_updated = excluded.last_updated
            "#,
            rusqlite::params![
                pricing.model_id,
                pricing.display_name,
                pricing.input_price,
                pricing.output_price,
                pricing.cache_read_price,
                pricing.cache_write_price,
                "custom",
                pricing.last_updated,
            ],
        )
        .map_err(|e| format!("Failed to add custom pricing: {}", e))?;
        Ok(())
    }

    /// 更新自定义模型价格
    pub fn update_custom_pricing(&self, pricing: &ModelPricingConfig) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute(
            r#"
            UPDATE model_pricing SET
                display_name = ?2,
                input_price = ?3,
                output_price = ?4,
                cache_read_price = ?5,
                cache_write_price = ?6,
                last_updated = ?7
            WHERE model_id = ?1
            "#,
            rusqlite::params![
                pricing.model_id,
                pricing.display_name,
                pricing.input_price,
                pricing.output_price,
                pricing.cache_read_price,
                pricing.cache_write_price,
                pricing.last_updated,
            ],
        )
        .map_err(|e| format!("Failed to update custom pricing: {}", e))?;
        Ok(())
    }

    /// 删除模型价格
    pub fn delete_model_pricing(&self, model_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        conn.execute(
            "DELETE FROM model_pricing WHERE model_id = ?1",
            rusqlite::params![model_id],
        )
        .map_err(|e| format!("Failed to delete model pricing: {}", e))?;
        Ok(())
    }

    /// 清空所有同步的模型价格（保留自定义模型）
    pub fn clear_synced_model_pricings(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let count = conn
            .execute("DELETE FROM model_pricing WHERE source != 'custom'", [])
            .map_err(|e| format!("Failed to clear synced model pricings: {}", e))?;
        Ok(count)
    }

    /// 根据 model_id 查找价格配置
    #[allow(dead_code)]
    pub fn get_model_pricing(&self, model_id: &str) -> Result<Option<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated FROM model_pricing WHERE model_id = ?1"
        ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let result = stmt.query_row(rusqlite::params![model_id], |row| {
            Ok(ModelPricingConfig {
                model_id: row.get(0)?,
                display_name: row.get(1)?,
                input_price: row.get(2)?,
                output_price: row.get(3)?,
                cache_read_price: row.get(4)?,
                cache_write_price: row.get(5)?,
                source: row.get(6)?,
                last_updated: row.get(7)?,
            })
        });

        match result {
            Ok(pricing) => Ok(Some(pricing)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get model pricing: {}", e)),
        }
    }

    /// 获取所有模型价格配置（用于费用计算）
    pub fn get_all_model_pricings(&self) -> Result<Vec<ModelPricingConfig>, String> {
        let conn = self.conn.lock().map_err(|e| format!("Lock error: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT model_id, display_name, input_price, output_price, cache_read_price, cache_write_price, source, last_updated FROM model_pricing ORDER BY model_id"
        ).map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let pricings = stmt
            .query_map([], |row| {
                Ok(ModelPricingConfig {
                    model_id: row.get(0)?,
                    display_name: row.get(1)?,
                    input_price: row.get(2)?,
                    output_price: row.get(3)?,
                    cache_read_price: row.get(4)?,
                    cache_write_price: row.get(5)?,
                    source: row.get(6)?,
                    last_updated: row.get(7)?,
                })
            })
            .map_err(|e| format!("Failed to query model pricings: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect results: {}", e))?;

        Ok(pricings)
    }

    fn build_pricing_match_params(
        conn: &rusqlite::Connection,
        filter: &PricingMatchFilter<'_>,
    ) -> Result<PricingMatchQuery, String> {
        if filter.match_mode == "exact" {
            return Self::build_exact_match_params(filter);
        }
        Self::build_fuzzy_match_params(conn, filter)
    }

    fn push_extra_conditions(
        filter: &PricingMatchFilter<'_>,
        params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
        extra_conditions: &mut String,
    ) {
        if let Some(start) = filter.time_range_start {
            extra_conditions.push_str(&format!(" AND timestamp >= ?{}", params.len() + 1));
            params.push(Box::new(start));
        }
        if let Some(end) = filter.time_range_end {
            extra_conditions.push_str(&format!(" AND timestamp <= ?{}", params.len() + 1));
            params.push(Box::new(end));
        }
        if let Some(tool) = filter.client_tool_filter {
            extra_conditions.push_str(&format!(" AND client_tool = ?{}", params.len() + 1));
            params.push(Box::new(tool.to_string()));
        }
        if let Some(prefixes) = filter.api_source_key_prefixes {
            if !prefixes.is_empty() {
                let prefix_placeholders: Vec<String> = prefixes
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", params.len() + 1 + i))
                    .collect();
                extra_conditions.push_str(&format!(
                    " AND api_key_prefix IN ({})",
                    prefix_placeholders.join(",")
                ));
                for p in prefixes {
                    params.push(Box::new(p.clone()));
                }
            }
        }
    }

    fn build_exact_match_params(
        filter: &PricingMatchFilter<'_>,
    ) -> Result<PricingMatchQuery, String> {
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(filter.model_id.to_string())];
        let mut extra_conditions = String::new();

        Self::push_extra_conditions(filter, &mut params, &mut extra_conditions);

        Ok(PricingMatchQuery {
            matched_models: vec![filter.model_id.to_string()],
            where_clause: format!("= ?1{}", extra_conditions),
            params,
        })
    }

    fn build_fuzzy_match_params(
        conn: &rusqlite::Connection,
        filter: &PricingMatchFilter<'_>,
    ) -> Result<PricingMatchQuery, String> {
        let mut stmt = conn
            .prepare("SELECT DISTINCT model FROM usage_records WHERE model != ''")
            .map_err(|e| format!("Failed to prepare model query: {}", e))?;
        let all_models: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query models: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect models: {}", e))?;

        let pricing_config = crate::models::ModelPricingConfig {
            model_id: filter.model_id.to_string(),
            display_name: None,
            input_price: 0.0,
            output_price: 0.0,
            cache_write_price: None,
            cache_read_price: None,
            source: String::new(),
            last_updated: 0,
        };
        let normalized = crate::models::normalize_model_id(filter.model_id);
        let matched_models: Vec<String> = all_models
            .into_iter()
            .filter(|m| crate::models::fuzzy_match_score(m, &normalized, &pricing_config).is_some())
            .collect();

        if matched_models.is_empty() {
            return Ok(PricingMatchQuery {
                matched_models: vec![],
                where_clause: String::new(),
                params: vec![],
            });
        }

        let placeholders: Vec<String> = matched_models
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let in_clause = placeholders.join(",");

        let mut extra_conditions = String::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = matched_models
            .iter()
            .map(|m| Box::new(m.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();

        Self::push_extra_conditions(filter, &mut params, &mut extra_conditions);

        Ok(PricingMatchQuery {
            matched_models,
            where_clause: format!("IN ({}){}", in_clause, extra_conditions),
            params,
        })
    }

    pub async fn preview_pricing_apply(
        &self,
        filter: &PricingMatchFilter<'_>,
    ) -> Result<PreviewPricingApplyResult, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let query = Self::build_pricing_match_params(&conn, filter)?;

        if query.matched_models.is_empty() {
            return Ok(PreviewPricingApplyResult {
                matched_count: 0,
                total_current_cost: 0.0,
                model_counts: vec![],
            });
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            query.params.iter().map(|p| p.as_ref()).collect();

        let sql = format!(
            r#"
            SELECT COUNT(*), COALESCE(SUM(estimated_cost), 0)
            FROM usage_records
            WHERE model {}
              AND (cost_locked = 0 OR cost_locked IS NULL)
            "#,
            query.where_clause
        );
        let (matched_count, total_current_cost) = conn
            .query_row(&sql, param_refs.as_slice(), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| format!("Failed to preview pricing apply: {}", e))?;

        let sql_models = format!(
            r#"
            SELECT model, COUNT(*) as cnt
            FROM usage_records
            WHERE model {}
              AND (cost_locked = 0 OR cost_locked IS NULL)
            GROUP BY model
            ORDER BY cnt DESC
            "#,
            query.where_clause
        );
        let mut stmt = conn
            .prepare(&sql_models)
            .map_err(|e| format!("Failed to prepare model count query: {}", e))?;
        let model_counts: Vec<ModelMatchCount> = stmt
            .query_map(param_refs.as_slice(), |row| {
                Ok(ModelMatchCount {
                    model: row.get::<_, String>(0)?,
                    count: row.get::<_, i64>(1)?,
                })
            })
            .map_err(|e| format!("Failed to query model counts: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect model counts: {}", e))?;

        Ok(PreviewPricingApplyResult {
            matched_count,
            total_current_cost,
            model_counts,
        })
    }

    pub async fn apply_pricing_to_records(
        &self,
        pricing: &crate::models::ModelPricingConfig,
        filter: &PricingMatchFilter<'_>,
    ) -> Result<i64, String> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let query = Self::build_pricing_match_params(&conn, filter)?;

        if query.matched_models.is_empty() {
            return Ok(0);
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            query.params.iter().map(|p| p.as_ref()).collect();

        let sql = format!(
            r#"
            SELECT id, timestamp, input_tokens, output_tokens, cache_create_tokens,
                   cache_read_tokens, model
            FROM usage_records
            WHERE model {}
              AND (cost_locked = 0 OR cost_locked IS NULL)
            "#,
            query.where_clause
        );

        let records: Vec<(i64, i64, u64, u64, u64, u64, String)> = {
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| format!("Failed to prepare apply query: {}", e))?;
            let rows = stmt
                .query_map(param_refs.as_slice(), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        Self::safe_i64_to_u64(row.get::<_, i64>(2)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(3)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(4)?),
                        Self::safe_i64_to_u64(row.get::<_, i64>(5)?),
                        row.get::<_, String>(6)?,
                    ))
                })
                .map_err(|e| format!("Failed to query records for apply: {}", e))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect records for apply: {}", e))?
        };

        if records.is_empty() {
            return Ok(0);
        }

        let pricings = vec![pricing.clone()];
        let snapshot_id = Self::pricing_snapshot_id(&pricings, "exact");

        let touched_dates: std::collections::HashSet<String> = records
            .iter()
            .filter(|(_, timestamp, _, _, _, _, _)| {
                let date = Self::record_local_date(*timestamp);
                date < Self::today_local_date()
            })
            .map(|(_, timestamp, _, _, _, _, _)| Self::record_local_date(*timestamp))
            .collect();

        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to begin transaction: {}", e))?;
        let now = chrono::Utc::now().timestamp();

        const BATCH_SIZE: usize = 1000;
        let mut total_updated: i64 = 0;

        let mut update_stmt = tx
            .prepare(
                r#"
                UPDATE usage_records
                SET estimated_cost = ?1, pricing_snapshot_id = ?2, cost_locked = 1, updated_at = ?3
                WHERE id = ?4
                "#,
            )
            .map_err(|e| format!("Failed to prepare update statement: {}", e))?;

        for batch in records.chunks(BATCH_SIZE) {
            for (id, _timestamp, input, output, cache_create, cache_read, model) in batch {
                let cost = crate::models::estimate_session_cost(
                    *input,
                    *output,
                    *cache_create,
                    *cache_read,
                    model,
                    &pricings,
                    "exact",
                );
                update_stmt
                    .execute(rusqlite::params![cost, &snapshot_id, now, id])
                    .map_err(|e| format!("Failed to update record: {}", e))?;
                total_updated += 1;
            }
        }
        drop(update_stmt);

        for date in &touched_dates {
            Self::refresh_daily_summary_for_date_conn(&tx, date)?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit transaction: {}", e))?;

        eprintln!(
            "[database] Applied pricing to {} records for model '{}'",
            total_updated, filter.model_id
        );
        if !touched_dates.is_empty() {
            if let Ok(local_db) = crate::local_usage::LocalUsageDatabase::get_global() {
                let _ = local_db.invalidate_unified_materialization_dates(
                    &touched_dates.into_iter().collect::<Vec<_>>(),
                );
            }
        }
        Ok(total_updated)
    }
}
