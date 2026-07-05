//! Qoder Work / Qoder Work CN 本地会话读取模块
//!
//! 两路数据来源合并到同一个 `tool_id`（`qoder_work` / `qoder_work_cn`）下：
//!
//! 1. Electron 主进程日志：`~/Library/Application Support/<app_dir>/logs/<ts>/main.log`，
//!    每个 `<ts>/main.log` 文件对应一个虚拟会话，解析 SSE message_delta 事件，日志行格式：
//!    `[ISO_TIMESTAMP] [LEVEL] [SDK] [QueryHandler] Received message: stream_event {...json...}`，
//!    提取其中 `event.type == "message_delta"` 的 `input_tokens` 和 `output_tokens`。
//!    注意：QoderWork CN 客户端升级到 0.9.9 后这条链路已经失效（新版不再把 usage JSON
//!    内联进 main.log），只对旧版本/国际版仍然有效。
//!
//! 2. CLI 子进程 transcript：`~/<cli_root_dir>/projects/<encoded-cwd>/<sessionId>.jsonl`
//!    （`cli_root_dir` 为 `.qoderwork` 或 `.qoderworkcn`），Claude-Code 风格逐行 JSON，
//!    一个文件对应一个真实会话。已用真实数据核实：这批 transcript 里 `message.usage` 的
//!    token 字段目前恒为 0（国际版、CN 版皆如此，是 CLI transcript 导出逻辑本身的特性，
//!    与 Electron 日志链路是否失效无关），但每条 `type=="assistant"` 记录对应一次真实的
//!    模型调用，可以补齐请求次数、模型分布、会话时间线——当 Electron 日志链路失效时
//!    （如新版 CN 客户端），这是唯一还能拿到的本地信号。若未来 Qoder 官方修复了 usage
//!    字段，这里的解析逻辑无需改动，会自动读到非零值。

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{
    extract_model, extract_project_name, extract_timestamp, extract_u64_by_keys, truncate_string,
};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

const QODER_WORK_SOURCE_KIND: &str = "qoder_work_mainlog";
const QODER_WORK_CLI_SOURCE_KIND: &str = "qoder_work_cli_jsonl";

pub(super) struct QoderWorkSource {
    tool: &'static str,
    app_dir: &'static str,
    cli_root_dir: &'static str,
    cache: OnceLock<Mutex<std::collections::HashMap<String, QoderWorkSessionData>>>,
}

#[derive(Debug, Clone)]
pub(crate) struct QoderWorkSessionData {
    pub meta: SessionMeta,
    pub requests: Vec<LocalRequestRecord>,
    pub fingerprint: u64,
    pub source_locator: String,
}

impl QoderWorkSource {
    pub(super) const fn new(
        tool: &'static str,
        app_dir: &'static str,
        cli_root_dir: &'static str,
    ) -> Self {
        Self {
            tool,
            app_dir,
            cli_root_dir,
            cache: OnceLock::new(),
        }
    }

    fn cache(&self) -> &Mutex<std::collections::HashMap<String, QoderWorkSessionData>> {
        self.cache
            .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
    }
}

impl SessionSource for QoderWorkSource {
    fn tool_id(&self) -> &'static str {
        self.tool
    }

    fn scan(&self) -> SourceSnapshot {
        let mut scanned = scan_qoder_work_sessions_for(self.app_dir, self.tool);
        scanned.extend(scan_qoder_work_cli_sessions_for(
            self.cli_root_dir,
            self.tool,
        ));
        let scan_fingerprint = compute_qoder_work_scan_fingerprint(&scanned);
        let sessions = scanned
            .iter()
            .map(|session| SessionFile {
                session_id: session.meta.session_id.clone(),
                tool: session.meta.tool.clone(),
                project_path: session.meta.project_name.clone().unwrap_or_default(),
                file_path: session.source_locator.clone(),
                transcript_paths: vec![session.meta.file_path.clone()],
                file_size: session.meta.file_size,
                last_modified: session.meta.last_modified,
                fingerprint: session.fingerprint,
            })
            .collect::<Vec<_>>();

        let mut cache = self.cache().lock().unwrap_or_else(|err| err.into_inner());
        cache.clear();
        cache.extend(
            scanned
                .into_iter()
                .map(|session| (session.meta.session_id.clone(), session)),
        );
        drop(cache);

        SourceSnapshot {
            source_id: self.tool_id(),
            update_mode: SourceUpdateMode::ReplaceAll,
            sessions,
            scan_fingerprint,
        }
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        let cache = self.cache().lock().unwrap_or_else(|err| err.into_inner());
        let parsed = cache
            .get(&session.session_id)
            .cloned()
            .ok_or_else(|| format!("qoder work session not found: {}", session.session_id))?;

        Ok(ParsedSessionData {
            meta: parsed.meta,
            requests: parsed.requests,
        })
    }
}

