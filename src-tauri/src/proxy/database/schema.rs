use super::ProxyDatabase;
use crate::models::ModelPricingConfig;
use rusqlite::{params, Connection};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

impl ProxyDatabase {
    pub fn create_tables(conn: &Connection) -> Result<(), String> {
        let needs_rebuild = Self::check_session_stats_needs_rebuild(conn);

        if needs_rebuild {
            eprintln!("[database] Rebuilding session_stats table due to schema change");
            conn.execute("DROP TABLE IF EXISTS session_stats", [])
                .map_err(|e| format!("Failed to drop old session_stats table: {}", e))?;
        }

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                message_id TEXT NOT NULL,
                storage_dedupe_key TEXT NOT NULL UNIQUE,
                canonical_request_key TEXT,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                model TEXT NOT NULL DEFAULT '',
                session_id TEXT,
                session_resolution_state TEXT NOT NULL DEFAULT 'unknown',
                message_id_conflicted INTEGER NOT NULL DEFAULT 0,
                request_start_time INTEGER,
                request_end_time INTEGER,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                output_tokens_per_second REAL,
                ttft_ms INTEGER,
                status_code INTEGER NOT NULL DEFAULT 200,
                migration_attempted_at INTEGER,
                estimated_cost REAL NOT NULL DEFAULT 0,
                pricing_snapshot_id TEXT,
                cost_locked INTEGER NOT NULL DEFAULT 0,
                api_key_prefix TEXT,
                request_base_url TEXT,
                client_tool TEXT NOT NULL DEFAULT 'claude_code',
                proxy_profile_id TEXT,
                client_detection_method TEXT NOT NULL DEFAULT 'legacy_path',
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            );

            CREATE INDEX IF NOT EXISTS idx_timestamp ON usage_records(timestamp);
            CREATE INDEX IF NOT EXISTS idx_message_id ON usage_records(message_id);
            CREATE INDEX IF NOT EXISTS idx_session_id ON usage_records(session_id);
            CREATE INDEX IF NOT EXISTS idx_model_timestamp ON usage_records(model, timestamp);

            CREATE TABLE IF NOT EXISTS session_stats (
                session_id TEXT PRIMARY KEY,
                total_duration_ms INTEGER NOT NULL DEFAULT 0,
                avg_output_tokens_per_second REAL NOT NULL DEFAULT 0,
                avg_ttft_ms REAL NOT NULL DEFAULT 0,
                proxy_request_count INTEGER NOT NULL DEFAULT 0,
                success_requests INTEGER NOT NULL DEFAULT 0,
                error_requests INTEGER NOT NULL DEFAULT 0,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                models TEXT,
                first_request_time INTEGER,
                last_request_time INTEGER,
                estimated_cost REAL NOT NULL DEFAULT 0,
                last_updated INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_session_stats_updated ON session_stats(last_updated);

            CREATE TABLE IF NOT EXISTS daily_summary (
                date TEXT PRIMARY KEY,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                cost REAL NOT NULL DEFAULT 0,
                success_total_tokens INTEGER NOT NULL DEFAULT 0,
                success_input_tokens INTEGER NOT NULL DEFAULT 0,
                success_output_tokens INTEGER NOT NULL DEFAULT 0,
                success_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                success_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                success_cost REAL NOT NULL DEFAULT 0,
                model_count INTEGER NOT NULL DEFAULT 0,
                success_requests INTEGER NOT NULL DEFAULT 0,
                client_error_requests INTEGER NOT NULL DEFAULT 0,
                server_error_requests INTEGER NOT NULL DEFAULT 0,
                finalized_at INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS daily_rollup_state (
                state_key TEXT PRIMARY KEY,
                state_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS model_usage (
                date TEXT NOT NULL,
                model TEXT NOT NULL,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                cost REAL NOT NULL DEFAULT 0,
                success_requests INTEGER NOT NULL DEFAULT 0,
                client_error_requests INTEGER NOT NULL DEFAULT 0,
                server_error_requests INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, model)
            );
            "#,
        )
        .map_err(|e| format!("Failed to create tables: {}", e))?;

        Ok(())
    }

