//! 会话面板查询性能测试
//!
//! 运行：cargo test --package usagemeter --lib --features performance-tests unified_usage::performance_tests -- --nocapture --test-threads=1
//!
//! 这些测试读取当前用户的 UsageMeter 数据库，并可能触发 migration 或派生物化写入；
//! 默认测试套件不编译本模块，必须显式启用 `performance-tests` feature。
//!
//! 测试目标：
//! - 最近会话查询 (get_merged_sessions_no_sync)
//! - 最近请求查询 (get_merged_request_facts_no_sync)
//! - 项目统计查询 (get_merged_project_stats_no_sync)
//!
//! 性能阈值（仅观测告警，不作为跨机器/跨数据规模的硬性断言，与
//! commands/usage/performance_tests.rs 的既有设计一致）：
//! - 冷启动首次查询：< 2000ms（警告级别，不会失败）
//! - 热缓存二次查询：< 100ms（警告级别，超过仅打印 WARNING）
//! - 签名变更后重查：< 500ms（警告级别）

#[cfg(test)]
mod tests {
    use std::time::Instant;

    #[tokio::test]
    async fn test_sessions_query_performance() {
        println!("\n=== 最近会话查询性能测试 ===\n");

        let settings = crate::settings::load_settings_blocking().unwrap_or_default();

        // 1. 首次查询最近会话（冷启动）
        let t = Instant::now();
        let sessions_cold = crate::unified_usage::get_merged_sessions_no_sync(&settings, 20, 0)
            .await
            .expect("get_merged_sessions_no_sync cold");
        let time_cold = t.elapsed().as_millis();

        println!(
            "✓ get_merged_sessions_no_sync (COLD): {} ms, {} sessions",
            time_cold,
            sessions_cold.len()
        );

        if time_cold > 2000 {
            eprintln!(
                "⚠️  WARNING: Cold query exceeded 2000ms threshold ({}ms)",
                time_cold
            );
        }

        // 2. 热缓存二次查询
        let t = Instant::now();
        let sessions_warm = crate::unified_usage::get_merged_sessions_no_sync(&settings, 20, 0)
            .await
            .expect("get_merged_sessions_no_sync warm");
        let time_warm = t.elapsed().as_millis();

        println!(
            "✓ get_merged_sessions_no_sync (WARM): {} ms, {} sessions",
            time_warm,
            sessions_warm.len()
        );

        if time_warm > 100 {
            eprintln!(
                "⚠️  WARNING: Warm query exceeded 100ms observational target ({}ms)",
                time_warm
            );
        }

        // 验证数据一致性（功能断言，保留）
        assert_eq!(
            sessions_cold.len(),
            sessions_warm.len(),
            "Session count mismatch between cold and warm"
        );
    }

    #[tokio::test]
    async fn test_recent_requests_query_performance() {
        println!("\n=== 最近请求查询性能测试 ===\n");

        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let include_errors = settings.proxy.include_error_requests;

        // 1. 冷启动查询最近请求（完整链路：merge + sort + page）
        let t = Instant::now();
        let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
            &settings,
            None,
            None,
            include_errors,
        )
        .await
        .expect("get_merged_request_facts cold");

        // 排序和分页（与 requests.rs 一致）
        let mut ordered: Vec<_> = facts.iter().collect();
        ordered.sort_by_key(|fact| std::cmp::Reverse(fact.timestamp_ms));
        let total_count = ordered.len();
        let page_count = ordered.into_iter().skip(0).take(50).count();
        let time_cold = t.elapsed().as_millis();

        println!(
            "✓ get_merged_request_facts + sort/page (COLD): {} ms, {} total, {} in page",
            time_cold, total_count, page_count
        );

        if time_cold > 2000 {
            eprintln!(
                "⚠️  WARNING: Cold query exceeded 2000ms threshold ({}ms)",
                time_cold
            );
        }