/// 扫描 QoderWork（国际版）全部会话：main.log 虚拟会话 + CLI transcript 真实会话
///
/// 注意：这个函数被 `scanner_sync.rs::sync_from_scanner()` 用来把数据写入
/// SQLite（UI 实际读取的持久化层）。必须和 `QoderWorkSource::scan()`（供内存
/// session 缓存使用的 `SessionSource` trait 实现）保持同样的"两路来源合并"
/// 逻辑，否则 CLI transcript 数据只会出现在内存缓存里，永远同步不进数据库。
pub(crate) fn scan_qoder_work_sessions() -> Vec<QoderWorkSessionData> {
    let mut sessions = scan_qoder_work_sessions_for("QoderWork", super::constants::TOOL_QODER_WORK);
    sessions.extend(scan_qoder_work_cli_sessions_for(
        ".qoderwork",
        super::constants::TOOL_QODER_WORK,
    ));
    sessions
}

/// 扫描 QoderWork CN（中国版）全部会话：main.log 虚拟会话 + CLI transcript 真实会话
///
/// 同上，必须和 `QoderWorkSource::scan()` 保持一致的合并逻辑。
pub(crate) fn scan_qoder_work_cn_sessions() -> Vec<QoderWorkSessionData> {
    let mut sessions =
        scan_qoder_work_sessions_for("QoderWork CN", super::constants::TOOL_QODER_WORK_CN);
    sessions.extend(scan_qoder_work_cli_sessions_for(
        ".qoderworkcn",
        super::constants::TOOL_QODER_WORK_CN,
    ));
    sessions
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

// ============================================================================
// CLI transcript 数据源：~/<cli_root_dir>/projects/<encoded-cwd>/<sessionId>.jsonl
// ============================================================================

/// 扫描 QoderWork CLI 子进程 transcript 目录下全部会话
///
/// `cli_root_dir` 形如 `.qoderwork` / `.qoderworkcn`（相对于 home 目录）。
fn scan_qoder_work_cli_sessions_for(cli_root_dir: &str, tool: &str) -> Vec<QoderWorkSessionData> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let projects_root = home.join(cli_root_dir).join("projects");
    if !projects_root.exists() {
        return Vec::new();
    }

    let Ok(project_entries) = fs::read_dir(&projects_root) else {
        return Vec::new();
    };

    let mut sessions = Vec::new();
    for entry in project_entries.flatten() {
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let encoded_cwd = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if encoded_cwd.is_empty() {
            continue;
        }

        for jsonl_path in collect_qoder_work_cli_jsonl_files(&project_dir) {
            let is_subagent_file = is_qoder_work_subagent_path(&jsonl_path);

            let metadata = fs::metadata(&jsonl_path).ok();
            let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            // 跳过明显为空的会话文件（对齐 qoder_cli_reader.rs 的同款过滤）
            if file_size < 50 {
                continue;
            }

            let session_key = qoder_work_cli_session_key(&project_dir, &jsonl_path);
            if session_key.is_empty() {
                continue;
            }

            if let Some(session) = parse_qoder_work_cli_session_file(
                &jsonl_path,
                &session_key,
                &encoded_cwd,
                tool,
                is_subagent_file,
                metadata,
            ) {
                sessions.push(session);
            }
        }
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.meta.last_modified));
    sessions
}

