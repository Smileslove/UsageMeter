//! Read-only DeepSeek Harness session usage. Never persist message bodies or credentials.

use super::constants::TOOL_DEEPSEEK_HARNESS;
use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::extract_project_name;
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub(super) struct DeepSeekHarnessSource;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeepSeekHarnessScanStatus {
    pub root: Option<String>,
    pub session_count: usize,
    pub error_code: Option<String>,
}

pub(crate) fn scan_status() -> DeepSeekHarnessScanStatus {
    match session_root() {
        Ok(None) => DeepSeekHarnessScanStatus {
            root: None,
            session_count: 0,
            error_code: None,
        },
        Ok(Some(root)) => {
            let result = scan_root(&root);
            DeepSeekHarnessScanStatus {
                root: Some(root.to_string_lossy().to_string()),
                session_count: result
                    .as_ref()
                    .map(|snapshot| snapshot.sessions.len())
                    .unwrap_or(0),
                error_code: result.err(),
            }
        }
        Err(code) => DeepSeekHarnessScanStatus {
            root: None,
            session_count: 0,
            error_code: Some(code),
        },
    }
}

const MAX_FORMAT_VERSION: u32 = 4;
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_LOG_BYTES: usize = 256 * 1024 * 1024;
const MAX_EVENTS: usize = 1_000_000;

impl SessionSource for DeepSeekHarnessSource {
    fn tool_id(&self) -> &'static str {
        TOOL_DEEPSEEK_HARNESS
    }

    fn scan(&self) -> SourceSnapshot {
        self.try_scan().unwrap_or_else(|_| empty_snapshot())
    }

    fn try_scan(&self) -> Result<SourceSnapshot, String> {
        let Some(root) = session_root()? else {
            return Ok(empty_snapshot());
        };
        scan_root(&root)
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        parse_session(session)
    }
}

fn empty_snapshot() -> SourceSnapshot {
    SourceSnapshot {
        source_id: TOOL_DEEPSEEK_HARNESS,
        update_mode: SourceUpdateMode::PerSession,
        sessions: Vec::new(),
        scan_fingerprint: 0,
    }
}

fn session_root() -> Result<Option<PathBuf>, String> {
    let configured_root = crate::settings::persisted_deepseek_harness_session_root()?;
    let explicit_home = std::env::var_os("DSH_HOME").filter(|value| !value.is_empty());
    let root = if let Some(path) = configured_root.as_ref().filter(|path| !path.is_empty()) {
        PathBuf::from(path)
    } else {
        let home = match explicit_home.as_ref() {
            Some(value) => PathBuf::from(value),
            _ => dirs::home_dir()
                .ok_or("deepseek_harness_home_unavailable")?
                .join(".dsh"),
        };
        home.join("sessions")
    };
    if !root.is_absolute() {
        return Err("deepseek_harness_root_not_absolute".to_string());
    }
    if !root.exists() {
        return if explicit_home.is_some() || configured_root.is_some() {
            Err("deepseek_harness_root_unreadable".to_string())
        } else {
            Ok(None)
        };
    }
    root.canonicalize()
        .map(Some)
        .map_err(|_| "deepseek_harness_root_unreadable".to_string())
}

