//! Qoder Work / Qoder Work CN main.log 读取模块
//!
//! 扫描 `~/Library/Application Support/<app_dir>/logs/<ts>/main.log`
//! 每个 `<ts>/main.log` 文件对应一个虚拟会话，解析 SSE message_delta 事件。
//!
//! 日志行格式：
//!   `[ISO_TIMESTAMP] [LEVEL] [SDK] [QueryHandler] Received message: stream_event {...json...}`
//!
//! 提取其中 `event.type == "message_delta"` 的 `input_tokens` 和 `output_tokens`。

use super::meta::{LocalRequestRecord, SessionMeta};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const QODER_WORK_SOURCE_KIND: &str = "qoder_work_mainlog";

#[derive(Debug, Clone)]
pub(crate) struct QoderWorkSessionData {
    pub meta: SessionMeta,
    pub requests: Vec<LocalRequestRecord>,
    pub fingerprint: u64,
    pub source_locator: String,
}

/// 扫描 QoderWork（国际版）全部 main.log 虚拟会话
pub(crate) fn scan_qoder_work_sessions() -> Vec<QoderWorkSessionData> {
    scan_qoder_work_sessions_for("QoderWork", super::constants::TOOL_QODER_WORK)
}

/// 扫描 QoderWork CN（中国版）全部 main.log 虚拟会话
pub(crate) fn scan_qoder_work_cn_sessions() -> Vec<QoderWorkSessionData> {
    scan_qoder_work_sessions_for("QoderWork CN", super::constants::TOOL_QODER_WORK_CN)
}

/// 通用扫描函数，通过 `app_dir` 参数区分国际版与 CN 版
pub(crate) fn scan_qoder_work_sessions_for(app_dir: &str, tool: &str) -> Vec<QoderWorkSessionData> {
    let Some(logs_root) = find_qoder_work_logs_root(app_dir) else {
        return Vec::new();
    };

    let Ok(entries) = fs::read_dir(&logs_root) else {
        return Vec::new();
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let ts_dir = entry.path();
        if !ts_dir.is_dir() {
            continue;
        }
        let ts_name = ts_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if ts_name.is_empty() || !ts_name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let log_path = ts_dir.join("main.log");
        if !log_path.exists() {
            continue;
        }

        if let Some(session) = parse_work_session(&log_path, &ts_name, tool) {
            sessions.push(session);
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.meta.last_modified));
    sessions
}

fn parse_work_session(log_path: &Path, ts_name: &str, tool: &str) -> Option<QoderWorkSessionData> {
    let metadata = fs::metadata(log_path).ok()?;
    let file_size = metadata.len();
    let last_modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let fingerprint = compute_work_session_fingerprint(log_path, file_size, last_modified);

    // session_id: "<tool>::<ts_name>"
    let session_id = format!("{}::{}", tool, ts_name);
    let source_locator = log_path.to_string_lossy().to_string();

    let requests = parse_work_log_requests(log_path, &session_id, tool);

    // 汇总 token 统计
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_cache_create = 0u64;
    let mut total_cache_read = 0u64;
    let mut earliest_ts: Option<i64> = None;
    let mut latest_ts: Option<i64> = None;
    let mut models_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for r in &requests {
        total_input += r.input_tokens;
        total_output += r.output_tokens;
        total_cache_create += r.cache_create_tokens;
        total_cache_read += r.cache_read_tokens;
        if !r.model.is_empty() && r.model != "unknown" {
            models_set.insert(r.model.clone());
        }
        earliest_ts = Some(
            earliest_ts
                .map(|c| c.min(r.timestamp))
                .unwrap_or(r.timestamp),
        );
        latest_ts = Some(latest_ts.map(|c| c.max(r.timestamp)).unwrap_or(r.timestamp));
    }

    // 从 ts_name（如 202606111739）派生人类可读时间作为 session_name
    let session_name = format_ts_name(ts_name);

    let meta = SessionMeta {
        session_id: session_id.clone(),
        tool: tool.to_string(),
        cwd: None,
        project_name: None,
        topic: None,
        last_prompt: None,
        session_name: Some(session_name),
        file_path: source_locator.clone(),
        file_size,
        last_modified,
        total_input_tokens: total_input,
        total_output_tokens: total_output,
        total_cache_create_tokens: total_cache_create,
        total_cache_read_tokens: total_cache_read,
        models: models_set.into_iter().collect(),
        message_count: requests.len() as u64,
        start_time: earliest_ts.unwrap_or(last_modified),
        end_time: latest_ts.unwrap_or(last_modified),
        source: QODER_WORK_SOURCE_KIND.to_string(),
        message_ids: requests.iter().map(|r| r.message_id.clone()).collect(),
        scope: None,
        explicit_estimated_cost: None,
    };

    Some(QoderWorkSessionData {
        meta,
        requests,
        fingerprint,
        source_locator,
    })
}

