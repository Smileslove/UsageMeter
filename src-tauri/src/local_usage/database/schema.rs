use rusqlite::{params, Connection};

use super::LocalUsageDatabase;

impl LocalUsageDatabase {
    pub(super) fn create_tables(conn: &Connection) -> Result<(), String> {
        Self::create_cache_tables(conn)?;
        Self::create_sync_v2_tables(conn)?;
        Self::create_unified_materialized_tables(conn)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS local_sync_state (
                state_key TEXT PRIMARY KEY,
                state_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("Failed to create local usage sync state table: {}", e))?;

        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO local_sync_state (state_key, state_value, updated_at)
             VALUES ('schema_version', '1', ?1)
             ON CONFLICT(state_key) DO NOTHING",
            params![now],
        )
        .map_err(|e| format!("Failed to initialize local usage schema state: {}", e))?;

        Ok(())
    }

    pub(super) fn create_unified_materialized_tables(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS unified_daily_materialized_facts (
                local_date TEXT NOT NULL,
                request_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                project_name TEXT,
                project_path TEXT,
                api_key_prefix TEXT,
                request_base_url TEXT,
                tool TEXT NOT NULL,
                timestamp_sec INTEGER NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 1,
                estimated_cost REAL NOT NULL DEFAULT 0,
                coverage_origin TEXT NOT NULL,
                status_code INTEGER,
                duration_ms INTEGER,
                output_tokens_per_second REAL,
                ttft_ms INTEGER,
                source_label TEXT,
                PRIMARY KEY(local_date, request_key)
            );
            CREATE INDEX IF NOT EXISTS idx_unified_daily_materialized_facts_date_timestamp
                ON unified_daily_materialized_facts(local_date, timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_unified_daily_materialized_facts_date_tool
                ON unified_daily_materialized_facts(local_date, tool);
            CREATE INDEX IF NOT EXISTS idx_unified_daily_materialized_facts_date_session
                ON unified_daily_materialized_facts(local_date, session_id);

            CREATE TABLE IF NOT EXISTS unified_daily_materialization_state (
                local_date TEXT PRIMARY KEY,
                day_boundary_mode TEXT NOT NULL DEFAULT 'standard',
                fact_count INTEGER NOT NULL DEFAULT 0,
                local_request_count INTEGER NOT NULL DEFAULT 0,
                local_max_sync_version INTEGER NOT NULL DEFAULT 0,
                local_max_timestamp INTEGER NOT NULL DEFAULT 0,
                remote_request_count INTEGER NOT NULL DEFAULT 0,
                remote_max_export_seq INTEGER NOT NULL DEFAULT 0,
                remote_max_timestamp INTEGER NOT NULL DEFAULT 0,
                proxy_record_count INTEGER NOT NULL DEFAULT 0,
                proxy_all_record_count INTEGER NOT NULL DEFAULT 0,
                proxy_max_timestamp_ms INTEGER NOT NULL DEFAULT 0,
                proxy_max_updated_at INTEGER NOT NULL DEFAULT 0,
                max_fact_timestamp_ms INTEGER NOT NULL DEFAULT 0,
                pricing_fingerprint INTEGER NOT NULL DEFAULT 0,
                is_finalized INTEGER NOT NULL DEFAULT 0,
                finalized_at INTEGER,
                materialized_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS unified_daily_summary (
                local_date TEXT PRIMARY KEY,
                request_count INTEGER NOT NULL DEFAULT 0,
                visible_request_count INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                visible_total_tokens INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                visible_input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                visible_output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                visible_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                visible_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_cost REAL NOT NULL DEFAULT 0,
                visible_cost REAL NOT NULL DEFAULT 0,
                success_request_count INTEGER NOT NULL DEFAULT 0,
                success_total_tokens INTEGER NOT NULL DEFAULT 0,
                success_input_tokens INTEGER NOT NULL DEFAULT 0,
                success_output_tokens INTEGER NOT NULL DEFAULT 0,
                success_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                success_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                success_cost REAL NOT NULL DEFAULT 0,
                client_error_requests INTEGER NOT NULL DEFAULT 0,
                server_error_requests INTEGER NOT NULL DEFAULT 0,
                model_count INTEGER NOT NULL DEFAULT 0,
                success_model_count INTEGER NOT NULL DEFAULT 0,
                proxy_backed_requests INTEGER NOT NULL DEFAULT 0,
                local_only_requests INTEGER NOT NULL DEFAULT 0,
                merged_overlap_requests INTEGER NOT NULL DEFAULT 0,
                has_partial_status_coverage INTEGER NOT NULL DEFAULT 0,
                has_partial_performance_coverage INTEGER NOT NULL DEFAULT 0,
                materialized_at INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS unified_daily_model_summary (
                local_date TEXT NOT NULL,
                model_name TEXT NOT NULL,
                request_count INTEGER NOT NULL DEFAULT 0,
                visible_request_count INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                visible_total_tokens INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                visible_input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                visible_output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                visible_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                visible_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_cost REAL NOT NULL DEFAULT 0,
                visible_cost REAL NOT NULL DEFAULT 0,
                success_request_count INTEGER NOT NULL DEFAULT 0,
                success_total_tokens INTEGER NOT NULL DEFAULT 0,
                success_input_tokens INTEGER NOT NULL DEFAULT 0,
                success_output_tokens INTEGER NOT NULL DEFAULT 0,
                success_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                success_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                success_cost REAL NOT NULL DEFAULT 0,
                client_error_requests INTEGER NOT NULL DEFAULT 0,
                server_error_requests INTEGER NOT NULL DEFAULT 0,
                local_only_requests INTEGER NOT NULL DEFAULT 0,
                rate_sum REAL NOT NULL DEFAULT 0,
                rate_count INTEGER NOT NULL DEFAULT 0,
                ttft_sum REAL NOT NULL DEFAULT 0,
                ttft_count INTEGER NOT NULL DEFAULT 0,
                status_counts_json TEXT NOT NULL DEFAULT '{}',
                materialized_at INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(local_date, model_name)
            );
            CREATE INDEX IF NOT EXISTS idx_unified_daily_model_summary_date
                ON unified_daily_model_summary(local_date);
            "#,
        )
        .map_err(|e| format!("Failed to create unified materialized tables: {}", e))?;
        Ok(())
    }

    pub(super) fn create_cache_tables(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS local_source_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tool TEXT NOT NULL,
                session_id TEXT NOT NULL,
                project_key TEXT,
                file_path TEXT NOT NULL UNIQUE,
                file_role TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                mtime_epoch INTEGER NOT NULL,
                fingerprint TEXT NOT NULL,
                last_scanned_at INTEGER NOT NULL,
                last_synced_at INTEGER,
                sync_status TEXT NOT NULL DEFAULT 'ready',
                sync_error TEXT,
                deleted_at INTEGER,
                deletion_reason TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_local_source_files_session_id
                ON local_source_files(session_id);
            CREATE INDEX IF NOT EXISTS idx_local_source_files_tool
                ON local_source_files(tool);
            CREATE INDEX IF NOT EXISTS idx_local_source_files_project_key
                ON local_source_files(project_key);

            CREATE TABLE IF NOT EXISTS local_sessions (
                session_id TEXT PRIMARY KEY,
                tool TEXT NOT NULL,
                project_key TEXT,
                cwd TEXT,
                project_name TEXT,
                topic TEXT,
                last_prompt TEXT,
                session_name TEXT,
                primary_file_path TEXT,
                file_size INTEGER NOT NULL DEFAULT 0,
                last_modified INTEGER NOT NULL DEFAULT 0,
                start_time INTEGER NOT NULL DEFAULT 0,
                end_time INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                model_list_json TEXT NOT NULL DEFAULT '[]',
                source_kind TEXT NOT NULL DEFAULT 'local_transcript',
                sync_version INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_local_sessions_tool
                ON local_sessions(tool);
            CREATE INDEX IF NOT EXISTS idx_local_sessions_project_key
                ON local_sessions(project_key);
            CREATE INDEX IF NOT EXISTS idx_local_sessions_end_time
                ON local_sessions(end_time);

            CREATE TABLE IF NOT EXISTS local_request_facts (
                request_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                tool TEXT NOT NULL,
                project_key TEXT,
                timestamp INTEGER NOT NULL,
                message_id TEXT,
                dedupe_key TEXT NOT NULL,
                request_key TEXT,
                model TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 1,
                explicit_estimated_cost REAL,
                source_file_id INTEGER,
                source_file_path TEXT,
                source_file_present INTEGER NOT NULL DEFAULT 1,
                source_offset INTEGER,
                event_index INTEGER,
                is_subagent INTEGER NOT NULL DEFAULT 0,
                raw_event_kind TEXT NOT NULL DEFAULT '',
                sync_version INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                UNIQUE(tool, dedupe_key)
            );
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_timestamp
                ON local_request_facts(timestamp);
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_session_id
                ON local_request_facts(session_id);
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_project_key
                ON local_request_facts(project_key);
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_tool_timestamp
                ON local_request_facts(tool, timestamp);
            CREATE INDEX IF NOT EXISTS idx_local_request_facts_model
                ON local_request_facts(model);

            CREATE TABLE IF NOT EXISTS local_sync_cursors (
                cursor_key TEXT PRIMARY KEY,
                tool TEXT NOT NULL,
                file_path TEXT NOT NULL,
                last_offset INTEGER,
                last_event_index INTEGER,
                last_seen_timestamp INTEGER,
                last_seen_dedupe_key TEXT,
                payload_json TEXT,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_local_sync_cursors_tool
                ON local_sync_cursors(tool);
            CREATE INDEX IF NOT EXISTS idx_local_sync_cursors_file_path
                ON local_sync_cursors(file_path);

            CREATE TABLE IF NOT EXISTS remote_devices (
                device_id TEXT PRIMARY KEY,
                last_seen_at INTEGER,
                last_export_seq INTEGER NOT NULL DEFAULT 0,
                sync_status TEXT NOT NULL DEFAULT 'ready',
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS remote_request_facts (
                request_key TEXT NOT NULL,
                origin_device_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                tool TEXT NOT NULL,
                project_key TEXT,
                timestamp INTEGER NOT NULL,
                message_id TEXT,
                dedupe_key TEXT NOT NULL,
                model TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 1,
                explicit_estimated_cost REAL,
                is_subagent INTEGER NOT NULL DEFAULT 0,
                source_kind TEXT NOT NULL DEFAULT 'remote_sync',
                imported_at INTEGER NOT NULL,
                export_seq INTEGER NOT NULL,
                PRIMARY KEY(origin_device_id, request_key)
            );
            CREATE INDEX IF NOT EXISTS idx_remote_request_facts_timestamp
                ON remote_request_facts(timestamp);
            CREATE INDEX IF NOT EXISTS idx_remote_request_facts_session_id
                ON remote_request_facts(session_id);
            CREATE INDEX IF NOT EXISTS idx_remote_request_facts_tool_timestamp
                ON remote_request_facts(tool, timestamp);

            CREATE TABLE IF NOT EXISTS remote_sessions (
                origin_device_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                tool TEXT NOT NULL,
                project_key TEXT,
                project_name TEXT,
                start_time INTEGER NOT NULL DEFAULT 0,
                end_time INTEGER NOT NULL DEFAULT 0,
                request_count INTEGER NOT NULL DEFAULT 0,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_create_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                model_list_json TEXT NOT NULL DEFAULT '[]',
                imported_at INTEGER NOT NULL,
                export_seq INTEGER NOT NULL,
                PRIMARY KEY(origin_device_id, session_id)
            );
            CREATE INDEX IF NOT EXISTS idx_remote_sessions_tool
                ON remote_sessions(tool);
            CREATE INDEX IF NOT EXISTS idx_remote_sessions_end_time
                ON remote_sessions(end_time);

            CREATE TABLE IF NOT EXISTS webdav_sync_state (
                state_key TEXT PRIMARY KEY,
                state_value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("Failed to create local usage tables: {}", e))?;
        Ok(())
    }

    pub(super) fn create_sync_v2_tables(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sync_outbox_request_events (
                event_id TEXT PRIMARY KEY,
                origin_device_id TEXT NOT NULL,
                request_key TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                event_version INTEGER NOT NULL,
                queued_at INTEGER NOT NULL,
                batched_seq INTEGER,
                uploaded_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_sync_outbox_request_events_uploaded_at
                ON sync_outbox_request_events(uploaded_at, queued_at);

            CREATE TABLE IF NOT EXISTS sync_outbox_session_events (
                session_event_id TEXT PRIMARY KEY,
                origin_device_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                session_version INTEGER NOT NULL,
                queued_at INTEGER NOT NULL,
                batched_seq INTEGER,
                uploaded_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_sync_outbox_session_events_uploaded_at
                ON sync_outbox_session_events(uploaded_at, queued_at);

            CREATE TABLE IF NOT EXISTS sync_device_cursors (
                device_id TEXT PRIMARY KEY,
                last_imported_batch_seq INTEGER NOT NULL DEFAULT 0,
                last_imported_snapshot_seq INTEGER,
                last_seen_instance_id TEXT,
                last_seen_at INTEGER NOT NULL,
                last_status TEXT NOT NULL DEFAULT 'idle',
                last_error TEXT
            );

            CREATE TABLE IF NOT EXISTS sync_batch_history (
                batch_seq INTEGER PRIMARY KEY,
                request_event_count INTEGER NOT NULL,
                session_event_count INTEGER NOT NULL,
                exported_at INTEGER NOT NULL,
                remote_path TEXT NOT NULL,
                status TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sync_settings_state (
                document_key TEXT PRIMARY KEY,
                local_version INTEGER NOT NULL,
                remote_version INTEGER,
                last_pushed_at INTEGER,
                last_pulled_at INTEGER
            );
            "#,
        )
        .map_err(|e| format!("Failed to create sync V2 tables: {}", e))?;
        Ok(())
    }
}