fn scan_root(root: &Path) -> Result<SourceSnapshot, String> {
    let mut sessions = Vec::new();
    let mut first_error: Option<String> = None;
    let root_tag = root_tag(root);
    let projects = fs::read_dir(root).map_err(|_| "deepseek_harness_root_unreadable")?;
    for project in projects {
        let project = match project {
            Ok(project) => project,
            Err(_) => {
                first_error.get_or_insert("deepseek_harness_root_unreadable".to_string());
                continue;
            }
        };
        if !project
            .file_type()
            .map(|file_type| file_type.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let project_name = project.file_name().to_string_lossy().to_string();
        if project_name != "_no-cwd"
            && !(project_name.starts_with("--") && project_name.ends_with("--"))
        {
            continue;
        }
        let entries = match fs::read_dir(project.path()) {
            Ok(entries) => entries,
            Err(_) => {
                first_error.get_or_insert("deepseek_harness_project_unreadable".to_string());
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    first_error.get_or_insert("deepseek_harness_project_unreadable".to_string());
                    continue;
                }
            };
            if !entry
                .file_type()
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let session_dir = entry.path();
            let artifacts = match fs::read_dir(&session_dir) {
                Ok(artifacts) => artifacts,
                Err(_) => {
                    first_error.get_or_insert("deepseek_harness_session_unreadable".to_string());
                    continue;
                }
            };
            let mut candidates = Vec::new();
            for artifact in artifacts {
                let artifact = match artifact {
                    Ok(artifact) => artifact,
                    Err(_) => {
                        first_error
                            .get_or_insert("deepseek_harness_session_unreadable".to_string());
                        continue;
                    }
                };
                if !artifact
                    .file_type()
                    .map(|file_type| file_type.is_file())
                    .unwrap_or(false)
                {
                    continue;
                }
                let name = artifact.file_name();
                let Some((version, compressed)) =
                    parse_generation_filename(&name.to_string_lossy())
                else {
                    continue;
                };
                let metadata = match artifact.metadata() {
                    Ok(metadata) => metadata,
                    Err(_) => {
                        first_error
                            .get_or_insert("deepseek_harness_session_unreadable".to_string());
                        continue;
                    }
                };
                candidates.push((
                    metadata.modified().ok(),
                    version,
                    compressed,
                    artifact.path(),
                    metadata,
                ));
            }
            if candidates.is_empty() {
                continue;
            }
            candidates.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
            let (modified, version, _, path, metadata) =
                candidates.pop().expect("nonempty candidates");
            if version > MAX_FORMAT_VERSION {
                first_error.get_or_insert("deepseek_harness_format_unsupported".to_string());
                continue;
            }
            let header = match read_header(&path, version) {
                Ok(header) => header,
                Err(error) => {
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            let encoded_id = entry.file_name().to_string_lossy().to_string();
            if encode_segment(&header.id) != encoded_id {
                first_error.get_or_insert("deepseek_harness_header_mismatch".to_string());
                continue;
            }
            let session_id = format!("{TOOL_DEEPSEEK_HARNESS}::{root_tag}::{}", header.id);
            let modified = modified
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            let path_text = path.to_string_lossy().to_string();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            path_text.hash(&mut hasher);
            metadata.len().hash(&mut hasher);
            modified.hash(&mut hasher);
            sessions.push(SessionFile {
                session_id,
                tool: TOOL_DEEPSEEK_HARNESS.to_string(),
                project_path: project_name.clone(),
                file_path: path_text.clone(),
                transcript_paths: vec![path_text],
                file_size: metadata.len(),
                last_modified: modified,
                fingerprint: hasher.finish(),
            });
        }
    }
    if sessions.is_empty() {
        if let Some(error) = first_error {
            return Err(error);
        }
    }
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for session in &sessions {
        session.session_id.hash(&mut hasher);
        session.fingerprint.hash(&mut hasher);
    }
    Ok(SourceSnapshot {
        source_id: TOOL_DEEPSEEK_HARNESS,
        update_mode: SourceUpdateMode::PerSession,
        sessions,
        scan_fingerprint: hasher.finish(),
    })
}

fn root_tag(root: &Path) -> String {
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    format!("{digest:x}")[..16].to_string()
}

fn parse_generation_filename(name: &str) -> Option<(u32, bool)> {
    let (stem, compressed) = if let Some(stem) = name.strip_suffix(".zstd") {
        (stem, true)
    } else {
        (name, false)
    };
    if stem == "session.jsonl" {
        return Some((0, compressed));
    }
    let version = stem.strip_prefix("session.v")?.strip_suffix(".jsonl")?;
    if version.starts_with('0')
        || version.is_empty()
        || !version.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some((version.parse().ok()?, compressed))
}

fn encode_segment(raw: &str) -> String {
    if raw == "." {
        return "~002E".to_string();
    }
    if raw == ".." {
        return "~002E~002E".to_string();
    }
    let mut encoded = String::new();
    for unit in raw.encode_utf16() {
        if unit <= 127 && (unit as u8).is_ascii_alphanumeric() || matches!(unit, 0x2e | 0x5f | 0x2d)
        {
            encoded.push(char::from_u32(u32::from(unit)).expect("ascii code unit"));
        } else {
            encoded.push_str(&format!("~{unit:04X}"));
        }
    }
    encoded
}

struct Header {
    id: String,
    created_at: i64,
    cwd: Option<String>,
    subagent: bool,
    inherited_count: u64,
    is_seeded: bool,
}

fn open_lines(path: &Path) -> Result<(BufReader<Box<dyn Read>>, bool), String> {
    if !fs::symlink_metadata(path)
        .map_err(|_| "deepseek_harness_file_unreadable")?
        .file_type()
        .is_file()
    {
        return Err("deepseek_harness_file_unreadable".to_string());
    }
    let file = File::open(path).map_err(|_| "deepseek_harness_file_unreadable")?;
    let is_zstd = path.extension().and_then(|part| part.to_str()) == Some("zstd");
    let reader: Box<dyn Read> = if is_zstd {
        Box::new(
            zstd::stream::read::Decoder::new(file).map_err(|_| "deepseek_harness_zstd_invalid")?,
        )
    } else {
        Box::new(file)
    };
    Ok((BufReader::new(reader), is_zstd))
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    total: &mut usize,
    tolerate_truncated_tail: bool,
) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(_) if tolerate_truncated_tail => return Ok(None),
            Err(_) => return Err("deepseek_harness_log_unreadable".to_string()),
        };
        if available.is_empty() {
            return if line.is_empty() || tolerate_truncated_tail {
                Ok(None)
            } else {
                Err("deepseek_harness_incomplete_tail".to_string())
            };
        }
        let amount = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len() + amount > MAX_LINE_BYTES || *total + amount > MAX_LOG_BYTES {
            return Err("deepseek_harness_source_limit_exceeded".to_string());
        }
        let complete = available[amount - 1] == b'\n';
        line.extend_from_slice(&available[..amount]);
        reader.consume(amount);
        *total += amount;
        if complete {
            return Ok(Some(line));
        }
    }
}