fn parse_work_log_requests(
    log_path: &Path,
    session_id: &str,
    tool: &str,
) -> Vec<LocalRequestRecord> {
    let Ok(file) = fs::File::open(log_path) else {
        return Vec::new();
    };

    let reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut line_idx: u64 = 0;

    for line in reader.lines().map_while(Result::ok) {
        line_idx += 1;
        if let Some(record) = parse_work_log_line(&line, session_id, tool, line_idx) {
            records.push(record);
        }
    }

    records
}

fn parse_work_log_line(
    line: &str,
    session_id: &str,
    tool: &str,
    line_idx: u64,
) -> Option<LocalRequestRecord> {
    // 快速过滤：只处理包含关键词的行
    if !line.contains("QueryHandler") || !line.contains("stream_event") {
        return None;
    }

    // 提取时间戳：行首 `[ISO_TIMESTAMP]`
    let timestamp = extract_log_line_timestamp(line).unwrap_or(0);

    // 提取 "stream_event " 后面的 JSON
    let json_str = extract_stream_event_json(line)?;
    let json: serde_json::Value = serde_json::from_str(json_str).ok()?;

    // 校验 event.type == "message_delta"
    let event = json.get("event")?;
    if event.get("type").and_then(|v| v.as_str()) != Some("message_delta") {
        return None;
    }

    let usage = event.get("usage")?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // 当 Qoder Work SDK 日志包含 cache 字段时读取；当前版本通常为 0
    let cache_create_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let total = input_tokens + output_tokens + cache_create_tokens + cache_read_tokens;
    if total == 0 {
        return None;
    }

    // 稳定去重键：session_id + 行索引 + 时间戳 + token 总量
    let message_id = format!("work_ln{}_ts{}_tok{}", line_idx, timestamp, total);

    Some(LocalRequestRecord {
        session_id: session_id.to_string(),
        tool: tool.to_string(),
        timestamp,
        message_id,
        input_tokens,
        output_tokens,
        reasoning_tokens: 0,
        cache_create_tokens,
        cache_read_tokens,
        total_tokens: total,
        request_count: 1,
        model: "unknown".to_string(),
        is_subagent: false,
        request_key: None,
        explicit_estimated_cost: None,
        source_file_present: None,
    })
}

/// 从日志行首提取 ISO 时间戳并转换为 Unix 秒
/// 格式：`[2026-05-28T02:15:05.241Z] ...`
fn extract_log_line_timestamp(line: &str) -> Option<i64> {
    if !line.starts_with('[') {
        return None;
    }
    let end = line.find(']')?;
    let ts_str = &line[1..end];
    chrono::DateTime::parse_from_rfc3339(ts_str)
        .ok()
        .map(|dt| dt.timestamp())
}

/// 提取 "stream_event " 之后第一个 `{` 到最后一个 `}` 的 JSON 子串
fn extract_stream_event_json(line: &str) -> Option<&str> {
    let marker = "stream_event ";
    let marker_pos = line.find(marker)?;
    let after_marker = &line[marker_pos + marker.len()..];
    let json_start = after_marker.find('{')?;
    let json_end = after_marker.rfind('}')?;
    if json_start > json_end {
        return None;
    }
    Some(&after_marker[json_start..=json_end])
}

