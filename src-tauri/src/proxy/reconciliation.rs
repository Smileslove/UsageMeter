//! Proxy request to local-session reconciliation.
//!
//! Proxy ingestion persists a request fact without requiring a local session scan. This service
//! owns the optional bridge to the session cache and keeps the proxy database limited to
//! persistence and aggregation work.

use super::database::{
    computed_session_resolution_state, ProxyDatabase, LEGACY_UNMATCHED_SESSION_ID,
};
use super::types::UsageRecord;
use std::sync::Arc;
use tokio::sync::mpsc;

const RECONCILIATION_QUEUE_CAPACITY: usize = 256;

struct ReconciliationJob {
    database: Arc<ProxyDatabase>,
    record: UsageRecord,
}

struct WorkerState {
    sender: std::sync::Mutex<Option<mpsc::Sender<ReconciliationJob>>>,
}

/// Resolves a locally scanned session from a proxy request identifier.
///
/// This is a port rather than a direct dependency of the collector or database, so a future
/// background reconciliation worker can use a different source without changing ingestion.
pub(crate) trait SessionIdResolver: Send + Sync {
    fn find_session_id_by_message_id(&self, message_id: &str) -> Option<String>;
}

struct CachedSessionIdResolver;

impl SessionIdResolver for CachedSessionIdResolver {
    fn find_session_id_by_message_id(&self, message_id: &str) -> Option<String> {
        crate::session::find_session_id_by_message_id(message_id)
    }
}

enum SessionStatsTarget {
    Resolved(String),
    LegacyUnmatched,
    Deferred,
}

/// Application service for deriving the legacy session-statistics projection from a proxy fact.
pub(crate) struct ProxySessionReconciliationService {
    session_id_resolver: Arc<dyn SessionIdResolver>,
    worker: Arc<WorkerState>,
}

impl Default for ProxySessionReconciliationService {
    fn default() -> Self {
        Self::new(Arc::new(CachedSessionIdResolver))
    }
}

impl ProxySessionReconciliationService {
    pub(crate) fn new(session_id_resolver: Arc<dyn SessionIdResolver>) -> Self {
        Self {
            session_id_resolver,
            worker: Arc::new(WorkerState {
                sender: std::sync::Mutex::new(None),
            }),
        }
    }