fn header_from_line(line: &[u8], version: u32) -> Result<Header, String> {
    let value: Value =
        serde_json::from_slice(line).map_err(|_| "deepseek_harness_header_invalid")?;
    let event_type = value.get("type").and_then(Value::as_str);
    if !matches!(event_type, Some("session" | "session/start"))
        || (value.get("version").is_some()
            && value.get("version").and_then(Value::as_u64) != Some(u64::from(version)))
    {
        return Err("deepseek_harness_header_mismatch".to_string());
    }
    let id = value
        .get("id")
        .or_else(|| value.pointer("/data/id"))
        .or_else(|| value.pointer("/data/sessionId"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or("deepseek_harness_header_invalid")?
        .to_string();
    let created_at = value
        .get("createdAt")
        .or_else(|| value.pointer("/data/createdAt"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        / 1000;
    let cwd = value
        .get("cwd")
        .or_else(|| value.pointer("/data/cwd"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let subagent = value.get("origin").and_then(Value::as_str) == Some("subagent")
        || value
            .get("delegationDepth")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0;
    let is_seeded = if version >= 2 {
        value
            .get("isSeeded")
            .or_else(|| value.pointer("/data/isSeeded"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    } else {
        false
    };
    let inherited_count = if version <= 1 {
        value
            .get("seedLength")
            .map(Value::as_u64)
            .unwrap_or(Some(0))
            .ok_or("deepseek_harness_header_invalid")?
    } else {
        0
    };
    Ok(Header {
        id,
        created_at,
        cwd,
        subagent,
        inherited_count,
        is_seeded,
    })
}

fn read_header(path: &Path, version: u32) -> Result<Header, String> {
    let (mut reader, _) = open_lines(path)?;
    let mut total = 0;
    let line = read_bounded_line(&mut reader, &mut total, false)?
        .ok_or("deepseek_harness_header_missing")?;
    header_from_line(&line, version)
}

#[derive(Clone, Copy)]
struct Usage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
}

fn parse_usage(value: &Value) -> Option<Usage> {
    let input = value.get("inputTokens")?.as_u64()?;
    let output = value.get("outputTokens")?.as_u64()?;
    let optional = |key: &str| value.get(key).map(Value::as_u64).unwrap_or(Some(0));
    let usage = Usage {
        input,
        output,
        cache_read: optional("cacheReadTokens")?,
        cache_write: optional("cacheWriteTokens")?,
        reasoning: optional("reasoningTokens")?,
    };
    Some(usage)
}

fn normalize_model(value: Option<&Value>) -> Option<String> {
    let value = value.and_then(Value::as_str)?.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.rsplit('/').next().unwrap_or(value).to_string())
}

fn event_usage(event: &Value) -> Option<Usage> {
    let data = event.get("data")?;
    if event.get("type").and_then(Value::as_str) == Some("compaction/summary") {
        return data.get("usage").and_then(parse_usage);
    }
    if let Some(usage) = data.get("usage").and_then(parse_usage) {
        return Some(usage);
    }
    data.get("stream")?
        .as_array()?
        .iter()
        .rev()
        .find_map(|chunk| {
            (chunk.get("type").and_then(Value::as_str) == Some("chunk")
                && chunk.pointer("/chunk/type").and_then(Value::as_str) == Some("usage"))
            .then(|| chunk.pointer("/chunk/usage").and_then(parse_usage))
            .flatten()
        })
}

fn parse_session(session: &SessionFile) -> Result<ParsedSessionData, String> {
    let path = Path::new(&session.file_path);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("deepseek_harness_filename_invalid")?;
    let (version, _) =
        parse_generation_filename(filename).ok_or("deepseek_harness_filename_invalid")?;
    if version > MAX_FORMAT_VERSION {
        return Err("deepseek_harness_format_unsupported".to_string());
    }
    let (mut reader, tolerate_truncated_tail) = open_lines(path)?;
    let mut total = 0;
    let header_line = read_bounded_line(&mut reader, &mut total, false)?
        .ok_or("deepseek_harness_header_missing")?;
    let header = header_from_line(&header_line, version)?;
    let root = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("deepseek_harness_path_invalid")?;
    let expected_id = format!("{TOOL_DEEPSEEK_HARNESS}::{}::{}", root_tag(root), header.id);
    if session.session_id != expected_id {
        return Err("deepseek_harness_header_mismatch".to_string());
    }
    let mut requests = Vec::new();
    let mut request_seqs = Vec::new();
    let mut current_model = String::from("unknown");
    let mut last_slot: Option<(u64, u64, usize)> = None;
    let mut seed_marker = None;
    let mut previous_seq: Option<u64> = None;
    let mut end_time = header.created_at;
    let mut event_count = 0;

    while let Some(line) = read_bounded_line(&mut reader, &mut total, tolerate_truncated_tail)? {
        event_count += 1;
        if event_count > MAX_EVENTS {
            return Err("deepseek_harness_source_limit_exceeded".to_string());
        }
        let event: Value =
            serde_json::from_slice(&line).map_err(|_| "deepseek_harness_event_invalid")?;
        if matches!(
            event.get("type").and_then(Value::as_str),
            Some("session" | "session/start")
        ) {
            continue;
        }
        if let Some((last_seq, count)) = packed_last_seq(&event, version, previous_seq)? {
            event_count = event_count.saturating_add(count - 1);
            if event_count > MAX_EVENTS {
                return Err("deepseek_harness_source_limit_exceeded".to_string());
            }
            previous_seq = Some(last_seq);
            continue;
        }
        let seq = event
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or("deepseek_harness_event_invalid")?;
        if previous_seq.map_or(seq != 0, |previous| previous.checked_add(1) != Some(seq)) {
            return Err("deepseek_harness_seq_invalid".to_string());
        }
        previous_seq = Some(seq);
        let timestamp = event
            .get("time")
            .and_then(Value::as_i64)
            .filter(|time| *time >= 0)
            .ok_or("deepseek_harness_event_invalid")?
            / 1000;
        end_time = end_time.max(timestamp);
        match event.get("type").and_then(Value::as_str) {
            Some("session/end-seed") => {
                if event.pointer("/data/inherited").and_then(Value::as_bool) == Some(true) {
                    seed_marker = Some(seq);
                }
            }
            Some("request/header") => {
                current_model = normalize_model(event.pointer("/data/header/config/model"))
                    .unwrap_or_else(|| "unknown".to_string());
            }
            Some("llm/retry-started") => last_slot = None,
            Some("assistant/message" | "message/assistant") => {
                let Some(usage) = event_usage(&event) else {
                    continue;
                };
                let data = event.get("data").ok_or("deepseek_harness_event_invalid")?;
                let slot = Some((
                    data.get("turn").and_then(Value::as_u64).unwrap_or(0),
                    data.get("step").and_then(Value::as_u64).unwrap_or(0),
                ));
                let output_tokens = usage
                    .output
                    .checked_add(usage.reasoning)
                    .ok_or("deepseek_harness_usage_overflow")?;
                let total_tokens = usage
                    .input
                    .checked_add(output_tokens)
                    .and_then(|sum| sum.checked_add(usage.cache_read))
                    .and_then(|sum| sum.checked_add(usage.cache_write))
                    .ok_or("deepseek_harness_usage_overflow")?;
                let source = data.pointer("/message/source");
                let model = normalize_model(
                    source.and_then(|source| source.pointer("/replayState/response/responseModel")),
                )
                .or_else(|| normalize_model(source.and_then(|source| source.get("model"))))
                .or_else(|| normalize_model(data.get("model")))
                .unwrap_or_else(|| current_model.clone());
                let message_id = format!("{}:{seq}", session.session_id);
                let record = LocalRequestRecord {
                    session_id: session.session_id.clone(),
                    tool: TOOL_DEEPSEEK_HARNESS.to_string(),
                    timestamp,
                    message_id: message_id.clone(),
                    input_tokens: usage.input,
                    output_tokens,
                    reasoning_tokens: usage.reasoning,
                    cache_create_tokens: usage.cache_write,
                    cache_read_tokens: usage.cache_read,
                    total_tokens,
                    request_count: 1,
                    model,
                    is_subagent: header.subagent,
                    request_key: Some(message_id),
                    ..Default::default()
                };
                if let Some((turn, step)) = slot {
                    if let Some((last_turn, last_step, index)) = last_slot {
                        if turn == last_turn && step == last_step {
                            requests[index] = record;
                            request_seqs[index] = seq;
                            continue;
                        }
                    }
                    last_slot = Some((turn, step, requests.len()));
                } else {
                    last_slot = None;
                }
                requests.push(record);
                request_seqs.push(seq);
            }
            _ => {}
        }
    }
    if version >= 2 && header.is_seeded != seed_marker.is_some() {
        return Err("deepseek_harness_seed_invalid".to_string());
    }
    let inherited_count = if version <= 1 {
        header.inherited_count
    } else {
        seed_marker.unwrap_or(0)
    };
    let requests: Vec<LocalRequestRecord> = requests
        .into_iter()
        .zip(request_seqs)
        .filter_map(|(request, seq)| (seq >= inherited_count).then_some(request))
        .collect();
    let models: BTreeSet<String> = requests
        .iter()
        .filter(|request| request.model != "unknown")
        .map(|request| request.model.clone())
        .collect();
    let mut meta = SessionMeta {
        session_id: session.session_id.clone(),
        tool: TOOL_DEEPSEEK_HARNESS.to_string(),
        cwd: header.cwd.clone(),
        project_name: header.cwd.as_deref().and_then(extract_project_name),
        file_path: session.file_path.clone(),
        file_size: session.file_size,
        last_modified: session.last_modified,
        models: models.into_iter().collect(),
        message_count: requests.len() as u64,
        start_time: header.created_at,
        end_time,
        source: "deepseek_harness_session".to_string(),
        message_ids: requests
            .iter()
            .map(|request| request.message_id.clone())
            .collect(),
        ..Default::default()
    };
    for record in &requests {
        meta.total_input_tokens = meta.total_input_tokens.saturating_add(record.input_tokens);
        meta.total_output_tokens = meta
            .total_output_tokens
            .saturating_add(record.output_tokens);
        meta.total_cache_create_tokens = meta
            .total_cache_create_tokens
            .saturating_add(record.cache_create_tokens);
        meta.total_cache_read_tokens = meta
            .total_cache_read_tokens
            .saturating_add(record.cache_read_tokens);
        meta.total_reasoning_tokens = meta
            .total_reasoning_tokens
            .saturating_add(record.reasoning_tokens);
    }
    Ok(ParsedSessionData { meta, requests })
}

fn packed_last_seq(
    event: &Value,
    version: u32,
    previous_seq: Option<u64>,
) -> Result<Option<(u64, usize)>, String> {
    let field = match event.get("type").and_then(Value::as_str) {
        Some("text-chunks" | "reasoning-chunks") => "texts",
        Some("tool-call-chunks") => "args",
        _ => return Ok(None),
    };
    if version > 1 {
        return Err("deepseek_harness_event_invalid".to_string());
    }
    let first = event
        .get("seq0")
        .and_then(Value::as_u64)
        .ok_or("deepseek_harness_event_invalid")?;
    let expected = match previous_seq {
        Some(seq) => seq.checked_add(1).ok_or("deepseek_harness_seq_invalid")?,
        None => 0,
    };
    if first != expected {
        return Err("deepseek_harness_seq_invalid".to_string());
    }
    let members = event
        .pointer(&format!("/data/{field}"))
        .and_then(Value::as_array)
        .filter(|members| !members.is_empty())
        .ok_or("deepseek_harness_event_invalid")?;
    let last = first
        .checked_add(members.len() as u64 - 1)
        .ok_or("deepseek_harness_seq_invalid")?;
    Ok(Some((last, members.len())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn generation_names_are_strict() {
        assert_eq!(
            parse_generation_filename("session.jsonl.zstd"),
            Some((0, true))
        );
        assert_eq!(
            parse_generation_filename("session.v4.jsonl"),
            Some((4, false))
        );
        assert_eq!(parse_generation_filename("session.v04.jsonl"), None);
        assert_eq!(parse_generation_filename("session.v4.jsonl.tmp"), None);
    }

    #[test]
    fn reads_v0_through_v4_with_plain_and_zstd_encoding() {
        for version in 0..=MAX_FORMAT_VERSION {
            for compressed in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let directory = temp.path().join("--example--").join("session-1");
                fs::create_dir_all(&directory).unwrap();
                let mut header = serde_json::json!({
                    "type": "session", "version": version, "id": "session-1",
                    "createdAt": 1000
                });
                if version <= 1 {
                    header["seedLength"] = serde_json::json!(0);
                } else {
                    header["isSeeded"] = serde_json::json!(false);
                }
                let rows = [
                    header,
                    serde_json::json!({"type":"request/header","seq":0,"time":1000,"data":{"header":{"config":{"model":"deepseek-test"}}}}),
                    serde_json::json!({"type":"assistant/message","seq":1,"time":2000,"data":{"turn":0,"step":0,"usage":{"inputTokens":2,"outputTokens":3},"stream":[]}}),
                ];
                let log = rows
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
                let basename = if version == 0 {
                    "session.jsonl".to_string()
                } else {
                    format!("session.v{version}.jsonl")
                };
                let path = directory.join(if compressed {
                    format!("{basename}.zstd")
                } else {
                    basename
                });
                if compressed {
                    let mut encoder = zstd::Encoder::new(File::create(path).unwrap(), 0).unwrap();
                    encoder.write_all(log.as_bytes()).unwrap();
                    encoder.finish().unwrap();
                } else {
                    fs::write(path, log).unwrap();
                }
                let snapshot = scan_root(temp.path()).unwrap();
                let parsed = parse_session(&snapshot.sessions[0]).unwrap();
                assert_eq!(
                    parsed.requests.len(),
                    1,
                    "version={version}, compressed={compressed}"
                );
                assert_eq!(parsed.requests[0].total_tokens, 5);
            }
        }
    }

    #[test]
    fn accepts_reference_session_start_and_message_assistant_protocol() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("--example--").join("session-1");
        fs::create_dir_all(&directory).unwrap();
        let rows = [
            serde_json::json!({
                "type": "session/start",
                "id": "session-1",
                "createdAt": 1000,
                "cwd": "/example"
            }),
            serde_json::json!({
                "type": "request/header",
                "seq": 0,
                "time": 1000,
                "data": {"header": {"config": {"model": "deepseek/deepseek-v4-pro"}}}
            }),
            serde_json::json!({
                "type": "message/assistant",
                "seq": 1,
                "time": 2000,
                "data": {
                    "model": "deepseek/deepseek-v4-flash",
                    "usage": {
                        "inputTokens": 100,
                        "outputTokens": 40,
                        "cacheReadTokens": 50,
                        "cacheWriteTokens": 10,
                        "reasoningTokens": 20
                    }
                }
            }),
        ];
        let mut log = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        log.push('\n');
        fs::write(directory.join("session.v3.jsonl"), log).unwrap();

        let snapshot = scan_root(temp.path()).unwrap();
        let parsed = parse_session(&snapshot.sessions[0]).unwrap();
        assert_eq!(parsed.requests.len(), 1);
        assert_eq!(parsed.requests[0].model, "deepseek-v4-flash");
        assert_eq!(parsed.requests[0].output_tokens, 60);
        assert_eq!(parsed.requests[0].total_tokens, 220);
        assert_eq!(parsed.requests[0].reasoning_tokens, 20);
    }

    #[test]
    fn parses_v4_zstd_usage_from_real_event_shapes() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("--example--").join("session-v4");
        fs::create_dir_all(&directory).unwrap();
        let rows = [
            serde_json::json!({
                "type": "session",
                "version": 4,
                "id": "session-v4",
                "createdAt": 1789230519472i64,
                "cwd": "/example",
                "isSeeded": false,
                "delegationDepth": 0
            }),
            serde_json::json!({
                "type": "request/header",
                "seq": 0,
                "time": 1789230519472i64,
                "data": {"header": {"config": {"model": "deepseek-flash"}}}
            }),
            serde_json::json!({
                "type": "request/context",
                "seq": 1,
                "time": 1789230519473i64,
                "data": {"model": "deepseek-flash"}
            }),
            serde_json::json!({
                "type": "assistant/message",
                "seq": 2,
                "time": 1789230519474i64,
                "data": {
                    "turn": 1,
                    "step": 1,
                    "message": {"source": {"model": "deepseek-flash"}},
                    "usage": {
                        "inputTokens": 9432,
                        "outputTokens": 96,
                        "totalTokens": 10808,
                        "cacheReadTokens": 1280
                    }
                }
            }),
        ];
        let log = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let path = directory.join("session.v4.jsonl.zstd");
        let mut encoder = zstd::Encoder::new(File::create(path).unwrap(), 0).unwrap();
        encoder.write_all(log.as_bytes()).unwrap();
        encoder.finish().unwrap();

        let snapshot = scan_root(temp.path()).unwrap();
        let parsed = parse_session(&snapshot.sessions[0]).unwrap();
        assert_eq!(parsed.requests.len(), 1);
        assert_eq!(parsed.requests[0].model, "deepseek-flash");
        assert_eq!(parsed.requests[0].input_tokens, 9432);
        assert_eq!(parsed.requests[0].output_tokens, 96);
        assert_eq!(parsed.requests[0].cache_read_tokens, 1280);
        assert_eq!(parsed.requests[0].total_tokens, 10808);
    }

    #[test]
    fn skips_bad_session_without_discarding_healthy_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("--example--");
        let healthy = project.join("healthy");
        let broken = project.join("broken");
        fs::create_dir_all(&healthy).unwrap();
        fs::create_dir_all(&broken).unwrap();
        fs::write(
            healthy.join("session.v3.jsonl"),
            concat!(
                "{\"type\":\"session\",\"version\":3,\"id\":\"healthy\",\"createdAt\":1000,\"isSeeded\":false}\n",
                "{\"type\":\"message/assistant\",\"seq\":0,\"time\":2000,\"data\":{\"usage\":{\"inputTokens\":1,\"outputTokens\":2}}}\n"
            ),
        )
        .unwrap();
        fs::write(
            broken.join("session.v99.jsonl"),
            b"{\"type\":\"session\",\"version\":99,\"id\":\"broken\"}\n",
        )
        .unwrap();

        let snapshot = scan_root(temp.path()).unwrap();
        assert_eq!(snapshot.sessions.len(), 1);
        assert!(snapshot.sessions[0].session_id.ends_with("::healthy"));
    }

    #[test]
    fn scans_one_generation_and_ignores_attempt_usage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        let directory = root.join("--example--").join("session-1");
        fs::create_dir_all(&directory).unwrap();
        let log = concat!(
            "{\"type\":\"session\",\"version\":3,\"id\":\"session-1\",\"createdAt\":1000,\"cwd\":\"/example\",\"isSeeded\":false,\"delegationDepth\":0}\n",
            "{\"type\":\"request/header\",\"seq\":0,\"time\":1000,\"data\":{\"header\":{\"config\":{\"provider\":\"deepseek-official\",\"model\":\"deepseek-test\"}},\"reason\":\"initial\"}}\n",
            "{\"type\":\"assistant/attempt\",\"seq\":1,\"time\":2000,\"data\":{\"turn\":0,\"step\":0,\"stream\":[{\"type\":\"chunk\",\"chunk\":{\"type\":\"usage\",\"usage\":{\"inputTokens\":2,\"outputTokens\":3}}}]}}\n",
            "{\"type\":\"assistant/message\",\"seq\":2,\"time\":3000,\"data\":{\"turn\":0,\"step\":0,\"usage\":{\"inputTokens\":4,\"outputTokens\":5,\"cacheReadTokens\":6,\"reasoningTokens\":2},\"stream\":[]}}\n",
            "{\"type\":\"llm/retry-started\",\"seq\":3,\"time\":4000,\"data\":{\"turn\":0,\"step\":0}}\n",
            "{\"type\":\"assistant/attempt\",\"seq\":4,\"time\":5000,\"data\":{\"turn\":0,\"step\":0,\"stream\":[{\"type\":\"chunk\",\"chunk\":{\"type\":\"usage\",\"usage\":{\"inputTokens\":1,\"outputTokens\":1}}}]}}\n"
        );
        fs::write(
            directory.join("session.v2.jsonl"),
            log.replace("\"version\":3", "\"version\":2"),
        )
        .unwrap();
        let mut encoder = zstd::Encoder::new(
            File::create(directory.join("session.v3.jsonl.zstd")).unwrap(),
            0,
        )
        .unwrap();
        encoder.write_all(log.as_bytes()).unwrap();
        encoder.finish().unwrap();
        let snapshot = scan_root(&root).unwrap();
        assert_eq!(snapshot.sessions.len(), 1);
        assert!(snapshot.sessions[0]
            .file_path
            .ends_with("session.v3.jsonl.zstd"));
        let parsed = parse_session(&snapshot.sessions[0]).unwrap();
        assert_eq!(parsed.requests.len(), 1);
        assert_eq!(parsed.requests[0].total_tokens, 17);
        assert_eq!(parsed.requests[0].reasoning_tokens, 2);
        assert_eq!(parsed.meta.message_count, 1);
        assert_eq!(parsed.meta.models, vec!["deepseek-test"]);
    }

    #[test]
    fn refuses_unknown_generation_without_falling_back() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("--example--").join("session-1");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("session.v5.jsonl"), b"future\n").unwrap();
        assert_eq!(
            scan_root(temp.path()).err().unwrap(),
            "deepseek_harness_format_unsupported"
        );
    }

    #[test]
    fn decodes_released_v0_packed_rows_without_losing_sequence() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("--example--").join("session-1");
        fs::create_dir_all(&directory).unwrap();
        let rows = [
            serde_json::json!({"type":"session","version":0,"id":"session-1","createdAt":1000,"delegationDepth":0}),
            serde_json::json!({"type":"request/header","seq":0,"time":1000,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-test"}},"reason":"initial"}}),
            serde_json::json!({"type":"text-chunks","seq0":1,"time0":1500,"data":{"turn":0,"step":0,"index":0,"dt":[1],"texts":["a","b"]}}),
            serde_json::json!({"type":"assistant/message","seq":3,"time":2000,"data":{"turn":0,"step":0,"usage":{"inputTokens":3,"outputTokens":4},"stream":[]}}),
        ];
        let log = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(directory.join("session.jsonl"), log).unwrap();
        let snapshot = scan_root(temp.path()).unwrap();
        let parsed = parse_session(&snapshot.sessions[0]).unwrap();
        assert_eq!(parsed.requests.len(), 1);
        assert_eq!(parsed.requests[0].total_tokens, 7);
    }

    #[test]
    fn seeded_generation_excludes_inherited_usage() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("--example--").join("session-1");
        fs::create_dir_all(&directory).unwrap();
        let rows = [
            serde_json::json!({"type":"session","version":3,"id":"session-1","createdAt":1000,"isSeeded":true,"delegationDepth":1}),
            serde_json::json!({"type":"request/header","seq":0,"time":1000,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-test"}},"reason":"initial"}}),
            serde_json::json!({"type":"assistant/message","seq":1,"time":2000,"data":{"turn":0,"step":0,"usage":{"inputTokens":10,"outputTokens":10},"stream":[]}}),
            serde_json::json!({"type":"session/end-seed","seq":2,"time":3000,"data":{"inherited":true}}),
            serde_json::json!({"type":"assistant/message","seq":3,"time":4000,"data":{"turn":0,"step":1,"usage":{"inputTokens":2,"outputTokens":3},"stream":[]}}),
        ];
        let log = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(directory.join("session.v3.jsonl"), log).unwrap();
        let snapshot = scan_root(temp.path()).unwrap();
        let parsed = parse_session(&snapshot.sessions[0]).unwrap();
        assert_eq!(parsed.requests.len(), 1);
        assert_eq!(parsed.requests[0].total_tokens, 5);
        assert!(parsed.requests[0].is_subagent);
    }

    #[test]
    fn ignores_compaction_usage_and_attributes_the_served_model() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("--example--").join("session-1");
        fs::create_dir_all(&directory).unwrap();
        let rows = [
            serde_json::json!({"type":"session","version":3,"id":"session-1","createdAt":1000,"cwd":"/example","isSeeded":false}),
            serde_json::json!({"type":"request/header","seq":0,"time":1000,"data":{"header":{"config":{"model":"header-model"}}}}),
            serde_json::json!({"type":"assistant/message","seq":1,"time":2000,"data":{"turn":0,"step":0,"message":{"source":{"model":"configured-model","replayState":{"response":{"responseModel":"served-model"}}}},"usage":{"inputTokens":2,"outputTokens":3}}}),
            serde_json::json!({"type":"compaction/summary","seq":2,"time":3000,"data":{"message":{"source":{"model":"summary-model"}},"usage":{"inputTokens":5,"outputTokens":7,"cacheReadTokens":11}}}),
            serde_json::json!({"type":"assistant/message","seq":3,"time":4000,"data":{"turn":0,"step":1,"message":{"source":{"model":"configured-model"}},"usage":{"inputTokens":13,"outputTokens":17}}}),
        ];
        let log = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(directory.join("session.v3.jsonl"), log).unwrap();

        let snapshot = scan_root(temp.path()).unwrap();
        let parsed = parse_session(&snapshot.sessions[0]).unwrap();
        assert_eq!(parsed.requests.len(), 2);
        assert_eq!(
            parsed
                .requests
                .iter()
                .map(|request| request.total_tokens)
                .collect::<Vec<_>>(),
            [5, 30]
        );
        assert_eq!(
            parsed
                .requests
                .iter()
                .map(|request| request.model.as_str())
                .collect::<Vec<_>>(),
            ["served-model", "configured-model"]
        );
        assert_eq!(parsed.meta.message_count, 2);
    }

    #[test]
    fn keeps_complete_events_before_a_truncated_zstd_tail() {
        fn frame(content: &[u8]) -> Vec<u8> {
            let mut encoder = zstd::Encoder::new(Vec::new(), 0).unwrap();
            encoder.write_all(content).unwrap();
            encoder.finish().unwrap()
        }

        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("--example--").join("session-1");
        fs::create_dir_all(&directory).unwrap();
        let header = b"{\"type\":\"session\",\"version\":3,\"id\":\"session-1\",\"createdAt\":1000,\"isSeeded\":false}\n";
        let events = concat!(
            "{\"type\":\"request/header\",\"seq\":0,\"time\":1000,\"data\":{\"header\":{\"config\":{\"model\":\"deepseek-test\"}}}}\n",
            "{\"type\":\"assistant/message\",\"seq\":1,\"time\":2000,\"data\":{\"turn\":0,\"step\":0,\"usage\":{\"inputTokens\":2,\"outputTokens\":3}}}\n"
        );
        let mut bytes = frame(header);
        bytes.extend(frame(events.as_bytes()));
        let mut torn = frame(b"{\"type\":\"assistant/message\",\"seq\":2");
        torn.truncate(torn.len() - 3);
        bytes.extend(torn);
        let path = directory.join("session.v3.jsonl.zstd");
        fs::write(path, bytes).unwrap();

        let snapshot = scan_root(temp.path()).unwrap();
        let parsed = parse_session(&snapshot.sessions[0]).unwrap();
        assert_eq!(parsed.requests.len(), 1);
        assert_eq!(parsed.requests[0].total_tokens, 5);
    }
}
