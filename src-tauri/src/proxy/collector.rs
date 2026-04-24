//! 使用量收集器，用于聚合 API 使用数据

use super::database::{ModelDistribution, ProxyDatabase, WindowRateStats, WindowRateSummary};
use super::types::{SessionStats, UsageRecord, WindowStats};
use crate::models::ModelPricingConfig;
use chrono::{Datelike, Duration, Local, TimeZone};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 使用量收集器，用于聚合代理请求的使用数据
pub struct UsageCollector {
    /// 用于持久化存储的数据库
    database: Arc<ProxyDatabase>,
    /// 最近记录的内存缓存（用于快速访问）
    recent_records: Arc<tokio::sync::RwLock<Vec<UsageRecord>>>,
    /// 内存缓存中保留的最大最近记录数
    max_recent: usize,
}

impl UsageCollector {
    /// 创建新的使用量收集器（带数据库持久化）
    pub fn new() -> Self {
        Self {
            database: Arc::new(ProxyDatabase::new().expect("Failed to initialize database")),
            recent_records: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            max_recent: 1000,
        }
    }

    /// 使用现有数据库创建收集器
    #[allow(dead_code)]
    pub fn with_database(database: Arc<ProxyDatabase>) -> Self {
        Self {
            database,
            recent_records: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            max_recent: 1000,
        }
    }

    /// 记录使用事件（持久化到数据库并更新内存缓存）
    pub async fn record(&self, record: UsageRecord) {
        // 检查最近缓存中是否有重复
        let is_duplicate = {
            let mut recent = self.recent_records.write().await;
            if !record.message_id.is_empty() {
                if let Some(existing) = recent.iter().find(|r| r.message_id == record.message_id) {
                    // 如果新记录有更多 token，则更新现有记录
                    if record.total_tokens > existing.total_tokens {
                        if let Some(idx) = recent
                            .iter()
                            .position(|r| r.message_id == record.message_id)
                        {
                            recent[idx] = record.clone();
                        }
                    }
                    true // 是重复的，已在缓存中处理
                } else {
                    // 不是重复的，添加到最近缓存
                    recent.push(record.clone());
                    if recent.len() > self.max_recent {
                        recent.remove(0);
                    }
                    false
                }
            } else {
                // 没有 message_id，直接添加到缓存
                recent.push(record.clone());
                if recent.len() > self.max_recent {
                    recent.remove(0);
                }
                false
            }
            // 此处作用域结束，锁被释放
        };

        // 保存到数据库以持久化（在锁外部）
        if is_duplicate {
            // 对于重复记录，仍然保存到数据库进行更新
            if let Err(e) = self.database.insert_record(&record).await {
                eprintln!("Failed to save record to database: {}", e);
            }
        } else {
            // 保存新记录
            if let Err(e) = self.database.insert_record(&record).await {
                eprintln!("Failed to save record to database: {}", e);
            }
        }

        // 增量更新 session_stats 表
        // 即使 session_id 为空，也会尝试通过 message_id 从 JSONL 查找对应的 session_id
        if let Err(e) = self
            .database
            .update_session_stats_incremental(&record)
            .await
        {
            eprintln!("[collector] Failed to update session stats: {}", e);
        }
    }

    /// 获取时间窗口内的记录（从数据库）
    #[allow(dead_code)]
    pub async fn get_records_since(&self, cutoff_ms: i64) -> Vec<UsageRecord> {
        match self.database.get_records_since(cutoff_ms).await {
            Ok(records) => records,
            Err(e) => {
                eprintln!("Failed to get records from database: {}", e);
                Vec::new()
            }
        }
    }