    fn ensure_worker(&self) -> Result<mpsc::Sender<ReconciliationJob>, String> {
        let mut sender = self
            .worker
            .sender
            .lock()
            .map_err(|_| "reconciliation worker state poisoned".to_string())?;
        if let Some(sender) = sender.as_ref() {
            return Ok(sender.clone());
        }

        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "reconciliation worker requires a Tokio runtime".to_string())?;
        let (tx, mut rx) = mpsc::channel::<ReconciliationJob>(RECONCILIATION_QUEUE_CAPACITY);
        let resolver = self.session_id_resolver.clone();
        handle.spawn(async move {
            while let Some(job) = rx.recv().await {
                let resolver = resolver.clone();
                match tokio::task::spawn_blocking(move || {
                    Self::reconcile_after_ingest_with_resolver(
                        resolver.as_ref(),
                        &job.database,
                        &job.record,
                    )
                })
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        eprintln!("[collector] Failed to reconcile session stats: {error}");
                    }
                    Err(error) => {
                        eprintln!("[collector] Reconciliation worker task failed: {error}");
                    }
                }
            }
        });
        *sender = Some(tx.clone());
        Ok(tx)
    }

    /// Enqueue a persisted proxy fact for background session-statistics projection.
    ///
    /// The bounded queue applies backpressure without running the synchronous database work on
    /// the ingestion task. A direct fallback is retained for callers created outside a Tokio
    /// runtime, which keeps construction and existing synchronous tests compatible.
    pub(crate) async fn enqueue_after_ingest(
        &self,
        database: Arc<ProxyDatabase>,
        record: UsageRecord,
    ) -> Result<(), String> {
        let sender = match self.ensure_worker() {
            Ok(sender) => sender,
            Err(error) if error.contains("requires a Tokio runtime") => {
                return self.reconcile_after_ingest(&database, &record);
            }
            Err(error) => return Err(error),
        };

        match sender.send(ReconciliationJob { database, record }).await {
            Ok(()) => Ok(()),
            Err(mpsc::error::SendError(job)) => {
                // A runtime shutdown can close the worker between `ensure_worker` and send.
                // Preserve the old projection guarantee for that rare boundary case.
                self.reconcile_after_ingest(&job.database, &job.record)
            }
        }
    }

    /// Projects a persisted proxy fact into the legacy `session_stats` table when safe.
    ///
    /// OpenCode requests without an explicit session id are deferred. Their local source uses
    /// token/time matching, which is performed by the scanner-driven reconciliation pass.
    pub(crate) fn reconcile_after_ingest(
        &self,
        database: &ProxyDatabase,
        record: &UsageRecord,
    ) -> Result<(), String> {
        Self::reconcile_after_ingest_with_resolver(
            self.session_id_resolver.as_ref(),
            database,
            record,
        )
    }

    fn reconcile_after_ingest_with_resolver(
        session_id_resolver: &dyn SessionIdResolver,
        database: &ProxyDatabase,
        record: &UsageRecord,
    ) -> Result<(), String> {
        match Self::session_stats_target_with_resolver(session_id_resolver, record) {
            SessionStatsTarget::Resolved(session_id) => {
                database.update_session_stats_for_resolved_session(record, &session_id)
            }
            SessionStatsTarget::LegacyUnmatched => database
                .update_session_stats_for_resolved_session(record, LEGACY_UNMATCHED_SESSION_ID),
            SessionStatsTarget::Deferred => Ok(()),
        }
    }

    #[cfg(test)]
    fn session_stats_target(&self, record: &UsageRecord) -> SessionStatsTarget {
        Self::session_stats_target_with_resolver(self.session_id_resolver.as_ref(), record)
    }

    fn session_stats_target_with_resolver(
        session_id_resolver: &dyn SessionIdResolver,
        record: &UsageRecord,
    ) -> SessionStatsTarget {
        if let Some(session_id) = record
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|session_id| !session_id.is_empty())
        {
            return SessionStatsTarget::Resolved(session_id.to_string());
        }

        if record.client_tool == "opencode" && computed_session_resolution_state(record) != "known"
        {
            return SessionStatsTarget::Deferred;
        }

        session_id_resolver
            .find_session_id_by_message_id(&record.message_id)
            .filter(|session_id| !session_id.trim().is_empty())
            .map(SessionStatsTarget::Resolved)
            .unwrap_or(SessionStatsTarget::LegacyUnmatched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubSessionIdResolver(Option<String>);

    impl SessionIdResolver for StubSessionIdResolver {
        fn find_session_id_by_message_id(&self, _message_id: &str) -> Option<String> {
            self.0.clone()
        }
    }

    fn reconciler(result: Option<&str>) -> ProxySessionReconciliationService {
        ProxySessionReconciliationService::new(Arc::new(StubSessionIdResolver(
            result.map(str::to_string),
        )))
    }

    #[test]
    fn explicit_session_id_does_not_query_local_session_source() {
        let record = UsageRecord {
            session_id: Some("session-1".to_string()),
            ..Default::default()
        };

        assert!(matches!(
            reconciler(None).session_stats_target(&record),
            SessionStatsTarget::Resolved(session_id) if session_id == "session-1"
        ));
    }

    #[test]
    fn unresolved_opencode_record_is_deferred_to_scanner_reconciliation() {
        let record = UsageRecord {
            client_tool: "opencode".to_string(),
            ..Default::default()
        };

        assert!(matches!(
            reconciler(Some("unexpected-session")).session_stats_target(&record),
            SessionStatsTarget::Deferred
        ));
    }

    #[test]
    fn other_tools_use_resolver_then_legacy_projection() {
        let record = UsageRecord {
            client_tool: "claude".to_string(),
            message_id: "message-1".to_string(),
            ..Default::default()
        };

        assert!(matches!(
            reconciler(Some("session-1")).session_stats_target(&record),
            SessionStatsTarget::Resolved(session_id) if session_id == "session-1"
        ));
        assert!(matches!(
            reconciler(None).session_stats_target(&record),
            SessionStatsTarget::LegacyUnmatched
        ));
    }

    #[test]
    fn reconciliation_projects_resolved_request_without_database_session_lookup() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let database = ProxyDatabase::new_with_path(&temp_dir.path().join("proxy_data.db"))
            .expect("open proxy database");
        let record = UsageRecord {
            client_tool: "claude".to_string(),
            message_id: "message-1".to_string(),
            input_tokens: 12,
            output_tokens: 8,
            total_tokens: 20,
            ..Default::default()
        };

        reconciler(Some("session-1"))
            .reconcile_after_ingest(&database, &record)
            .expect("project session stats");

        let conn = database.conn.lock().expect("lock database");
        let (request_count, input_tokens, output_tokens): (i64, i64, i64) = conn
            .query_row(
                "SELECT proxy_request_count, total_input_tokens, total_output_tokens
                 FROM session_stats WHERE session_id = 'session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("read projected stats");
        assert_eq!((request_count, input_tokens, output_tokens), (1, 12, 8));
    }
}
