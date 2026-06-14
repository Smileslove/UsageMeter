use rusqlite::{params, Connection, OptionalExtension};

use super::LocalUsageDatabase;

impl LocalUsageDatabase {
    fn load_schema_version(conn: &Connection) -> Result<i64, String> {
        let version = conn
            .query_row(
                "SELECT state_value FROM local_sync_state WHERE state_key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("Failed to query local usage schema version: {}", e))?;

        Ok(version
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(1))
    }

    pub(super) fn migrate_schema(conn: &Connection) -> Result<(), String> {
        let schema_version = Self::load_schema_version(conn)?;
        if schema_version >= 15 {
            return Ok(());
        }
        let mut cleared_runtime_caches = false;

        if schema_version < 2 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start local usage schema migration: {}", e))?;

            tx.execute_batch(
                r#"
                DROP TABLE IF EXISTS local_request_facts;
                DELETE FROM local_sessions;
                DELETE FROM local_source_files;
                DELETE FROM local_sync_cursors;
                "#,
            )
            .map_err(|e| format!("Failed to reset local usage cache during migration: {}", e))?;

            Self::create_cache_tables(&tx)?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '2', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| {
                format!(
                    "Failed to update migrated local usage schema version: {}",
                    e
                )
            })?;

            tx.commit()
                .map_err(|e| format!("Failed to commit local usage schema migration: {}", e))?;
        }