        // 2. 热缓存查询
        let t = Instant::now();
        let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
            &settings,
            None,
            None,
            include_errors,
        )
        .await
        .expect("get_merged_request_facts warm");

        let mut ordered: Vec<_> = facts.iter().collect();
        ordered.sort_by_key(|fact| std::cmp::Reverse(fact.timestamp_ms));
        let total_count_warm = ordered.len();
        let page_count_warm = ordered.into_iter().skip(0).take(50).count();
        let time_warm = t.elapsed().as_millis();

        println!(
            "✓ get_merged_request_facts + sort/page (WARM): {} ms, {} total, {} in page",
            time_warm, total_count_warm, page_count_warm
        );

        if time_warm > 100 {
            eprintln!(
                "⚠️  WARNING: Warm query exceeded 100ms observational target ({}ms)",
                time_warm
            );
        }

        assert_eq!(
            total_count, total_count_warm,
            "Request count mismatch between cold and warm"
        );
    }

    #[tokio::test]
    async fn test_project_stats_query_performance() {
        println!("\n=== 项目统计查询性能测试 ===\n");

        let settings = crate::settings::load_settings_blocking().unwrap_or_default();

        // 1. 冷启动查询项目统计
        let t = Instant::now();
        let projects_cold = crate::unified_usage::get_merged_project_stats_no_sync(&settings)
            .await
            .expect("get_merged_project_stats_no_sync cold");
        let time_cold = t.elapsed().as_millis();

        println!(
            "✓ get_merged_project_stats_no_sync (COLD): {} ms, {} projects",
            time_cold,
            projects_cold.len()
        );

        if time_cold > 2000 {
            eprintln!(
                "⚠️  WARNING: Cold query exceeded 2000ms threshold ({}ms)",
                time_cold
            );
        }

        // 2. 热缓存查询
        let t = Instant::now();
        let projects_warm = crate::unified_usage::get_merged_project_stats_no_sync(&settings)
            .await
            .expect("get_merged_project_stats_no_sync warm");
        let time_warm = t.elapsed().as_millis();

        println!(
            "✓ get_merged_project_stats_no_sync (WARM): {} ms, {} projects",
            time_warm,
            projects_warm.len()
        );

        if time_warm > 100 {
            eprintln!(
                "⚠️  WARNING: Warm query exceeded 100ms observational target ({}ms)",
                time_warm
            );
        }

        assert_eq!(
            projects_cold.len(),
            projects_warm.len(),
            "Project count mismatch"
        );
    }

    #[tokio::test]
    async fn test_full_panel_load_sequence() {
        println!("\n=== 模拟用户打开会话面板的完整加载流程 ===\n");

        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let include_errors = settings.proxy.include_error_requests;

        // 模拟前端依次发起三个查询
        let total_start = Instant::now();

        let t = Instant::now();
        let sessions = crate::unified_usage::get_merged_sessions_no_sync(&settings, 20, 0)
            .await
            .expect("sessions");
        let time_sessions = t.elapsed().as_millis();
        println!("1. 最近会话: {} ms", time_sessions);

        let t = Instant::now();
        let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
            &settings,
            None,
            None,
            include_errors,
        )
        .await
        .expect("requests");
        let mut ordered: Vec<_> = facts.iter().collect();
        ordered.sort_by_key(|fact| std::cmp::Reverse(fact.timestamp_ms));
        let requests_count = ordered.into_iter().skip(0).take(50).count();
        let time_requests = t.elapsed().as_millis();
        println!("2. 最近请求: {} ms", time_requests);

        let t = Instant::now();
        let projects = crate::unified_usage::get_merged_project_stats_no_sync(&settings)
            .await
            .expect("projects");
        let time_projects = t.elapsed().as_millis();
        println!("3. 项目统计: {} ms", time_projects);

        let total_time = total_start.elapsed().as_millis();
        println!("\n总耗时: {} ms", total_time);
        println!(
            "数据: {} sessions, {} requests, {} projects",
            sessions.len(),
            requests_count,
            projects.len()
        );

        if total_time > 3000 {
            eprintln!(
                "⚠️  WARNING: Total panel load time exceeded 3000ms ({}ms)",
                total_time
            );
        }

        println!("\n=== 二次查询（热缓存）===\n");

        let warm_start = Instant::now();

        let t = Instant::now();
        let _ = crate::unified_usage::get_merged_sessions_no_sync(&settings, 20, 0)
            .await
            .expect("sessions warm");
        let time_sessions_warm = t.elapsed().as_millis();
        println!("1. 最近会话: {} ms", time_sessions_warm);

        let t = Instant::now();
        let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
            &settings,
            None,
            None,
            include_errors,
        )
        .await
        .expect("requests warm");
        let mut ordered: Vec<_> = facts.iter().collect();
        ordered.sort_by_key(|fact| std::cmp::Reverse(fact.timestamp_ms));
        let _ = ordered.into_iter().skip(0).take(50).count();
        let time_requests_warm = t.elapsed().as_millis();
        println!("2. 最近请求: {} ms", time_requests_warm);

        let t = Instant::now();
        let _ = crate::unified_usage::get_merged_project_stats_no_sync(&settings)
            .await
            .expect("projects warm");
        let time_projects_warm = t.elapsed().as_millis();
        println!("3. 项目统计: {} ms", time_projects_warm);

        let warm_total = warm_start.elapsed().as_millis();
        println!("\n热缓存总耗时: {} ms\n", warm_total);

        if warm_total > 300 {
            eprintln!(
                "⚠️  WARNING: Warm cache total exceeded 300ms observational target ({}ms)",
                warm_total
            );
        }
    }

    #[test]
    fn test_sync_overhead() {
        println!("\n=== 同步扫描开销测试 ===\n");

        let t = Instant::now();
        let _ = crate::local_usage::ensure_local_usage_synced();
        let time_sync1 = t.elapsed().as_millis();
        println!("首次 ensure_local_usage_synced: {} ms", time_sync1);

        if time_sync1 > 2000 {
            eprintln!(
                "⚠️  WARNING: Initial sync exceeded 2000ms ({}ms)",
                time_sync1
            );
        }

        let t = Instant::now();
        let _ = crate::local_usage::ensure_local_usage_synced();
        let time_sync2 = t.elapsed().as_millis();
        println!("节流期内二次同步: {} ms", time_sync2);

        if time_sync2 > 10 {
            eprintln!(
                "⚠️  WARNING: Throttled sync exceeded 10ms observational target ({}ms)",
                time_sync2
            );
        }

        println!();
    }

    #[tokio::test]
    async fn test_detailed_breakdown() {
        println!("\n=== 查询链路详细分解 ===\n");

        let settings = crate::settings::load_settings_blocking().unwrap_or_default();
        let include_errors = settings.proxy.include_error_requests;

        // 1. 同步开销
        let t = Instant::now();
        let _ = crate::local_usage::ensure_local_usage_synced();
        let time_sync = t.elapsed().as_millis();
        println!("Step 1 - ensure_local_usage_synced: {} ms", time_sync);

        // 2. 获取全量合并事实（无派生聚合）
        let t = Instant::now();
        let (facts, _) = crate::unified_usage::get_merged_request_facts_no_sync(
            &settings,
            None,
            None,
            include_errors,
        )
        .await
        .expect("get_merged_request_facts_no_sync");
        let time_merge = t.elapsed().as_millis();
        println!(
            "Step 2 - get_merged_request_facts_no_sync: {} ms ({} facts)",
            time_merge,
            facts.len()
        );

        // 3. 排序开销（最近请求查询需要）
        let t = Instant::now();
        let mut ordered: Vec<_> = facts.iter().collect();
        ordered.sort_by_key(|fact| std::cmp::Reverse(fact.timestamp_ms));
        let _ = ordered.into_iter().take(50).count();
        let time_sort = t.elapsed().as_millis();
        println!("Step 3 - sort + paginate (top 50): {} ms", time_sort);

        // 4. 聚合会话（从已缓存的 facts）
        let t = Instant::now();
        let sessions = crate::unified_usage::get_merged_sessions_no_sync(&settings, 20, 0)
            .await
            .expect("get_merged_sessions_no_sync");
        let time_sessions = t.elapsed().as_millis();
        println!(
            "Step 4 - derive sessions from cache: {} ms ({} sessions)",
            time_sessions,
            sessions.len()
        );

        // 5. 聚合项目（从已缓存的 facts）
        let t = Instant::now();
        let projects = crate::unified_usage::get_merged_project_stats_no_sync(&settings)
            .await
            .expect("get_merged_project_stats_no_sync");
        let time_projects = t.elapsed().as_millis();
        println!(
            "Step 5 - derive projects from cache: {} ms ({} projects)",
            time_projects,
            projects.len()
        );

        println!(
            "\n总计: {} ms (sync:{} + merge:{} + sort:{} + sessions:{} + projects:{})\n",
            time_sync + time_merge + time_sort + time_sessions + time_projects,
            time_sync,
            time_merge,
            time_sort,
            time_sessions,
            time_projects
        );
    }
}
