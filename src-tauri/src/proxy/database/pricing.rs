use crate::models::ModelPricingConfig;

use super::ProxyDatabase;
use super::StatusCodeDistribution;

impl ProxyDatabase {
    /// 获取状态码分布
    #[allow(dead_code)]
    pub async fn get_status_code_distribution(
        &self,
        cutoff_ms: i64,
    ) -> Result<Vec<StatusCodeDistribution>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Failed to lock connection: {}", e))?;

        let mut stmt = conn
            .prepare(
                r#"
                SELECT status_code, COUNT(*) as count
                FROM usage_records
                WHERE timestamp >= ?1
                GROUP BY status_code
                ORDER BY count DESC
                "#,
            )
            .map_err(|e| format!("Failed to prepare statement: {}", e))?;

        let distribution = stmt
            .query_map([cutoff_ms], |row| {
                let status_code: i64 = row.get(0)?;
                let count: i64 = row.get(1)?;
                let category = if (200..300).contains(&status_code) {
                    "success".to_string()
                } else if (400..500).contains(&status_code) {
                    "client_error".to_string()
                } else if status_code >= 500 {
                    "server_error".to_string()
                } else {
                    "other".to_string()
                };
                Ok(StatusCodeDistribution {
                    status_code,
                    count,
                    category,
                })
            })
            .map_err(|e| format!("Failed to query status code distribution: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect status code distribution: {}", e))?;

        Ok(distribution)
    }

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
}