fn parse_qoder_work_cli_session_file(
    jsonl_path: &Path,
    session_key: &str,
    encoded_cwd: &str,
    tool: &str,
    is_subagent_file: bool,
    metadata: Option<fs::Metadata>,
) -> Option<QoderWorkSessionData> {
    let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
    let last_modified = metadata
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let Ok(file) = fs::File::open(jsonl_path) else {
        return None;
    };

    // session_id 加 "::cli::" 命名空间，避免和 Electron main.log 来源的
    // "<tool>::<ts_name>" 会话 id 空间冲突。
    let session_id = format!("{}::cli::{}", tool, session_key);
    let source_locator = jsonl_path.to_string_lossy().to_string();

    let reader = BufReader::new(file);
    let mut by_message_id: HashMap<String, LocalRequestRecord> = HashMap::new();
    let mut cwd: Option<String> = None;
    let mut topic: Option<String> = None;
    let mut last_prompt: Option<String> = None;
    let mut earliest_ts: Option<i64> = None;
    let mut latest_ts: Option<i64> = None;
    let mut line_idx: u64 = 0;

    for line in reader.lines().map_while(Result::ok) {
        line_idx += 1;
        let Ok(json) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if cwd.is_none() {
            if let Some(c) = json.get("cwd").and_then(|v| v.as_str()) {
                if !c.is_empty() {
                    cwd = Some(c.to_string());
                }
            }
        }

        if let Some(ts) = extract_timestamp(&json) {
            earliest_ts = Some(earliest_ts.map(|c| c.min(ts)).unwrap_or(ts));
            latest_ts = Some(latest_ts.map(|c| c.max(ts)).unwrap_or(ts));
        }

        let entry_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match entry_type {
            "user" => {
                if let Some(text) =
                    extract_qoder_work_cli_user_text(json.get("message").unwrap_or(&Value::Null))
                {
                    if topic.is_none() {
                        topic = Some(truncate_string(&text, 50));
                    }
                    last_prompt = Some(truncate_string(&text, 100));
                }
            }
            "assistant" => {
                if let Some(record) = extract_qoder_work_cli_request_record(
                    &json,
                    &session_id,
                    tool,
                    line_idx,
                    is_subagent_file,
                ) {
                    // 防御性去重：同 message_id 出现多次时保留 total_tokens 更大的一条
                    by_message_id
                        .entry(record.message_id.clone())
                        .and_modify(|existing| {
                            if record.total_tokens > existing.total_tokens {
                                *existing = record.clone();
                            }
                        })
                        .or_insert(record);
                }
            }
            _ => {}
        }
    }

    let mut requests: Vec<LocalRequestRecord> = by_message_id.into_values().collect();
    requests.sort_by_key(|r| r.timestamp);

    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_cache_create = 0u64;
    let mut total_cache_read = 0u64;
    let mut models_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for r in &requests {
        total_input += r.input_tokens;
        total_output += r.output_tokens;
        total_cache_create += r.cache_create_tokens;
        total_cache_read += r.cache_read_tokens;
        if !r.model.is_empty() && r.model != "unknown" {
            models_set.insert(r.model.clone());
        }
    }

    let project_name = cwd
        .as_deref()
        .and_then(extract_project_name)
        .or_else(|| decode_qoder_work_cli_project_name(encoded_cwd));

    let start_time = earliest_ts.unwrap_or(last_modified);
    let end_time = latest_ts.unwrap_or(last_modified);
    let session_name = format_cli_session_name(start_time);

    let meta = SessionMeta {
        session_id: session_id.clone(),
        tool: tool.to_string(),
        cwd,
        project_name,
        topic,
        last_prompt,
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
        start_time,
        end_time,
        source: QODER_WORK_CLI_SOURCE_KIND.to_string(),
        message_ids: requests.iter().map(|r| r.message_id.clone()).collect(),
        scope: None,
        explicit_estimated_cost: None,
    };

    let fingerprint = compute_work_session_fingerprint(jsonl_path, file_size, last_modified);

    Some(QoderWorkSessionData {
        meta,
        requests,
        fingerprint,
        source_locator,
    })
}

