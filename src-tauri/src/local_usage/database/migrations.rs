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
        if schema_version >= 29 {
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

        if schema_version < 16 {
            // Codex local-scan records fabricate a per-request message_id that can never
            // exact-key-match their real proxy-captured counterpart, so before this version
            // every Codex request routed through the local proxy was double-counted (once via
            // `from_local` as "未归因", once via `from_proxy` correctly attributed). The merge
            // logic now reconciles these via fuzzy matching, but already-materialized historical
            // days computed under the old logic won't self-correct: `materialization_state_matches`
            // only tracks raw-input fingerprints (record counts/timestamps/pricing), not merge
            // logic version, so a pure logic fix never trips `needs_rebuild`. Clear every
            // persisted day so the next read recomputes through `merge_realtime_range` with the
            // fixed reconciliation — a one-time, automatic backfill instead of requiring users to
            // discover and click "重建本地缓存".
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v16 schema migration: {}", e))?;

            Self::clear_unified_materialization_tx(&tx, chrono::Utc::now().timestamp())?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '16', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v16 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v16 schema migration: {}", e))?;
        }

        if schema_version < 17 {
            // The v16 fuzzy-match reconciliation itself never actually matched anything: the
            // local scanner's session id is namespaced `codex::<uuid>` while the proxy captures
            // the bare uuid Codex CLI sends, so the exact `==` comparison in
            // `codex_fuzzy_candidate_matches` failed for every single Codex request. Every day
            // materialized under v16 is still silently double-counted (and, after the v16 fix,
            // misattributed to the *real* source instead of "未归因" — worse to spot). Clear
            // again now that the comparison strips the prefix.
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v17 schema migration: {}", e))?;

            Self::clear_unified_materialization_tx(&tx, chrono::Utc::now().timestamp())?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '17', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v17 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v17 schema migration: {}", e))?;
        }

        if schema_version < 18 {
            // The v17 fix still required `proxy.session_id == local.session_id` (after prefix
            // stripping) — but real Codex CLI requests carry no session/conversation
            // identifier the proxy can observe at all (confirmed against production data:
            // session_id is empty for every captured Codex proxy record). So the equality
            // check failed for every request just like v16 did, for a different reason. The
            // match now relies on model + total_tokens + close timestamp only. Every day
            // materialized under v16/v17 is still double-counted — clear again.
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v18 schema migration: {}", e))?;

            Self::clear_unified_materialization_tx(&tx, chrono::Utc::now().timestamp())?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '18', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v18 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v18 schema migration: {}", e))?;
        }

        if schema_version < 19 {
            // v18 dropped session_id but still required the local-vs-proxy match on a single
            // `total_tokens` equality. Two independent per-request divergences defeat that:
            // (1) Codex's JSONL token_count events never report cache_creation, so the local
            // total omits a component the proxy total includes; (2) the two sides derive tokens
            // differently (cumulative-delta vs single-response usage), so totals rarely line up
            // exactly. The match now uses cc-switch's per-field fingerprint (input/output/
            // cache_read exact + cache_create "unknown passthrough") over a 10-minute window.
            // Days materialized under v16–v18 are still double-counted — clear once more so they
            // recompute under the corrected match.
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v19 schema migration: {}", e))?;

            Self::clear_unified_materialization_tx(&tx, chrono::Utc::now().timestamp())?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '19', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v19 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v19 schema migration: {}", e))?;
        }

        if schema_version < 20 {
            // v20 makes persisted materialization-state rows the authoritative freshness marker:
            // every production Local/Remote/Proxy write path now deletes affected historical
            // states. Clear rows built by older versions once, because their write paths did not
            // consistently invalidate after Proxy session reconciliation.
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v20 schema migration: {}", e))?;

            Self::clear_unified_materialization_tx(&tx, chrono::Utc::now().timestamp())?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '20', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v20 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v20 schema migration: {}", e))?;
        }

        if schema_version < 21 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v21 schema migration: {}", e))?;

            for table in ["local_sessions", "remote_sessions"] {
                Self::add_column_if_missing(
                    &tx,
                    table,
                    "total_reasoning_tokens",
                    "INTEGER NOT NULL DEFAULT 0",
                )?;
                Self::add_column_if_missing(
                    &tx,
                    table,
                    "total_elapsed_ms",
                    "INTEGER NOT NULL DEFAULT 0",
                )?;
                Self::add_column_if_missing(&tx, table, "explicit_cost", "REAL")?;
                Self::add_column_if_missing(&tx, table, "explicit_cost_currency", "TEXT")?;
                Self::add_column_if_missing(
                    &tx,
                    table,
                    "usage_sources_json",
                    "TEXT NOT NULL DEFAULT '{}'",
                )?;
            }
            // Reasonix v2 会话累计值会参与新的残差聚合，旧物化结果必须重算；
            // 仅清理派生缓存，不删除会话、请求或代理原始事实。
            Self::clear_unified_materialization_tx(&tx, chrono::Utc::now().timestamp())?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '21', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v21 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v21 schema migration: {}", e))?;
        }

        if schema_version < 22 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v22 schema migration: {}", e))?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_materialization_state",
                "fact_cache_status",
                "TEXT NOT NULL DEFAULT 'complete'",
            )?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '22', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v22 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v22 schema migration: {}", e))?;
        }

        if schema_version < 23 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v23 schema migration: {}", e))?;
            for table in ["sync_outbox_request_events", "sync_outbox_session_events"] {
                Self::add_column_if_missing(&tx, table, "discarded_at", "INTEGER")?;
                Self::add_column_if_missing(&tx, table, "discard_reason", "TEXT")?;
            }
            tx.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_sync_outbox_request_events_active
                 ON sync_outbox_request_events(origin_device_id, uploaded_at, discarded_at, queued_at);
                 CREATE INDEX IF NOT EXISTS idx_sync_outbox_session_events_active
                 ON sync_outbox_session_events(origin_device_id, uploaded_at, discarded_at, queued_at);",
            )
            .map_err(|e| format!("Failed to create v23 outbox indexes: {}", e))?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '23', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v23 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v23 schema migration: {}", e))?;
        }

        if schema_version < 24 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v24 schema migration: {}", e))?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS local_session_tombstones (
                    session_id TEXT PRIMARY KEY,
                    tool TEXT NOT NULL,
                    project_key TEXT,
                    project_name TEXT,
                    scope TEXT,
                    start_time INTEGER NOT NULL DEFAULT 0,
                    end_time INTEGER NOT NULL DEFAULT 0,
                    deleted_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_local_session_tombstones_updated_at
                    ON local_session_tombstones(updated_at);",
            )
            .map_err(|e| format!("Failed to create v24 session tombstones: {}", e))?;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '24', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v24 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v24 schema migration: {}", e))?;
        }

        if schema_version < 25 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v25 schema migration: {}", e))?;

            // 网关 proxy-only 事实的 tool 归因变更（此前 api_gateway 归因到真实工具）
            // 需要重建历史物化数据，否则升级后历史统计仍按 api_gateway 展示、与
            // 键前缀分裂。清空四张物化表并 bump invalidation version，让下一次查询
            // 按新归因全量重新物化（一次性成本）。
            tx.execute("DELETE FROM unified_daily_materialized_facts", [])
                .map_err(|e| format!("Failed to clear v25 materialized facts: {}", e))?;
            tx.execute("DELETE FROM unified_daily_summary", [])
                .map_err(|e| format!("Failed to clear v25 daily summary: {}", e))?;
            tx.execute("DELETE FROM unified_daily_model_summary", [])
                .map_err(|e| format!("Failed to clear v25 model summary: {}", e))?;
            tx.execute("DELETE FROM unified_daily_materialization_state", [])
                .map_err(|e| format!("Failed to clear v25 materialization state: {}", e))?;
            Self::bump_unified_materialization_invalidation_version_tx(
                &tx,
                chrono::Utc::now().timestamp(),
            )?;
            cleared_runtime_caches = true;
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '25', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v25 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v25 schema migration: {}", e))?;
        }

        if schema_version < 26 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v26 schema migration: {}", e))?;

            // Reasonix telemetry `estimated` 标记：本地来源可在用量估算（请求中断/
            // 失败）时标注估算数据。local_sessions / remote_sessions 持久化会话级标记，
            // unified_daily_materialized_facts 持久化事实级标记。
            Self::add_column_if_missing(
                &tx,
                "local_sessions",
                "estimated",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::add_column_if_missing(
                &tx,
                "remote_sessions",
                "estimated",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_materialized_facts",
                "estimated",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            Self::add_column_if_missing(
                &tx,
                "unified_daily_model_summary",
                "estimated_request_count",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            // 事实级 estimated 仅影响展示口径（估算标注），无需重建物化数据：
            // 旧行默认 0（非估算），与新逻辑一致；存量 reasonix 估算会话会在
            // 下一次自然物化时带上标记。
            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '26', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v26 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v26 schema migration: {}", e))?;
        }

        if schema_version < 27 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v27 schema migration: {}", e))?;

            // v27：移除 ReasonX 本地会话统计链路（会话级累计 telemetry 按 end_time
            // 整段归入统计窗口，粒度太粗导致窗口数据失真）。仅清理 reasonix 的
            // 本地会话行与物化残差，**绝不触碰 proxy_data.db**：代理采集的
            // reasonix / api_gateway 逐请求数据（usage_records）继续保留与统计。
            //
            // 防护：删除后校验关键表（reasonix 会话与残差）已清空，不一致则回滚。
            // 其余表（墓碑/源文件/远程行）与 reasonix 会话同源，由同一事务保证。

            // 1) 先收集 reasonix 会话 id（outbox 清理依赖它们；必须在删除
            //    local_sessions 之前取，否则子查询查不到行导致清理失效）。
            let reasonix_session_ids: Vec<String> = {
                let mut stmt = tx
                    .prepare("SELECT session_id FROM local_sessions WHERE tool = 'reasonix'")
                    .map_err(|e| format!("v27 failed to prepare reasonix session ids: {e}"))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| format!("v27 failed to query reasonix session ids: {e}"))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("v27 failed to collect reasonix session ids: {e}"))?
            };

            // 2) 待同步的 outbox 会话事件：在删除 local_sessions 前清理，
            //    避免向其它设备广播已删会话的导出。
            if !reasonix_session_ids.is_empty() {
                let placeholders = reasonix_session_ids
                    .iter()
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(", ");
                tx.execute(
                    &format!(
                        "DELETE FROM sync_outbox_session_events
                         WHERE session_id IN ({placeholders})"
                    ),
                    rusqlite::params_from_iter(reasonix_session_ids.iter()),
                )
                .map_err(|e| format!("v27 failed to delete reasonix outbox events: {e}"))?;
            }

            // 3) 会话级数据：本地会话、墓碑、源文件、远程同步会话。
            tx.execute("DELETE FROM local_sessions WHERE tool = 'reasonix'", [])
                .map_err(|e| format!("v27 failed to delete reasonix sessions: {e}"))?;

            tx.execute(
                "DELETE FROM local_session_tombstones WHERE tool = 'reasonix'",
                [],
            )
            .map_err(|e| format!("v27 failed to delete reasonix tombstones: {e}"))?;

            tx.execute("DELETE FROM local_source_files WHERE tool = 'reasonix'", [])
                .map_err(|e| format!("v27 failed to delete reasonix source files: {e}"))?;

            tx.execute("DELETE FROM remote_sessions WHERE tool = 'reasonix'", [])
                .map_err(|e| format!("v27 failed to delete reasonix remote sessions: {e}"))?;

            // 4) 物化残差：只删 reasonix 的 local_only（会话级残差事实）。
            //    proxy_only / merged（代理/网关数据）与其它工具完全保留。
            tx.execute(
                "DELETE FROM unified_daily_materialized_facts
                 WHERE tool = 'reasonix' AND coverage_origin = 'local_only'",
                [],
            )
            .map_err(|e| format!("v27 failed to delete reasonix residuals: {e}"))?;

            // 4) 失效全部历史物化（含统一日汇总/模型汇总/物化状态），
            //    下次查询按"无 reasonix 本地链"全量重物化。
            tx.execute("DELETE FROM unified_daily_summary", [])
                .map_err(|e| format!("v27 failed to clear unified daily summary: {e}"))?;
            tx.execute("DELETE FROM unified_daily_model_summary", [])
                .map_err(|e| format!("v27 failed to clear unified model summary: {e}"))?;
            tx.execute("DELETE FROM unified_daily_materialization_state", [])
                .map_err(|e| format!("v27 failed to clear materialization state: {e}"))?;
            Self::bump_unified_materialization_invalidation_version_tx(
                &tx,
                chrono::Utc::now().timestamp(),
            )?;

            // 5) 删除后校验：reasonix 会话与残差必须为 0；其它数据不受影响。
            let reasonix_sessions_after: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM local_sessions WHERE tool = 'reasonix'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| format!("v27 failed to re-count reasonix sessions: {e}"))?;
            if reasonix_sessions_after != 0 {
                return Err(format!(
                    "v27 reasonix session cleanup verification failed: expected 0, got {reasonix_sessions_after}"
                ));
            }
            let reasonix_residual_after: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM unified_daily_materialized_facts
                     WHERE tool = 'reasonix' AND coverage_origin = 'local_only'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| format!("v27 failed to re-count reasonix residuals: {e}"))?;
            if reasonix_residual_after != 0 {
                return Err(format!(
                    "v27 reasonix residual cleanup verification failed: expected 0, got {reasonix_residual_after}"
                ));
            }

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '27', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v27 schema version: {}", e))?;
            tx.commit()
                .map_err(|e| format!("Failed to commit v27 schema migration: {}", e))?;
            cleared_runtime_caches = true;
        }

        if schema_version < 28 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v28 schema migration: {}", e))?;

            // v28：深度活动索引 5 张表（M2；设计文档 12.4）。
            // 与既有迁移幂等兼容：新库直接建表（create_tables 已含 activity 表），
            // 老库升级到此块补建；v2 迁移的 DROP 三表历史逻辑不涉及这些新表。
            Self::create_activity_tables(&tx)?;

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '28', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v28 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit v28 schema migration: {}", e))?;
        }

        if schema_version < 29 {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("Failed to start v29 schema migration: {}", e))?;

            // v29：FTS5 全文索引虚拟表 `session_event_fts`（M3；设计文档 12.4
            // 「可选全文表」）。建表 SQL 与 `activity::db::ACTIVITY_TABLES_DDL`
            // 同一来源，`CREATE VIRTUAL TABLE IF NOT EXISTS` 幂等；新库建表
            // 已含该表（create_tables → create_activity_tables），老库升级到此
            // 块补建。索引数据不在此迁移回填：由 `write_activity_batch` 的
            // FTS 同步路径（activity::fts::sync_events_to_fts）在下次会话写入
            // 或 rebuild 时自动填充，存量会话无需一次性全量建索引。
            Self::create_activity_tables(&tx)?;

            tx.execute(
                "INSERT INTO local_sync_state (state_key, state_value, updated_at)
                 VALUES ('schema_version', '29', ?1)
                 ON CONFLICT(state_key) DO UPDATE
                 SET state_value = excluded.state_value,
                     updated_at = excluded.updated_at",
                params![chrono::Utc::now().timestamp()],
            )
            .map_err(|e| format!("Failed to update v29 schema version: {}", e))?;

            tx.commit()
                .map_err(|e| format!("Failed to commit v29 schema migration: {}", e))?;
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