    fn check_session_stats_needs_rebuild(conn: &Connection) -> bool {
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('session_stats')")
            .and_then(|mut stmt| {
                let mut cols = Vec::new();
                let rows = stmt.query_map([], |row| row.get(0))?;
                for row in rows {
                    cols.push(row?);
                }
                Ok(cols)
            })
            .unwrap_or_default();

        let required_columns = [
            "proxy_request_count",
            "success_requests",
            "error_requests",
            "avg_ttft_ms",
            "last_updated",
        ];

        for col in &required_columns {
            if !columns.iter().any(|c| c == col) {
                eprintln!("[database] session_stats missing column: {}", col);
                return true;
            }
        }

        false
    }

    fn usage_records_has_storage_dedupe_unique(conn: &Connection) -> bool {
        let mut stmt = match conn.prepare("PRAGMA index_list('usage_records')") {
            Ok(stmt) => stmt,
            Err(_) => return false,
        };
        let rows = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2).unwrap_or(0) != 0,
            ))
        }) {
            Ok(rows) => rows,
            Err(_) => return false,
        };

        for row in rows.flatten() {
            if !row.1 {
                continue;
            }
            let mut info_stmt = match conn.prepare(&format!("PRAGMA index_info('{}')", row.0)) {
                Ok(stmt) => stmt,
                Err(_) => continue,
            };
            let cols = match info_stmt.query_map([], |info_row| info_row.get::<_, String>(2)) {
                Ok(cols) => cols,
                Err(_) => continue,
            };
            let col_names: Vec<String> = cols.flatten().collect();
            if col_names.len() == 1 && col_names[0] == "storage_dedupe_key" {
                return true;
            }
        }
        false
    }

    pub(super) fn create_model_pricing_table_static(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS model_pricing (
                model_id TEXT PRIMARY KEY,
                display_name TEXT,
                input_price REAL NOT NULL,
                output_price REAL NOT NULL,
                cache_read_price REAL,
                cache_write_price REAL,
                source TEXT NOT NULL DEFAULT 'api',
                last_updated INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_model_pricing_search ON model_pricing(model_id, display_name);
            "#,
        )
        .map_err(|e| format!("Failed to create model_pricing table: {}", e))?;
        Ok(())
    }

    pub(super) fn migrate_schema(conn: &Connection) -> Result<(), String> {
        let migrations = [
            "ALTER TABLE usage_records ADD COLUMN storage_dedupe_key TEXT",
            "ALTER TABLE usage_records ADD COLUMN canonical_request_key TEXT",
            "ALTER TABLE usage_records ADD COLUMN request_start_time INTEGER",
            "ALTER TABLE usage_records ADD COLUMN request_end_time INTEGER",
            "ALTER TABLE usage_records ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN output_tokens_per_second REAL",
            "ALTER TABLE usage_records ADD COLUMN status_code INTEGER NOT NULL DEFAULT 200",
            "ALTER TABLE usage_records ADD COLUMN ttft_ms INTEGER",
            "ALTER TABLE usage_records ADD COLUMN migration_attempted_at INTEGER",
            "ALTER TABLE usage_records ADD COLUMN session_resolution_state TEXT NOT NULL DEFAULT 'unknown'",
            "ALTER TABLE usage_records ADD COLUMN message_id_conflicted INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN estimated_cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN pricing_snapshot_id TEXT",
            "ALTER TABLE usage_records ADD COLUMN cost_locked INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE usage_records ADD COLUMN api_key_prefix TEXT",
            "ALTER TABLE usage_records ADD COLUMN request_base_url TEXT",
            "ALTER TABLE usage_records ADD COLUMN client_tool TEXT NOT NULL DEFAULT 'claude_code'",
            "ALTER TABLE usage_records ADD COLUMN proxy_profile_id TEXT",
            "ALTER TABLE usage_records ADD COLUMN client_detection_method TEXT NOT NULL DEFAULT 'legacy_path'",
            "ALTER TABLE daily_summary ADD COLUMN cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_total_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_input_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_output_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_cache_create_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_cache_read_tokens INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN model_count INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN success_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN client_error_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN server_error_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE daily_summary ADD COLUMN finalized_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE model_usage ADD COLUMN cost REAL NOT NULL DEFAULT 0",
            "ALTER TABLE model_usage ADD COLUMN success_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE model_usage ADD COLUMN client_error_requests INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE model_usage ADD COLUMN server_error_requests INTEGER NOT NULL DEFAULT 0",
        ];

        for migration in migrations {
            let _ = conn.execute(migration, []);
        }

        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_source_lookup ON usage_records(api_key_prefix, request_base_url)",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_usage_tool_source ON usage_records(client_tool, api_key_prefix, request_base_url)",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_usage_tool_time ON usage_records(client_tool, timestamp)",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_usage_canonical_key ON usage_records(canonical_request_key)",
            [],
        );
        let _ = conn.execute(
            "INSERT INTO daily_rollup_state (state_key, state_value, updated_at)
             VALUES ('day_boundary_mode', 'standard', strftime('%s', 'now'))
             ON CONFLICT(state_key) DO NOTHING",
            [],
        );

        if !Self::usage_records_has_storage_dedupe_unique(conn) {
            Self::rebuild_usage_records_table(conn)?;
        }
        Self::normalize_legacy_opencode_native_session_ids(conn)?;

        Ok(())
    }

    fn normalize_legacy_opencode_native_session_ids(conn: &Connection) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start OpenCode session id normalization: {}", e))?;

        let updated_records = tx
            .execute(
                r#"
                UPDATE usage_records
                SET session_id = 'opencode::native::' || substr(session_id, length('opencode::') + 1),
                    updated_at = ?1
                WHERE COALESCE(client_tool, '') = 'opencode'
                  AND session_id LIKE 'opencode::%'
                  AND session_id NOT LIKE 'opencode::native::%'
                  AND session_id NOT LIKE 'opencode::wsl:%::%'
                "#,
                params![now],
            )
            .map_err(|e| format!("Failed to normalize OpenCode session ids: {}", e))?;

        let updated_keys = tx
            .execute(
                r#"
                UPDATE usage_records
                SET canonical_request_key = 'opencode:opencode::native::'
                    || substr(canonical_request_key, length('opencode:opencode::') + 1),
                    updated_at = ?1
                WHERE COALESCE(client_tool, '') = 'opencode'
                  AND canonical_request_key LIKE 'opencode:opencode::%|%'
                  AND canonical_request_key NOT LIKE 'opencode:opencode::native::%|%'
                  AND canonical_request_key NOT LIKE 'opencode:opencode::wsl:%::%|%'
                "#,
                params![now],
            )
            .map_err(|e| format!("Failed to normalize OpenCode canonical request keys: {}", e))?;

        if updated_records > 0 || updated_keys > 0 {
            Self::rebuild_opencode_session_stats_after_session_id_normalization(&tx, now)?;
        }

        tx.commit()
            .map_err(|e| format!("Failed to commit OpenCode session id normalization: {}", e))?;
        Ok(())
    }

    fn rebuild_opencode_session_stats_after_session_id_normalization(
        tx: &rusqlite::Transaction<'_>,
        now: i64,
    ) -> Result<(), String> {
        tx.execute(
            "DELETE FROM session_stats WHERE session_id LIKE 'opencode::%'",
            [],
        )
        .map_err(|e| format!("Failed to clear OpenCode session stats: {}", e))?;

        tx.execute(
            r#"
            INSERT INTO session_stats (
                session_id, total_duration_ms, avg_output_tokens_per_second, avg_ttft_ms,
                proxy_request_count, success_requests, error_requests,
                total_input_tokens, total_output_tokens, total_cache_create_tokens,
                total_cache_read_tokens, models, first_request_time, last_request_time,
                estimated_cost, last_updated
            )
            SELECT
                session_id,
                COALESCE(SUM(duration_ms), 0),
                CASE
                    WHEN COALESCE(SUM(duration_ms), 0) > 0
                    THEN COALESCE(SUM(output_tokens), 0) * 1000.0 / COALESCE(SUM(duration_ms), 0)
                    ELSE 0
                END,
                COALESCE(AVG(ttft_ms), 0),
                COUNT(*),
                COALESCE(SUM(CASE WHEN status_code < 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_create_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                GROUP_CONCAT(DISTINCT model),
                MIN(request_start_time),
                MAX(request_end_time),
                COALESCE(SUM(estimated_cost), 0),
                ?1
            FROM usage_records
            WHERE COALESCE(client_tool, '') = 'opencode'
              AND session_id IS NOT NULL
              AND session_id != ''
              AND session_id != ?2
            GROUP BY session_id
            "#,
            params![now, super::LEGACY_UNMATCHED_SESSION_ID],
        )
        .map_err(|e| format!("Failed to rebuild OpenCode session stats: {}", e))?;
        Ok(())
    }

    fn rebuild_usage_records_table(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS usage_records_v2 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                message_id TEXT NOT NULL,
                storage_dedupe_key TEXT NOT NULL UNIQUE,
                canonical_request_key TEXT,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                model TEXT NOT NULL DEFAULT '',
                session_id TEXT,
                session_resolution_state TEXT NOT NULL DEFAULT 'unknown',
                message_id_conflicted INTEGER NOT NULL DEFAULT 0,
                request_start_time INTEGER,
                request_end_time INTEGER,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                output_tokens_per_second REAL,
                ttft_ms INTEGER,
                status_code INTEGER NOT NULL DEFAULT 200,
                migration_attempted_at INTEGER,
                estimated_cost REAL NOT NULL DEFAULT 0,
                pricing_snapshot_id TEXT,
                cost_locked INTEGER NOT NULL DEFAULT 0,
                api_key_prefix TEXT,
                request_base_url TEXT,
                client_tool TEXT NOT NULL DEFAULT 'claude_code',
                proxy_profile_id TEXT,
                client_detection_method TEXT NOT NULL DEFAULT 'legacy_path',
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            );
            INSERT INTO usage_records_v2 (
                id, timestamp, message_id, storage_dedupe_key, canonical_request_key,
                input_tokens, output_tokens, cache_create_tokens, cache_read_tokens, model,
                session_id, session_resolution_state, message_id_conflicted,
                request_start_time, request_end_time, duration_ms, output_tokens_per_second,
                ttft_ms, status_code, migration_attempted_at, estimated_cost, pricing_snapshot_id,
                cost_locked, api_key_prefix, request_base_url, client_tool, proxy_profile_id,
                client_detection_method, created_at, updated_at
            )
            SELECT
                id,
                timestamp,
                message_id,
                CASE
                    WHEN COALESCE(storage_dedupe_key, '') != '' THEN storage_dedupe_key
                    WHEN COALESCE(client_tool, 'claude_code') = 'opencode' AND COALESCE(message_id, '') != ''
                        THEN COALESCE(client_tool, 'claude_code') || ':' || message_id || ':' || COALESCE(NULLIF(request_start_time, 0), timestamp)
                    WHEN COALESCE(message_id, '') != ''
                        THEN COALESCE(client_tool, 'claude_code') || ':' || message_id
                    ELSE COALESCE(client_tool, 'claude_code') || ':' || COALESCE(session_id, '') || ':' || timestamp || ':' || model || ':' ||
                         input_tokens || ':' || output_tokens || ':' || cache_create_tokens || ':' || cache_read_tokens
                END,
                CASE
                    WHEN COALESCE(canonical_request_key, '') != '' THEN canonical_request_key
                    WHEN COALESCE(message_id, '') != ''
                        THEN COALESCE(client_tool, 'claude_code') || ':' || message_id
                    ELSE COALESCE(client_tool, 'claude_code') || ':' || COALESCE(session_id, '') || ':' || timestamp || ':' || model || ':' ||
                         input_tokens || ':' || output_tokens || ':' || cache_create_tokens || ':' || cache_read_tokens
                END,
                input_tokens,
                output_tokens,
                cache_create_tokens,
                cache_read_tokens,
                model,
                session_id,
                CASE
                    WHEN COALESCE(session_resolution_state, '') != '' THEN session_resolution_state
                    WHEN COALESCE(client_tool, 'claude_code') = 'opencode' AND (session_id IS NULL OR session_id = '') THEN 'unknown'
                    WHEN session_id IS NULL OR session_id = '' THEN 'unknown'
                    ELSE 'known'
                END,
                COALESCE(message_id_conflicted, 0),
                request_start_time,
                request_end_time,
                duration_ms,
                output_tokens_per_second,
                ttft_ms,
                status_code,
                migration_attempted_at,
                estimated_cost,
                pricing_snapshot_id,
                cost_locked,
                api_key_prefix,
                request_base_url,
                COALESCE(client_tool, 'claude_code'),
                proxy_profile_id,
                COALESCE(client_detection_method, 'legacy_path'),
                created_at,
                updated_at
            FROM usage_records;
            DROP TABLE usage_records;
            ALTER TABLE usage_records_v2 RENAME TO usage_records;
            CREATE INDEX IF NOT EXISTS idx_timestamp ON usage_records(timestamp);
            CREATE INDEX IF NOT EXISTS idx_message_id ON usage_records(message_id);
            CREATE INDEX IF NOT EXISTS idx_usage_storage_key ON usage_records(storage_dedupe_key);
            CREATE INDEX IF NOT EXISTS idx_usage_canonical_key ON usage_records(canonical_request_key);
            CREATE INDEX IF NOT EXISTS idx_session_id ON usage_records(session_id);
            CREATE INDEX IF NOT EXISTS idx_model_timestamp ON usage_records(model, timestamp);
            CREATE INDEX IF NOT EXISTS idx_source_lookup ON usage_records(api_key_prefix, request_base_url);
            CREATE INDEX IF NOT EXISTS idx_usage_tool_source ON usage_records(client_tool, api_key_prefix, request_base_url);
            CREATE INDEX IF NOT EXISTS idx_usage_tool_time ON usage_records(client_tool, timestamp);
            COMMIT;
            "#,
        )
        .map_err(|e| format!("Failed to rebuild usage_records table: {}", e))
    }

    pub(super) fn pricing_snapshot_id(pricings: &[ModelPricingConfig], match_mode: &str) -> String {
        let mut normalized = pricings.to_vec();
        normalized.sort_by(|a, b| {
            a.model_id
                .cmp(&b.model_id)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.last_updated.cmp(&b.last_updated))
        });
        let payload = serde_json::json!({
            "matchMode": match_mode,
            "pricings": normalized,
        });
        let mut hasher = DefaultHasher::new();
        payload.to_string().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    pub(super) fn record_local_date(timestamp_ms: i64) -> String {
        let settings = crate::commands::load_settings().unwrap_or_default();
        Self::record_local_date_with_settings(timestamp_ms, &settings)
    }

    pub(super) fn today_local_date() -> String {
        let settings = crate::commands::load_settings().unwrap_or_default();
        Self::today_local_date_with_settings(&settings)
    }

    pub(super) fn record_local_date_with_settings(
        timestamp_ms: i64,
        settings: &crate::models::AppSettings,
    ) -> String {
        crate::utils::business_time::business_date_for_timestamp_ms(timestamp_ms, settings)
    }

    pub(super) fn today_local_date_with_settings(settings: &crate::models::AppSettings) -> String {
        crate::utils::business_time::current_business_date(settings)
    }

    pub(super) fn current_day_boundary_mode() -> String {
        let settings = crate::commands::load_settings().unwrap_or_default();
        crate::utils::business_time::normalize_day_boundary_mode(&settings.day_boundary_mode)
    }
}