/// 从一条 `type=="assistant"` 记录提取 `LocalRequestRecord`
///
/// 与 `qoder_cli_reader.rs::extract_cli_request_record` 的关键差异：即使全部
/// token 字段为 0（QoderWork CLI transcript 目前恒为 0）也保留该记录，
/// 因为这条数据源的价值在于请求次数/模型分布，不能因为 total==0 就整条丢弃。
fn extract_qoder_work_cli_request_record(
    json: &Value,
    session_id: &str,
    tool: &str,
    line_idx: u64,
    default_is_subagent: bool,
) -> Option<LocalRequestRecord> {
    let message = json.get("message")?;
    if message.get("role").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }

    let timestamp = extract_timestamp(json).unwrap_or(0);
    let model = extract_model(json).unwrap_or_else(|| "unknown".to_string());
    let is_subagent = json
        .get("isSidechain")
        .and_then(|value| value.as_bool())
        .unwrap_or(default_is_subagent);

    let usage = message.get("usage");
    let (input_tokens, output_tokens, cache_create_tokens, cache_read_tokens) = usage
        .map(|u| {
            (
                extract_u64_by_keys(u, &["input_tokens", "inputTokens"]),
                extract_u64_by_keys(u, &["output_tokens", "outputTokens"]),
                extract_u64_by_keys(
                    u,
                    &["cache_creation_input_tokens", "cacheCreationInputTokens"],
                ),
                extract_u64_by_keys(u, &["cache_read_input_tokens", "cacheReadInputTokens"]),
            )
        })
        .unwrap_or((0, 0, 0, 0));
    let total_tokens = input_tokens + output_tokens + cache_create_tokens + cache_read_tokens;

    let message_id = message
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            json.get("uuid")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| format!("qwcli_ln{}_ts{}", line_idx, timestamp));

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
        total_tokens,
        request_count: 1,
        model,
        is_subagent,
        request_key: None,
        explicit_estimated_cost: None,
        source_file_present: None,
    })
}

fn collect_qoder_work_cli_jsonl_files(project_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![project_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("jsonl"))
                == Some(true)
            {
                files.push(path);
            }
        }
    }

    files.sort();
    files
}

fn qoder_work_cli_session_key(project_dir: &Path, jsonl_path: &Path) -> String {
    let relative = jsonl_path.strip_prefix(project_dir).unwrap_or(jsonl_path);
    let mut parts: Vec<String> = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(|part| part.trim_end_matches(".jsonl").to_string())
        .filter(|part| !part.is_empty())
        .collect();

    if parts.is_empty() {
        parts.push(
            jsonl_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string(),
        );
    }

    parts.join("::")
}

fn is_qoder_work_subagent_path(jsonl_path: &Path) -> bool {
    jsonl_path
        .components()
        .any(|component| component.as_os_str().to_str() == Some("subagents"))
}

/// 从用户消息的 `content` 中提取真正的用户输入文本
///
/// `content` 为数组时，过滤掉 `tool_result` 块和被
/// `<system-reminder>...</system-reminder>` 包裹的注入文本，取剩余文本块中
/// 最后一个（真正的用户提问通常在数组末尾，前面可能混杂环境说明等注入内容）。
fn extract_qoder_work_cli_user_text(message: &Value) -> Option<String> {
    let content = message.get("content")?;

    let candidate = if let Some(text) = content.as_str() {
        Some(text.to_string())
    } else if let Some(items) = content.as_array() {
        items
            .iter()
            .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("text"))
            .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
            .rfind(|text| !is_system_reminder_text(text))
            .map(|s| s.to_string())
    } else {
        None
    };

    candidate
        .map(|s| s.trim().to_string())
        .filter(|s| s.chars().count() >= 3)
}

fn is_system_reminder_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("<system-reminder>")
}