/// 将 `YYYYMMDDHHNN` 格式转成可读字符串 "YYYY-MM-DD HH:MM"
fn format_ts_name(ts: &str) -> String {
    if ts.len() == 12 {
        format!(
            "{}-{}-{} {}:{}",
            &ts[0..4],
            &ts[4..6],
            &ts[6..8],
            &ts[8..10],
            &ts[10..12]
        )
    } else {
        ts.to_string()
    }
}

fn find_qoder_work_logs_root(app_dir: &str) -> Option<PathBuf> {
    dirs::data_dir()
        .map(|d| d.join(app_dir).join("logs"))
        .filter(|p| p.exists())
}

fn compute_work_session_fingerprint(log_path: &Path, file_size: u64, last_modified: i64) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    log_path.to_string_lossy().hash(&mut hasher);
    file_size.hash(&mut hasher);
    last_modified.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parse_work_log_line_extracts_message_delta() {
        let line = r#"[2026-05-28T02:15:05.241Z] [INFO] [SDK] [QueryHandler] Received message: stream_event {"event":{"type":"message_delta","usage":{"input_tokens":100,"output_tokens":50}}}"#;
        let record = parse_work_log_line(line, "qoder_work::202605281557", "qoder_work", 1);
        assert!(record.is_some());
        let r = record.unwrap();
        assert_eq!(r.input_tokens, 100);
        assert_eq!(r.output_tokens, 50);
        assert_eq!(r.total_tokens, 150);
        assert_eq!(r.cache_create_tokens, 0);
        assert_eq!(r.cache_read_tokens, 0);
    }

    #[test]
    fn parse_work_log_line_ignores_non_delta_events() {
        let line = r#"[2026-05-28T02:15:04.000Z] [INFO] [SDK] [QueryHandler] Received message: stream_event {"event":{"type":"message_start","message":{"id":"m1"}}}"#;
        assert!(parse_work_log_line(line, "s", "t", 1).is_none());
    }

    #[test]
    fn parse_work_log_line_ignores_zero_token_lines() {
        let line = r#"[2026-05-28T02:15:05.000Z] [INFO] [SDK] [QueryHandler] Received message: stream_event {"event":{"type":"message_delta","usage":{"input_tokens":0,"output_tokens":0}}}"#;
        assert!(parse_work_log_line(line, "s", "t", 1).is_none());
    }

    #[test]
    fn parse_work_log_line_skips_non_queryhandler() {
        let line = "[2026-05-28T02:15:05.000Z] [INFO] [DB] Creating table";
        assert!(parse_work_log_line(line, "s", "t", 1).is_none());
    }

    #[test]
    fn format_ts_name_formats_correctly() {
        assert_eq!(format_ts_name("202606111739"), "2026-06-11 17:39");
        assert_eq!(format_ts_name("short"), "short");
    }

    #[test]
    fn parse_work_session_from_temp_log() {
        let tmpdir = tempdir().unwrap();
        let ts_dir = tmpdir.path().join("202605281557");
        fs::create_dir_all(&ts_dir).unwrap();
        let log_path = ts_dir.join("main.log");
        let mut f = fs::File::create(&log_path).unwrap();

        writeln!(f, "[2026-05-28T07:39:05.000Z] [INFO] [DB] Startup").unwrap();
        writeln!(
            f,
            r#"[2026-05-28T07:39:10.241Z] [INFO] [SDK] [QueryHandler] Received message: stream_event {{"event":{{"type":"message_delta","usage":{{"input_tokens":200,"output_tokens":80}}}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"[2026-05-28T07:40:00.000Z] [INFO] [SDK] [QueryHandler] Received message: stream_event {{"event":{{"type":"message_delta","usage":{{"input_tokens":50,"output_tokens":30}}}}}}"#
        )
        .unwrap();

        let session = parse_work_session(&log_path, "202605281557", "qoder_work").unwrap();
        assert_eq!(session.requests.len(), 2);
        assert_eq!(session.meta.total_input_tokens, 250);
        assert_eq!(session.meta.total_output_tokens, 110);
        assert_eq!(
            session.meta.session_name.as_deref(),
            Some("2026-05-28 15:57")
        );
        assert_eq!(session.meta.message_count, 2);
    }
}