    /// 获取特定时间窗口的统计数据
    ///
    /// 时间窗口定义：
    /// - "5h": 滑动窗口，当前时间往前推 5 小时
    /// - "1d": 自然日，今天 00:00:00 到现在
    /// - "7d": 自然周，本周一 00:00:00 到现在
    /// - "30d": 滑动窗口，当前时间往前推 30 天
    /// - "current_month": 自然月，本月 1 日 00:00:00 到现在
    ///
    /// # 参数
    /// - `window`: 时间窗口名称
    /// - `include_errors`: 是否包含错误请求（4xx/5xx）
    pub async fn get_window_stats(&self, window: &str, include_errors: bool) -> WindowStats {
        let cutoff_ms = Self::calculate_window_cutoff(window);
        let now = Self::current_timestamp();

        match self
            .database
            .get_window_stats_filtered(cutoff_ms, include_errors)
            .await
        {
            Ok(aggregate) => WindowStats {
                window: window.to_string(),
                token_used: (aggregate.input_tokens
                    + aggregate.cache_create_tokens
                    + aggregate.cache_read_tokens
                    + aggregate.output_tokens) as u64,
                input_tokens: aggregate.input_tokens as u64,
                output_tokens: aggregate.output_tokens as u64,
                cache_create_tokens: aggregate.cache_create_tokens as u64,
                cache_read_tokens: aggregate.cache_read_tokens as u64,
                request_used: aggregate.request_count as u64,
                last_updated: now,
                success_requests: aggregate.status_2xx as u64,
                client_error_requests: aggregate.status_4xx as u64,
                server_error_requests: aggregate.status_5xx as u64,
            },
            Err(e) => {
                eprintln!("Failed to get window stats: {}", e);
                WindowStats::default()
            }
        }
    }

    /// 计算时间窗口的截止时间戳（毫秒）
    ///
    /// 返回窗口开始时间的 Unix 时间戳（毫秒）
    fn calculate_window_cutoff(window: &str) -> i64 {
        Self::calculate_window_cutoff_public(window)
    }