        if schema_version < 3 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start remote device schema migration: {}", e))?;

            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS remote_devices_v3 (
                    device_id TEXT PRIMARY KEY,
                    last_seen_at INTEGER,
                    last_export_seq INTEGER NOT NULL DEFAULT 0,
                    sync_status TEXT NOT NULL DEFAULT 'ready',
                    updated_at INTEGER NOT NULL
                );
                INSERT INTO remote_devices_v3 (
                    device_id, last_seen_at, last_export_seq, sync_status, updated_at
                )
                SELECT device_id, last_seen_at, last_export_seq, sync_status, updated_at
                FROM remote_devices;
                DROP TABLE remote_devices;
                ALTER TABLE remote_devices_v3 RENAME TO remote_devices;
                "#,
            )
            .map_err(|e| format!("Failed to migrate remote devices schema: {}", e))?;

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '3', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update remote device schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit remote device schema migration: {}", e))?;
        }

        if schema_version < 4 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start sync V2 schema migration: {}", e))?;

            Self::create_sync_v2_tables(&tx)?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '4', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update sync V2 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit sync V2 schema migration: {}", e))?;
        }

        if schema_version < 5 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v5 schema migration: {}", e))?;

            Self::add_column_if_missing(&tx, "local_request_facts", "request_key", "TEXT")?;
            Self::add_column_if_missing(&tx, "local_request_facts", "source_file_path", "TEXT")?;
            Self::add_column_if_missing(
                &tx,
                "local_request_facts",
                "source_file_present",
                "INTEGER NOT NULL DEFAULT 1",
            )?;
            Self::add_column_if_missing(&tx, "local_source_files", "deleted_at", "INTEGER")?;
            Self::add_column_if_missing(&tx, "local_source_files", "deletion_reason", "TEXT")?;

            tx.execute_batch(
                r#"
                CREATE INDEX IF NOT EXISTS idx_local_request_facts_request_key
                    ON local_request_facts(request_key);
                CREATE INDEX IF NOT EXISTS idx_local_request_facts_source_file_present
                    ON local_request_facts(source_file_present);
                CREATE INDEX IF NOT EXISTS idx_local_source_files_deleted_at
                    ON local_source_files(deleted_at);
                "#,
            )
            .map_err(|e| format!("Failed to create v5 indexes: {}", e))?;

            tx.execute(
                "UPDATE local_request_facts
                 SET request_key = CASE
                     WHEN message_id IS NOT NULL AND TRIM(message_id) != ''
                       THEN tool || ':' || message_id
                     ELSE tool || ':' || session_id || ':' || timestamp || ':' || model
                          || ':' || input_tokens || ':' || output_tokens
                          || ':' || cache_create_tokens || ':' || cache_read_tokens
                          || ':' || total_tokens
                 END
                 WHERE request_key IS NULL OR request_key = ''",
                [],
            )
            .map_err(|e| format!("Failed to backfill request_key: {}", e))?;

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '5', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v5 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit v5 schema migration: {}", e))?;
        }

        if schema_version < 6 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v6 schema migration: {}", e))?;

            Self::create_unified_materialized_tables(&tx)?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '6', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v6 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit v6 schema migration: {}", e))?;
        }

        if schema_version < 7 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v7 schema migration: {}", e))?;

            Self::create_unified_materialized_tables(&tx)?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '7', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v7 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit v7 schema migration: {}", e))?;
        }

        if schema_version < 8 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v8 schema migration: {}", e))?;

            Self::create_unified_materialized_tables(&tx)?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_model_summary",
                "success_total_tokens",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_model_summary",
                "success_input_tokens",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_model_summary",
                "success_output_tokens",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_model_summary",
                "success_cache_create_tokens",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_model_summary",
                "success_cache_read_tokens",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_model_summary",
                "success_cost",
                "REAL NOT NULL DEFAULT 0",
            )?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '8', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v8 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit v8 schema migration: {}", e))?;
        }

        if schema_version < 9 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v9 schema migration: {}", e))?;

            Self::create_unified_materialized_tables(&tx)?;
            for column in [
                ("visible_request_count", "INTEGER NOT NULL DEFAULT 0"),
                ("visible_total_tokens", "INTEGER NOT NULL DEFAULT 0"),
                ("visible_input_tokens", "INTEGER NOT NULL DEFAULT 0"),
                ("visible_output_tokens", "INTEGER NOT NULL DEFAULT 0"),
                ("visible_cache_create_tokens", "INTEGER NOT NULL DEFAULT 0"),
                ("visible_cache_read_tokens", "INTEGER NOT NULL DEFAULT 0"),
                ("visible_cost", "REAL NOT NULL DEFAULT 0"),
            ] {
                Self::add_column_if_missing(&tx, "unified_daily_summary", column.0, column.1)?;
                Self::add_column_if_missing(
                    &tx,
                    "unified_daily_model_summary",
                    column.0,
                    column.1,
                )?;
            }
            tx.execute("DELETE FROM unified_daily_materialization_state", [])
                .map_err(|e| format!("Failed to clear v9 materialization state: {}", e))?;
            tx.execute("DELETE FROM unified_daily_summary", [])
                .map_err(|e| format!("Failed to clear v9 daily summary: {}", e))?;
            tx.execute("DELETE FROM unified_daily_model_summary", [])
                .map_err(|e| format!("Failed to clear v9 model summary: {}", e))?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '9', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v9 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v9 schema migration: {}", e))?;
        }

        if schema_version < 10 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v10 schema migration: {}", e))?;

            Self::create_unified_materialized_tables(&tx)?;
            for column in [
                ("local_max_sync_version", "INTEGER NOT NULL DEFAULT 0"),
                ("local_max_timestamp", "INTEGER NOT NULL DEFAULT 0"),
                ("remote_max_export_seq", "INTEGER NOT NULL DEFAULT 0"),
                ("remote_max_timestamp", "INTEGER NOT NULL DEFAULT 0"),
                ("proxy_max_timestamp_ms", "INTEGER NOT NULL DEFAULT 0"),
                ("proxy_max_updated_at", "INTEGER NOT NULL DEFAULT 0"),
            ] {
                Self::add_column_if_missing(
                    &tx,
                    "unified_daily_materialization_state",
                    column.0,
                    column.1,
                )?;
            }
            Self::upsert_sync_state(
                &tx,
                "unified_materialization_invalidation_version",
                "1",
                chrono::Utc::now().timestamp(),
            )?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '10', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v10 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit v10 schema migration: {}", e))?;
        }

        if schema_version < 11 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v11 schema migration: {}", e))?;

            Self::add_column_if_missing(
                &tx,
                "unified_daily_model_summary",
                "local_only_requests",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            tx.execute("DELETE FROM unified_daily_model_summary", [])
                .map_err(|e| format!("Failed to clear v11 model summary: {}", e))?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '11', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v11 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v11 schema migration: {}", e))?;
        }

        if schema_version < 12 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v12 schema migration: {}", e))?;

            Self::add_column_if_missing(
                &tx,
                "local_request_facts",
                "reasoning_tokens",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '12', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v12 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v12 schema migration: {}", e))?;
        }

        if schema_version < 13 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v13 schema migration: {}", e))?;

            Self::add_column_if_missing(
                &tx,
                "unified_daily_materialization_state",
                "day_boundary_mode",
                "TEXT NOT NULL DEFAULT 'standard'",
            )?;
            tx.execute("DELETE FROM unified_daily_materialization_state", [])
                .map_err(|e| format!("Failed to clear v13 materialization state: {}", e))?;
            tx.execute("DELETE FROM unified_daily_summary", [])
                .map_err(|e| format!("Failed to clear v13 daily summary: {}", e))?;
            tx.execute("DELETE FROM unified_daily_model_summary", [])
                .map_err(|e| format!("Failed to clear v13 model summary: {}", e))?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '13', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v13 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v13 schema migration: {}", e))?;
        }

        if schema_version < 14 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v14 schema migration: {}", e))?;

            Self::add_column_if_missing(
                &tx,
                "local_request_facts",
                "request_count",
                "INTEGER NOT NULL DEFAULT 1",
            )?;
            Self::add_column_if_missing(
                &tx,
                "local_request_facts",
                "explicit_estimated_cost",
                "REAL",
            )?;
            Self::add_column_if_missing(
                &tx,
                "remote_request_facts",
                "request_count",
                "INTEGER NOT NULL DEFAULT 1",
            )?;
            Self::add_column_if_missing(
                &tx,
                "remote_request_facts",
                "explicit_estimated_cost",
                "REAL",
            )?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_materialized_facts",
                "request_count",
                "INTEGER NOT NULL DEFAULT 1",
            )?;

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '14', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v14 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v14 schema migration: {}", e))?;
        }

        if schema_version < 15 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v15 schema migration: {}", e))?;

            Self::add_column_if_missing(&tx, "local_sessions", "scope", "TEXT")?;
            Self::add_column_if_missing(&tx, "remote_sessions", "scope", "TEXT")?;

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '15', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v15 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v15 schema migration: {}", e))?;
        }

        if cleared_runtime_caches {
            crate::unified_usage::clear_runtime_caches();
        }

        Ok(())
    }

    pub(super) fn add_column_if_missing(
        tx: &rusqlite::Transaction<'_>,
        table: &str,
        column: &str,
        column_def: &str,
    ) -> Result<(), String> {
        let exists: bool = tx
            .prepare(&format!("PRAGMA table_info({})", table))
            .map_err(|e| format!("Failed to inspect table {}: {}", table, e))?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| format!("Failed to read columns of {}: {}", table, e))?
            .filter_map(|name| name.ok())
            .any(|name| name == column);
        if exists {
            return Ok(());
        }
        tx.execute(
            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, column_def),
            [],
        )
        .map_err(|e| format!("Failed to add column {}.{}: {}", table, column, e))?;
        Ok(())
    }
}