/// 从 encoded 目录名（如 `-Users-foo-bar-myproject`）退化提取项目名
///
/// 与 `qoder_cli_reader.rs::decode_qoder_project_name` 逻辑一致：这只是一个
/// 简化的兜底方案，不还原完整路径。QoderWork CLI 的 `cwd` 字段指向的是沙箱
/// 工作目录而非用户真实项目路径，所以多数情况下会走到这个兜底分支。
fn decode_qoder_work_cli_project_name(encoded_dir: &str) -> Option<String> {
    encoded_dir
        .split('-')
        .rfind(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 将 CLI 会话的起始时间格式化为可读字符串 "YYYY-MM-DD HH:MM"
fn format_cli_session_name(start_time: i64) -> String {
    chrono::DateTime::from_timestamp(start_time, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| start_time.to_string())
}

fn compute_work_session_fingerprint(log_path: &Path, file_size: u64, last_modified: i64) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    log_path.to_string_lossy().hash(&mut hasher);
    file_size.hash(&mut hasher);
    last_modified.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn compute_qoder_work_scan_fingerprint(sessions: &[QoderWorkSessionData]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for session in sessions {
        session.meta.session_id.hash(&mut hasher);
        session.fingerprint.hash(&mut hasher);
    }
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

    // ------------------------------------------------------------------
    // CLI transcript 数据源测试
    // ------------------------------------------------------------------

    #[test]
    fn extract_qoder_work_cli_user_text_skips_reminder_and_tool_result() {
        let message: Value = serde_json::from_str(
            r#"{
                "content": [
                    {"type": "text", "text": "<system-reminder>\nUser environment — Timezone: Asia/Shanghai\n</system-reminder>"},
                    {"type": "text", "text": "请检查一下我的博客前端页面"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(
            extract_qoder_work_cli_user_text(&message).as_deref(),
            Some("请检查一下我的博客前端页面")
        );

        let tool_result_only: Value =
            serde_json::from_str(r#"{"content": [{"type": "tool_result", "content": "ok"}]}"#)
                .unwrap();
        assert!(extract_qoder_work_cli_user_text(&tool_result_only).is_none());
    }

    #[test]
    fn extract_qoder_work_cli_request_record_keeps_zero_token_requests() {
        // QoderWork CLI transcript 目前的 usage 字段恒为 0，但仍应产出一条请求记录，
        // 不能因为 total_tokens==0 就被判定为"没有发生请求"而丢弃。
        let line: Value = serde_json::from_str(
            r#"{
                "uuid": "c7cf44c9-e80a-4f52-9a59-f44cbf780281",
                "timestamp": "2026-06-15T15:55:50.644Z",
                "message": {
                    "role": "assistant",
                    "id": "882d2b3d-0bd5-4cb5-a3f6-a5ea3dd0cb36",
                    "model": "qmodel_latest",
                    "usage": {
                        "input_tokens": 0, "output_tokens": 0,
                        "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0
                    }
                }
            }"#,
        )
        .unwrap();
        let record = extract_qoder_work_cli_request_record(
            &line,
            "qoder_work_cn::cli::s1",
            "qoder_work_cn",
            1,
            false,
        )
        .expect("zero-token assistant turn should still produce a request record");
        assert_eq!(record.total_tokens, 0);
        assert_eq!(record.request_count, 1);
        assert_eq!(record.model, "qmodel_latest");
        assert_eq!(record.message_id, "882d2b3d-0bd5-4cb5-a3f6-a5ea3dd0cb36");
    }

    #[test]
    fn extract_qoder_work_cli_request_record_preserves_internal_model_ids() {
        let line: Value = serde_json::from_str(
            r#"{
                "uuid": "u1", "timestamp": "2026-06-15T15:55:50.644Z",
                "message": {"role": "assistant", "id": "m1", "model": "gm51model"}
            }"#,
        )
        .unwrap();
        let record =
            extract_qoder_work_cli_request_record(&line, "s", "qoder_work_cn", 1, false).unwrap();
        assert_eq!(record.model, "gm51model");
    }

    #[test]
    fn extract_qoder_work_cli_request_record_filters_synthetic_model() {
        let line: Value = serde_json::from_str(
            r#"{
                "uuid": "u1", "timestamp": "2026-06-15T15:55:50.644Z",
                "message": {"role": "assistant", "id": "m1", "model": "<synthetic>"}
            }"#,
        )
        .unwrap();
        let record =
            extract_qoder_work_cli_request_record(&line, "s", "qoder_work", 1, false).unwrap();
        assert_eq!(record.model, "unknown");
    }

    #[test]
    fn extract_qoder_work_cli_request_record_ignores_non_assistant() {
        let line: Value =
            serde_json::from_str(r#"{"uuid": "u1", "message": {"role": "user", "id": "m1"}}"#)
                .unwrap();
        assert!(
            extract_qoder_work_cli_request_record(&line, "s", "qoder_work", 1, false).is_none()
        );
    }

    #[test]
    fn extract_qoder_work_cli_request_record_marks_subagents() {
        let line: Value = serde_json::from_str(
            r#"{
                "uuid": "u1", "isSidechain": true,
                "message": {"role": "assistant", "id": "m1", "model": "gm51model"}
            }"#,
        )
        .unwrap();
        let record =
            extract_qoder_work_cli_request_record(&line, "s", "qoder_work_cn", 1, false).unwrap();
        assert!(record.is_subagent);
    }

    #[test]
    fn decode_qoder_work_cli_project_name_takes_last_segment() {
        assert_eq!(
            decode_qoder_work_cli_project_name(
                "-Users-smileslove--qoderworkcn-workspace-mqfeaikxu2gy5q6x"
            ),
            Some("mqfeaikxu2gy5q6x".to_string())
        );
    }

    #[test]
    fn parse_qoder_work_cli_session_file_from_temp_transcript() {
        let tmpdir = tempdir().unwrap();
        let project_dir = tmpdir
            .path()
            .join("-Users-test--qoderworkcn-workspace-abc123");
        fs::create_dir_all(&project_dir).unwrap();
        let jsonl_path = project_dir.join("771bc3d8-0def-4c8c-a9b1-f674e93bd981.jsonl");
        let mut f = fs::File::create(&jsonl_path).unwrap();

        writeln!(
            f,
            r#"{{"uuid":"u0","type":"user","timestamp":"2026-06-15T15:55:50.644Z","cwd":"/Users/test/.qoderworkcn/workspace/abc123","message":{{"role":"user","content":[{{"type":"text","text":"<system-reminder>env stuff</system-reminder>"}},{{"type":"text","text":"请检查前端页面"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"uuid":"u1","type":"assistant","timestamp":"2026-06-15T15:56:00.000Z","message":{{"role":"assistant","id":"a1","model":"qmodel_latest","usage":{{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"uuid":"u2","type":"user","message":{{"role":"user","content":[{{"type":"tool_result","content":"ok"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"uuid":"u3","type":"assistant","timestamp":"2026-06-15T15:57:00.000Z","message":{{"role":"assistant","id":"a2","model":"qmodel_latest","usage":{{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
        )
        .unwrap();
        // padding to pass the >=50 byte size gate used at the directory-scan level (not exercised here directly)
        writeln!(f, "{}", "x".repeat(60)).unwrap();
        drop(f);

        let metadata = fs::metadata(&jsonl_path).ok();
        let session = parse_qoder_work_cli_session_file(
            &jsonl_path,
            "771bc3d8-0def-4c8c-a9b1-f674e93bd981",
            "-Users-test--qoderworkcn-workspace-abc123",
            "qoder_work_cn",
            false,
            metadata,
        )
        .expect("should parse a valid session file");

        assert_eq!(session.requests.len(), 2);
        assert_eq!(session.meta.message_count, 2);
        assert_eq!(session.meta.total_input_tokens, 0);
        assert_eq!(session.meta.total_output_tokens, 0);
        assert_eq!(session.meta.topic.as_deref(), Some("请检查前端页面"));
        assert_eq!(
            session.meta.session_id,
            "qoder_work_cn::cli::771bc3d8-0def-4c8c-a9b1-f674e93bd981"
        );
        assert_eq!(session.meta.source, QODER_WORK_CLI_SOURCE_KIND);
        assert_eq!(session.meta.models, vec!["qmodel_latest".to_string()]);
        // cwd 是沙箱路径，project_name 应当退化成 encoded 目录名的最后一段
        assert_eq!(session.meta.project_name.as_deref(), Some("abc123"));
    }

    #[test]
    fn scan_qoder_work_cli_sessions_includes_subagent_transcripts() {
        let tmpdir = tempdir().unwrap();
        let project_dir = tmpdir
            .path()
            .join("-Users-test--qoderworkcn-workspace-abc123");
        let session_dir = project_dir
            .join("771bc3d8-0def-4c8c-a9b1-f674e93bd981")
            .join("subagents");
        fs::create_dir_all(&session_dir).unwrap();
        let jsonl_path = session_dir.join("agent-aExplore-123.jsonl");
        let mut f = fs::File::create(&jsonl_path).unwrap();
        writeln!(
            f,
            r#"{{"uuid":"u1","type":"assistant","isSidechain":true,"timestamp":"2026-06-15T15:56:00.000Z","message":{{"role":"assistant","id":"a1","model":"gm51model"}}}}"#
        )
        .unwrap();
        writeln!(f, "{}", "x".repeat(60)).unwrap();
        drop(f);

        let files = collect_qoder_work_cli_jsonl_files(&project_dir);
        assert_eq!(files, vec![jsonl_path.clone()]);
        let session_key = qoder_work_cli_session_key(&project_dir, &jsonl_path);
        assert_eq!(
            session_key,
            "771bc3d8-0def-4c8c-a9b1-f674e93bd981::subagents::agent-aExplore-123"
        );

        let metadata = fs::metadata(&jsonl_path).ok();
        let subagent = parse_qoder_work_cli_session_file(
            &jsonl_path,
            &session_key,
            "-Users-test--qoderworkcn-workspace-abc123",
            "qoder_work_cn",
            is_qoder_work_subagent_path(&jsonl_path),
            metadata,
        )
        .expect("subagent transcript should parse");

        assert!(subagent
            .meta
            .session_id
            .contains("subagents::agent-aExplore-123"));
        assert_eq!(subagent.requests.len(), 1);
        assert!(subagent.requests[0].is_subagent);
        assert_eq!(subagent.requests[0].model, "gm51model");
    }

    /// 回归测试：`scan_qoder_work_cn_sessions()`（供 `scanner_sync.rs` 写入 SQLite
    /// 的持久化路径）必须和 `QoderWorkSource::scan()`（内存缓存路径）一样把
    /// CLI transcript 会话也合并进来。此前只修了后者，导致 CLI 数据能出现在
    /// 内存缓存里，却永远同步不进数据库——用户重装后 QoderWork CN 依然统计不到用量。
    #[test]
    fn scan_qoder_work_cn_sessions_includes_cli_transcript_sessions() {
        let tmp_home = tempdir().unwrap();
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp_home.path());

        let project_dir = tmp_home
            .path()
            .join(".qoderworkcn")
            .join("projects")
            .join("-Users-test--qoderworkcn-workspace-abc123");
        fs::create_dir_all(&project_dir).unwrap();
        let jsonl_path = project_dir.join("771bc3d8-0def-4c8c-a9b1-f674e93bd981.jsonl");
        let mut f = fs::File::create(&jsonl_path).unwrap();
        writeln!(
            f,
            r#"{{"uuid":"u1","type":"assistant","timestamp":"2026-06-15T15:56:00.000Z","message":{{"role":"assistant","id":"a1","model":"qmodel_latest","usage":{{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
        )
        .unwrap();
        // padding to pass the >=50 byte size gate
        writeln!(f, "{}", "x".repeat(60)).unwrap();
        drop(f);

        let sessions = scan_qoder_work_cn_sessions();

        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }

        assert!(
            sessions
                .iter()
                .any(|s| s.meta.source == QODER_WORK_CLI_SOURCE_KIND),
            "scan_qoder_work_cn_sessions() must include CLI transcript sessions, not just main.log ones"
        );
    }
}
