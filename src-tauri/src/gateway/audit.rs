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
        // 解析回对象逐字段断言，验证 camelCase 序列化与字段齐全（而非 contains 弱断言）。
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["eventType"], "profile_created");
        assert_eq!(value["profileId"], "gateway-test");
        assert_eq!(value["actor"], "admin");
        assert_eq!(value["result"], "success");
        assert_eq!(value["details"]["name"], "Test Profile");
        assert!(value["timestampMs"].is_number());
    }

    #[test]
    fn auditor_writes_structured_json_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("audit.log");
        let auditor = GatewayAuditor::with_file(log_path.clone()).expect("auditor with file");
        let event = GatewayAuditEvent {
            timestamp_ms: 1_234_567_890,
            event_type: GatewayAuditEventType::UpstreamKeyAdded,
            profile_id: "profile-1".to_string(),
            actor: Some("admin".to_string()),
            details: serde_json::json!({ "key_id": "k1" }),
            result: AuditResult::Failure {
                reason: "bad key".to_string(),
            },
        };

        auditor.log(event);
        drop(auditor); // 关闭文件句柄确保内容落盘

        let content = std::fs::read_to_string(&log_path).expect("audit file written");
        let parsed: serde_json::Value =
            serde_json::from_str(content.trim()).expect("audit line is valid JSON");
        assert_eq!(parsed["eventType"].as_str(), Some("upstream_key_added"));
        assert_eq!(parsed["profileId"].as_str(), Some("profile-1"));
        assert_eq!(parsed["actor"].as_str(), Some("admin"));
        assert_eq!(parsed["timestampMs"].as_i64(), Some(1_234_567_890));
        assert_eq!(parsed["details"]["key_id"].as_str(), Some("k1"));
        assert_eq!(
            parsed["result"],
            serde_json::json!({ "failure": { "reason": "bad key" } })
        );
    }

    #[test]
    fn auditor_disabled_does_not_write_file() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("audit-disabled.log");
        let mut auditor = GatewayAuditor::with_file(log_path.clone()).expect("auditor with file");
        auditor.disable();
        auditor.log(GatewayAuditEvent {
            timestamp_ms: 1,
            event_type: GatewayAuditEventType::RequestForwarded,
            profile_id: "test".to_string(),
            actor: None,
            details: serde_json::json!({}),
            result: AuditResult::Success,
        });
        drop(auditor);

        assert_eq!(
            std::fs::read_to_string(&log_path).expect("audit file exists"),
            ""
        );
    }
}
