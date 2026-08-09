//! 使用量收集器，用于聚合 API 使用数据

use super::database::{ProxyDatabase, WindowRateStats};
use super::reconciliation::ProxySessionReconciliationService;
use super::types::{SessionStats, UsageRecord, WindowStats};
use crate::models::ModelPricingConfig;
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
    /// 将已写入的代理事实投影到会话统计的应用服务。
    session_reconciliation: ProxySessionReconciliationService,
}

impl UsageCollector {
    fn recent_dedupe_key(record: &UsageRecord) -> String {
        if let Some(key) = record.storage_dedupe_key.as_ref() {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Some(key) = record.canonical_request_key.as_ref() {
            let trimmed = key.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if record.client_tool == "opencode" && !record.message_id.trim().is_empty() {
            let stable_time = if record.request_start_time > 0 {
                record.request_start_time
            } else {
                record.timestamp
            };
            return format!(
                "{}:{}:{}",
                record.client_tool, record.message_id, stable_time
            );
        }
        if !record.message_id.trim().is_empty() {
            return format!("{}:{}", record.client_tool, record.message_id);
        }
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            record.client_tool,
            record.session_id.clone().unwrap_or_default(),
            record.timestamp / 1000,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.cache_create_tokens,
            record.cache_read_tokens,
            record.total_tokens
        )
    }

    /// 创建新的使用量收集器（带数据库持久化）
    ///
    /// 复用进程级全局数据库实例：启动期多个 tokio worker 可能并发构造
    /// 收集器（如被动代理监控与代理服务器启动），若各自 `ProxyDatabase::new()`
    /// 打开独立连接并并发跑 schema 迁移，SQLite 单写者会报 `database is locked`。
    /// 全局单例 + 初始化互斥（见 `ProxyDatabase::get_or_create_global`）保证
    /// 迁移只执行一次且串行化。
    pub fn new() -> Self {
        Self {
            database: ProxyDatabase::get_or_create_global().expect("Failed to initialize database"),
            recent_records: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            max_recent: 1000,
            session_reconciliation: ProxySessionReconciliationService::default(),
        }
    }

    /// 使用现有数据库创建收集器
    #[allow(dead_code)]
    pub fn with_database(database: Arc<ProxyDatabase>) -> Self {
        Self {
            database,
            recent_records: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            max_recent: 1000,
            session_reconciliation: ProxySessionReconciliationService::default(),
        }
    }

    /// 记录使用事件（持久化到数据库并更新内存缓存）
    pub async fn record(&self, record: UsageRecord) {
        {
            let mut recent = self.recent_records.write().await;
            let record_key = Self::recent_dedupe_key(&record);
            if !record_key.is_empty() {
                if let Some(existing) = recent
                    .iter()
                    .find(|r| Self::recent_dedupe_key(r) == record_key)
                {
                    // 如果新记录有更多 token，则更新现有记录
                    if record.total_tokens > existing.total_tokens {
                        if let Some(idx) = recent
                            .iter()
                            .position(|r| Self::recent_dedupe_key(r) == record_key)
                        {
                            recent[idx] = record.clone();
                        }
                    } // 是重复的，仅更新内存中的最新快照
                } else {
                    // 不是重复的，添加到最近缓存
                    recent.push(record.clone());
                    if recent.len() > self.max_recent {
                        recent.remove(0);
                    }
                }
            } else {
                // 没有 message_id，直接添加到缓存
                recent.push(record.clone());
                if recent.len() > self.max_recent {
                    recent.remove(0);
                }
            }
            // 此处作用域结束，锁被释放
        }

        // 保存到数据库以持久化（在锁外部）
        if let Err(e) = self.database.insert_record(&record).await {
            eprintln!("Failed to save record to database: {}", e);
        }

        // 会话归属解析是独立于代理持久化的应用服务：数据库只接收已解析的
        // session_id，OpenCode 等需要模糊匹配的来源会在后续扫描阶段再对账。
        if let Err(error) = self
            .session_reconciliation
            .enqueue_after_ingest(self.database.clone(), record)
            .await
        {
            eprintln!("[collector] Failed to reconcile session stats: {error}");
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
    /// - "24h": 滑动窗口，当前时间往前推 24 小时
    /// - "today": 自然日，今天 00:00:00 到现在
    /// - "7d": 滑动窗口，当前时间往前推 7 天
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
                // 总 Token = 输入 + 缓存创建 + 缓存读取 + 输出
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
        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        crate::utils::business_time::business_window_cutoff_epoch(window, &settings)
            .saturating_mul(1000)
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
        for window in &["5h", "24h", "today", "7d", "30d", "current_month"] {
            result.insert(
                window.to_string(),
                self.get_window_stats(window, include_errors).await,
            );
        }
        result
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
    fn window_cutoff_sliding_windows_are_now_minus_duration() {
        // 注入显式 now 与显式 settings：不读真实 ~/.usagemeter/settings.json，
        // 不依赖墙钟，滑动窗口截止时间可精确断言（而非 ±1s 容差）。
        let settings = crate::models::AppSettings::default();
        let now = 1_700_000_000i64; // 固定参照点（2023-11-14 左右）

        for (window, hours) in [("5h", 5), ("24h", 24), ("7d", 7 * 24), ("30d", 30 * 24)] {
            let cutoff = crate::utils::business_time::business_window_cutoff_epoch_at(
                window, &settings, now,
            );
            assert_eq!(
                cutoff,
                now - hours * 3600,
                "{window} cutoff must be exactly now - {hours}h"
            );
        }
    }

    #[test]
    fn window_cutoff_business_windows_align_with_business_day_boundaries() {
        // today / current_month 依赖本地时区，但与业务日边界计算自洽：
        // 同一注入 now 下，截止时间必须等于业务日/业务月起点。
        let settings = crate::models::AppSettings::default();
        let now = 1_700_000_000i64;

        let today_cutoff =
            crate::utils::business_time::business_window_cutoff_epoch_at("today", &settings, now);
        let today_label = crate::utils::business_time::business_date_for_timestamp(now, &settings);
        let (today_start, _) =
            crate::utils::business_time::business_date_epoch_bounds(&today_label, &settings)
                .expect("today bounds");
        assert_eq!(
            today_cutoff, today_start,
            "today cutoff = business day start"
        );

        let month_cutoff = crate::utils::business_time::business_window_cutoff_epoch_at(
            "current_month",
            &settings,
            now,
        );
        // current_month 应等于"当前业务月 1 号的业务日边界"。
        let month_label = crate::utils::business_time::business_date_for_timestamp(now, &settings);
        let first_of_month = {
            let mut parts = month_label.split('-');
            let year: i32 = parts.next().unwrap().parse().unwrap();
            let month: u32 = parts.next().unwrap().parse().unwrap();
            format!("{year:04}-{month:02}-01")
        };
        let (month_start, _) =
            crate::utils::business_time::business_date_epoch_bounds(&first_of_month, &settings)
                .expect("month bounds");
        assert_eq!(
            month_cutoff, month_start,
            "current_month cutoff = month start"
        );
    }

    #[test]
    fn window_cutoff_ordering_holds_for_injected_now() {
        // 用注入 now 验证窗口顺序，不依赖墙钟与真实 settings。
        let settings = crate::models::AppSettings::default();
        let now = 1_700_000_000i64;

        let cutoff_5h =
            crate::utils::business_time::business_window_cutoff_epoch_at("5h", &settings, now);
        let cutoff_24h =
            crate::utils::business_time::business_window_cutoff_epoch_at("24h", &settings, now);
        let cutoff_7d =
            crate::utils::business_time::business_window_cutoff_epoch_at("7d", &settings, now);
        let cutoff_30d =
            crate::utils::business_time::business_window_cutoff_epoch_at("30d", &settings, now);
        let cutoff_today =
            crate::utils::business_time::business_window_cutoff_epoch_at("today", &settings, now);
        let cutoff_current_month = crate::utils::business_time::business_window_cutoff_epoch_at(
            "current_month",
            &settings,
            now,
        );

        // 滑动窗口严格递增：30d 最早，5h 最晚。
        assert!(cutoff_30d < cutoff_7d);
        assert!(cutoff_7d < cutoff_24h);
        assert!(cutoff_24h < cutoff_5h);
        // 所有截止时间都在注入 now 之前。
        for cutoff in [
            cutoff_5h,
            cutoff_24h,
            cutoff_today,
            cutoff_7d,
            cutoff_30d,
            cutoff_current_month,
        ] {
            assert!(cutoff < now, "cutoff {cutoff} must be in the past");
        }
    }

    #[tokio::test]
    async fn record_persists_full_fields_into_recent_cache() {
        let path = tempfile::tempdir().unwrap().path().join("collector.db");
        let collector = UsageCollector::with_database(Arc::new(
            ProxyDatabase::new_with_path(&path).expect("open temp db"),
        ));
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
            reasoning_tokens: 0,
            total_tokens: 330, // 总 Token = input(100) + cache_create(10) + cache_read(20) + output(200)
            model: "claude-sonnet-4".to_string(),
            session_id: Some("session-123".to_string()),
            request_start_time: now - 5000, // 5 秒前开始
            request_end_time: now,
            duration_ms: 5000,
            output_tokens_per_second: Some(40.0), // 200 tokens / 5 秒
            ttft_ms: Some(100),
            status_code: 200,
            estimated_cost: 0.0,
            pricing_snapshot_id: None,
            cost_locked: false,
            api_key_prefix: None,
            request_base_url: None,
            client_tool: crate::models::DEFAULT_CLIENT_TOOL.to_string(),
            proxy_profile_id: None,
            client_detection_method: crate::models::DEFAULT_CLIENT_DETECTION_METHOD.to_string(),
            ..Default::default()
        };

        collector.record(record.clone()).await;

        let recent = collector.recent_records.read().await;
        let stored = recent
            .iter()
            .find(|r| r.message_id == "test-msg")
            .expect("recorded record must be visible in recent cache");
        assert_eq!(stored.message_id, "test-msg");
        assert_eq!(stored.total_tokens, 330); // input(100) + cache_create(10) + cache_read(20) + output(200)
        assert_eq!(stored.duration_ms, 5000);
        assert_eq!(stored.output_tokens_per_second, Some(40.0));
        assert_eq!(stored.model, "claude-sonnet-4");
        assert_eq!(stored.session_id.as_deref(), Some("session-123"));
    }

    #[tokio::test]
    async fn recent_duplicate_replaces_cached_snapshot() {
        let path = tempfile::tempdir().unwrap().path().join("collector.db");
        let collector = UsageCollector::with_database(Arc::new(
            ProxyDatabase::new_with_path(&path).expect("open temp db"),
        ));

        let base_time = 1_700_000_000_000i64;
        let first = UsageRecord {
            timestamp: base_time,
            message_id: "dup-msg".to_string(),
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            client_tool: "codex".to_string(),
            ..Default::default()
        };
        let updated = UsageRecord {
            timestamp: base_time + 1,
            message_id: "dup-msg".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            client_tool: "codex".to_string(),
            ..Default::default()
        };

        collector.record(first).await;
        collector.record(updated.clone()).await;

        let recent = collector.recent_records.read().await;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].total_tokens, updated.total_tokens);
        assert_eq!(recent[0].output_tokens, updated.output_tokens);
    }
}