    /// 计算时间窗口的截止时间戳（毫秒）- 公开方法
    ///
    /// 返回窗口开始时间的 Unix 时间戳（毫秒）
    pub fn calculate_window_cutoff_public(window: &str) -> i64 {
        let now = Local::now();

        match window {
            "5h" => {
                // 滑动窗口：当前时间往前推 5 小时
                let cutoff = now - Duration::hours(5);
                cutoff.timestamp_millis()
            }
            "1d" => {
                // 自然日：今天 00:00:00
                let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
                Local
                    .from_local_datetime(&today_start)
                    .unwrap()
                    .timestamp_millis()
            }
            "7d" => {
                // 自然周：本周一 00:00:00
                let weekday = now.weekday().num_days_from_monday();
                let monday = now.date_naive() - Duration::days(weekday as i64);
                let week_start = monday.and_hms_opt(0, 0, 0).unwrap();
                Local
                    .from_local_datetime(&week_start)
                    .unwrap()
                    .timestamp_millis()
            }
            "30d" => {
                // 滑动窗口：当前时间往前推 30 天
                let cutoff = now - Duration::days(30);
                cutoff.timestamp_millis()
            }
            "current_month" => {
                // 自然月：本月 1 日 00:00:00
                let month_start = now
                    .date_naive()
                    .with_day(1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap();
                Local
                    .from_local_datetime(&month_start)
                    .unwrap()
                    .timestamp_millis()
            }
            _ => {
                // 默认：1 天滑动窗口
                let cutoff = now - Duration::hours(24);
                cutoff.timestamp_millis()
            }
        }
    }

    /// 获取所有时间窗口的统计数据
    ///
    /// # 参数
    /// - `include_errors`: 是否包含错误请求（4xx/5xx）
    pub async fn get_all_window_stats(
        &self,
        include_errors: bool,
    ) -> std::collections::HashMap<String, WindowStats> {
        let mut result = std::collections::HashMap::new();
        for window in &["5h", "1d", "7d", "30d", "current_month"] {
            result.insert(
                window.to_string(),
                self.get_window_stats(window, include_errors).await,
            );
        }
        result
    }

    /// 获取时间窗口内的模型分布
    pub async fn get_model_distribution(&self, window: &str) -> Vec<ModelDistribution> {
        let cutoff_ms = Self::calculate_window_cutoff(window);

        match self.database.get_model_distribution(cutoff_ms).await {
            Ok(models) => models,
            Err(e) => {
                eprintln!("Failed to get model distribution: {}", e);
                Vec::new()
            }
        }
    }

    /// 清除所有记录（数据库和缓存）
    #[allow(dead_code)]
    pub async fn clear(&self) {
        // 清除最近缓存
        self.recent_records.write().await.clear();

        // 注意：我们不清除数据库以保留历史记录
        // 如果需要，可以添加单独的方法来清理数据库
    }

    /// 获取总记录数
    pub async fn record_count(&self) -> usize {
        match self.database.get_record_count().await {
            Ok(count) => count,
            Err(_) => self.recent_records.read().await.len(),
        }
    }

    /// 获取当前时间戳（毫秒）
    fn current_timestamp() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    /// 清理数据库中的旧记录
    #[allow(dead_code)]
    pub async fn cleanup_old_records(&self, days: i64) -> Result<usize, String> {
        self.database.cleanup_old_records(days).await
    }

    /// 获取会话统计信息
    #[allow(dead_code)]
    pub async fn get_session_stats(
        &self,
        session_id: &str,
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> Option<SessionStats> {
        match self
            .database
            .get_session_stats(session_id, pricings, match_mode)
            .await
        {
            Ok(stats) => stats,
            Err(e) => {
                eprintln!("Failed to get session stats: {}", e);
                None
            }
        }
    }

    /// 获取数据库引用（用于模型价格等操作）
    #[allow(dead_code)]
    pub fn get_database(&self) -> Arc<ProxyDatabase> {
        self.database.clone()
    }

    /// 获取所有会话列表（按最后请求时间倒序）
    #[allow(dead_code)]
    pub async fn get_all_sessions(
        &self,
        limit: i64,
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> Vec<SessionStats> {
        match self
            .database
            .get_all_sessions(limit, pricings, match_mode)
            .await
        {
            Ok(sessions) => sessions,
            Err(e) => {
                eprintln!("Failed to get all sessions: {}", e);
                Vec::new()
            }
        }
    }

    /// 通过 message_id 列表查询会话统计信息
    ///
    /// 用于将 JSONL 会话文件中的消息与代理数据库记录关联
    #[allow(dead_code)]
    pub async fn get_session_stats_by_message_ids(
        &self,
        message_ids: &[String],
        pricings: &[ModelPricingConfig],
        match_mode: &str,
    ) -> Option<SessionStats> {
        self.database
            .get_session_stats_by_message_ids(message_ids, pricings, match_mode)
            .await
    }

    /// 获取窗口内的速率统计
    #[allow(dead_code)]
    pub async fn get_window_rate_stats(&self, window: &str) -> WindowRateStats {
        let cutoff_ms = Self::calculate_window_cutoff(window);

        match self.database.get_window_rate_stats(cutoff_ms).await {
            Ok(stats) => stats,
            Err(e) => {
                eprintln!("Failed to get window rate stats: {}", e);
                WindowRateStats::default()
            }
        }
    }

    /// 获取窗口速率汇总（整体 + 按模型分组）
    pub async fn get_window_rate_summary(&self, window: &str) -> WindowRateSummary {
        let cutoff_ms = Self::calculate_window_cutoff(window);

        let overall = match self.database.get_window_rate_stats(cutoff_ms).await {
            Ok(stats) => stats,
            Err(e) => {
                eprintln!("Failed to get window rate stats: {}", e);
                WindowRateStats::default()
            }
        };

        let by_model = match self.database.get_model_rate_stats(cutoff_ms).await {
            Ok(models) => models,
            Err(e) => {
                eprintln!("Failed to get model rate stats: {}", e);
                Vec::new()
            }
        };

        WindowRateSummary {
            window: window.to_string(),
            overall,
            by_model,
        }
    }

    /// 获取状态码分布
    #[allow(dead_code)]
    pub async fn get_status_code_distribution(
        &self,
        window: &str,
    ) -> Vec<super::database::StatusCodeDistribution> {
        let cutoff_ms = Self::calculate_window_cutoff(window);

        match self.database.get_status_code_distribution(cutoff_ms).await {
            Ok(distribution) => distribution,
            Err(e) => {
                eprintln!("Failed to get status code distribution: {}", e);
                Vec::new()
            }
        }
    }

    /// 获取窗口内的 TTFT 统计（首 Token 生成时间）
    pub async fn get_ttft_stats(&self, cutoff_ms: i64) -> super::database::TtftStats {
        match self.database.get_ttft_stats(cutoff_ms).await {
            Ok(stats) => stats,
            Err(e) => {
                eprintln!("Failed to get TTFT stats: {}", e);
                super::database::TtftStats::default()
            }
        }
    }

    /// 获取窗口内按模型分组的 TTFT 统计
    pub async fn get_model_ttft_stats(
        &self,
        cutoff_ms: i64,
    ) -> Vec<super::database::ModelTtftStats> {
        match self.database.get_model_ttft_stats(cutoff_ms).await {
            Ok(models) => models,
            Err(e) => {
                eprintln!("Failed to get model TTFT stats: {}", e);
                Vec::new()
            }
        }
    }
}

impl Default for UsageCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_cutoff_calculation() {
        // 测试窗口截止时间计算是否正确
        let now = Local::now();

        // 5h 滑动窗口：应该约为 5 小时前
        let cutoff_5h = UsageCollector::calculate_window_cutoff("5h");
        let expected_5h = (now - Duration::hours(5)).timestamp_millis();
        // 允许 1 秒误差
        assert!((cutoff_5h - expected_5h).abs() < 1000);

        // 1d 自然日：应该是今天 00:00:00
        let cutoff_1d = UsageCollector::calculate_window_cutoff("1d");
        let today_start = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
        let expected_1d = Local
            .from_local_datetime(&today_start)
            .unwrap()
            .timestamp_millis();
        assert_eq!(cutoff_1d, expected_1d);

        // 7d 自然周：应该是本周一 00:00:00
        let cutoff_7d = UsageCollector::calculate_window_cutoff("7d");
        let weekday = now.weekday().num_days_from_monday();
        let monday = now.date_naive() - Duration::days(weekday as i64);
        let week_start = monday.and_hms_opt(0, 0, 0).unwrap();
        let expected_7d = Local
            .from_local_datetime(&week_start)
            .unwrap()
            .timestamp_millis();
        assert_eq!(cutoff_7d, expected_7d);

        // 30d 滑动窗口：应该约为 30 天前
        let cutoff_30d = UsageCollector::calculate_window_cutoff("30d");
        let expected_30d = (now - Duration::days(30)).timestamp_millis();
        assert!((cutoff_30d - expected_30d).abs() < 1000);

        // current_month 自然月：应该是本月 1 日 00:00:00
        let cutoff_current_month = UsageCollector::calculate_window_cutoff("current_month");
        let month_start = now
            .date_naive()
            .with_day(1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let expected_current_month = Local
            .from_local_datetime(&month_start)
            .unwrap()
            .timestamp_millis();
        assert_eq!(cutoff_current_month, expected_current_month);
    }

    #[test]
    fn test_window_ordering() {
        // 验证窗口截止时间的逻辑正确性
        let cutoff_5h = UsageCollector::calculate_window_cutoff("5h");
        let cutoff_1d = UsageCollector::calculate_window_cutoff("1d");
        let cutoff_7d = UsageCollector::calculate_window_cutoff("7d");
        let cutoff_30d = UsageCollector::calculate_window_cutoff("30d");
        let cutoff_current_month = UsageCollector::calculate_window_cutoff("current_month");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        // current_month 是本月第一天，30d 是30天前
        // 如果当前是月中之后，current_month 会晚于 30d
        // 如果当前是月初，两者可能接近或 current_month 更早
        // 这里只验证它们都是合理的时间戳
        assert!(cutoff_current_month > 0);
        assert!(cutoff_30d > 0);

        // 验证滑动窗口的顺序：30d <= 7d（30天前早于或等于7天前）
        // 注意：7d 是本周一，30d 是30天前，所以 30d 应该更早
        assert!(cutoff_30d <= cutoff_7d);
        // 所有截止时间都应该在过去
        assert!(cutoff_5h < now);
        assert!(cutoff_1d < now);
        assert!(cutoff_7d < now);
        assert!(cutoff_30d < now);
        assert!(cutoff_current_month < now);
    }

    #[tokio::test]
    async fn test_record_creation() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let record = UsageRecord {
            timestamp: now,
            message_id: "test-msg".to_string(),
            input_tokens: 100,
            output_tokens: 200,
            cache_create_tokens: 10,
            cache_read_tokens: 20,
            total_tokens: 330, // 总 Token = input(100) + cache_create(10) + cache_read(20) + output(200)
            model: "claude-sonnet-4".to_string(),
            session_id: Some("session-123".to_string()),
            request_start_time: now - 5000, // 5 秒前开始
            request_end_time: now,
            duration_ms: 5000,
            output_tokens_per_second: Some(40.0), // 200 tokens / 5 seconds
            ttft_ms: Some(100),
            status_code: 200,
        };

        assert_eq!(record.message_id, "test-msg");
        assert_eq!(record.total_tokens, 330); // input(100) + cache_create(10) + cache_read(20) + output(200)
        assert_eq!(record.duration_ms, 5000);
        assert_eq!(record.output_tokens_per_second, Some(40.0));
    }
}
