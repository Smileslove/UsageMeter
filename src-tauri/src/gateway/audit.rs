//! Gateway audit logging for security and operational tracking.

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

/// Gateway audit event for structured logging of security-relevant operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayAuditEvent {
    pub timestamp_ms: i64,
    pub event_type: GatewayAuditEventType,
    pub profile_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub details: serde_json::Value,
    pub result: AuditResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayAuditEventType {
    /// A request was forwarded through the gateway
    RequestForwarded,
    /// An upstream key was added to a profile
    UpstreamKeyAdded,
    /// An upstream key was deleted from a profile
    UpstreamKeyDeleted,
    /// An upstream key experienced authentication failure
    UpstreamKeyFailed,
    /// A local key was created
    LocalKeyCreated,
    /// A local key was revoked
    LocalKeyRevoked,
    /// A gateway profile was created
    ProfileCreated,
    /// A gateway profile was deleted
    ProfileDeleted,
    /// Failover was triggered due to upstream failure
    FailoverTriggered,
    /// Rate limit was exceeded
    RateLimitExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditResult {
    Success,
    Failure { reason: String },
}

/// Gateway auditor handles structured logging of security-relevant events.
pub struct GatewayAuditor {
    enabled: bool,
    log_file: Option<Mutex<File>>,
}

impl GatewayAuditor {
    /// Create a new auditor that logs to the structured log target.
    pub fn new() -> Self {
        Self {
            enabled: true,
            log_file: None,
        }
    }

    /// Create a new auditor with file output enabled.
    #[allow(dead_code)]
    pub fn with_file(audit_file_path: PathBuf) -> Result<Self, String> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&audit_file_path)
            .map_err(|e| format!("Failed to open audit log file: {}", e))?;

        Ok(Self {
            enabled: true,
            log_file: Some(Mutex::new(file)),
        })
    }

    /// Log a gateway audit event.
    pub fn log(&self, event: GatewayAuditEvent) {
        if !self.enabled {
            return;
        }

        // Always log to structured log target for monitoring tools
        let json = serde_json::to_string(&event).unwrap_or_else(|_| {
            format!(
                r#"{{"error":"serialization_failed","event_type":"{:?}"}}"#,
                event.event_type
            )
        });

        log::info!(target: "gateway_audit", "{}", json);

        // Additionally write to file if configured
        if let Some(file_mutex) = &self.log_file {
            if let Ok(mut file) = file_mutex.lock() {
                let _ = writeln!(file, "{}", json);
                let _ = file.flush();
            }
        }
    }

    /// Disable audit logging (for testing or temporary suspension).
    #[allow(dead_code)]
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Enable audit logging.
    #[allow(dead_code)]
    pub fn enable(&mut self) {
        self.enabled = true;
    }
}

impl Default for GatewayAuditor {
    fn default() -> Self {
        Self::new()
    }
}

/// Global gateway auditor instance for application-wide audit logging.
static GLOBAL_AUDITOR: LazyLock<Mutex<GatewayAuditor>> =
    LazyLock::new(|| Mutex::new(GatewayAuditor::new()));

/// Log a gateway audit event using the global auditor.
pub fn log_audit(event: GatewayAuditEvent) {
    if let Ok(auditor) = GLOBAL_AUDITOR.lock() {
        auditor.log(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_audit_event_correctly() {
        let event = GatewayAuditEvent {
            timestamp_ms: 1234567890,
            event_type: GatewayAuditEventType::ProfileCreated,
            profile_id: "gateway-test".to_string(),
            actor: Some("admin".to_string()),
            details: serde_json::json!({"name": "Test Profile"}),
            result: AuditResult::Success,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("profile_created"));
        assert!(json.contains("gateway-test"));
    }

    #[test]
    fn auditor_logs_without_panic() {
        let auditor = GatewayAuditor::new();
        let event = GatewayAuditEvent {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            event_type: GatewayAuditEventType::RequestForwarded,
            profile_id: "test".to_string(),
            actor: None,
            details: serde_json::json!({}),
            result: AuditResult::Success,
        };

        auditor.log(event);
        // Should not panic
    }
}
