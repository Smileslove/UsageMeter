//! Reasonix 本地会话读取模块（仅元数据补全）
//!
//! Reasonix 的本地会话文件 `<config_dir>/reasonix/sessions/<ts>-<model>.jsonl`
//! 每行只是一条对话消息（role/content/reasoning_content/tool_calls），
//! **不包含任何 token 用量**。token 仅存在于运行时内存与事件流，从不落盘。
//!
//! 因此本适配器只产出会话/项目级元数据（标题、消息数、模型、时间、cwd），
//! `requests` 恒为空——准确 token/cost 一律由代理链提供，本地不伪造用量。
//!
//! 路径说明：Reasonix（Go）用 `os.UserConfigDir()` 落盘，与 Rust 的
//! `dirs::config_dir()` 逐平台一致：
//! - macOS: `~/Library/Application Support/reasonix/`
//! - Linux: `~/.config/reasonix/`
//! - Windows: `%AppData%\reasonix\`
//!
//! 同时兼容 Linux 上可能存在的 `~/.config/reasonix/`。

use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::{extract_project_name, truncate_string};
use super::source::{ParsedSessionData, SessionSource, SourceSnapshot, SourceUpdateMode};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(super) struct ReasonixSource;

impl SessionSource for ReasonixSource {
    fn tool_id(&self) -> &'static str {
        super::constants::TOOL_REASONIX
    }

    fn scan(&self) -> SourceSnapshot {
        let mut sessions = Vec::new();
        for root in reasonix_session_roots() {
            if root.exists() {
                sessions.extend(collect_reasonix_session_files(&root));
            }
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for session in &sessions {
            session.session_id.hash(&mut hasher);
            session.fingerprint.hash(&mut hasher);
        }

        SourceSnapshot {
            source_id: self.tool_id(),
            update_mode: SourceUpdateMode::PerSession,
            sessions,
            scan_fingerprint: hasher.finish(),
        }
    }

    fn parse(&self, session: &SessionFile) -> Result<ParsedSessionData, String> {
        let data = parse_reasonix_session_file(session);
        Ok(ParsedSessionData {
            meta: data.meta,
            requests: data.requests,
        })
    }
}

pub(super) struct ReasonixParsedData {
    pub(super) meta: SessionMeta,
    pub(super) requests: Vec<LocalRequestRecord>,
}

/// Reasonix `.jsonl.meta` 旁车文件（BranchMeta 子集）
#[derive(Debug, Default, Deserialize)]
struct ReasonixBranchMeta {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    topic_title: Option<String>,
}

/// Reasonix 会话 JSONL 单行消息（仅取需要的字段）
#[derive(Debug, Default, Deserialize)]
struct ReasonixMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: String,
}

fn reasonix_session_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(config_dir) = dirs::config_dir() {
        roots.push(config_dir.join("reasonix").join("sessions"));
    }
    // Linux 上 dirs::config_dir() 即 ~/.config；其它平台额外兼容 ~/.config/reasonix。
    if let Some(home) = dirs::home_dir() {
        let xdg = home.join(".config").join("reasonix").join("sessions");
        if !roots.contains(&xdg) {
            roots.push(xdg);
        }
    }
    roots
}

fn collect_reasonix_session_files(root: &Path) -> Vec<SessionFile> {
    let mut sessions = Vec::new();
    let Ok(read_dir) = fs::read_dir(root) else {
        return sessions;
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // 只取会话主文件 *.jsonl，跳过 .jsonl.meta / .display.json / .legacy-imported 等。
        if !file_name.ends_with(".jsonl") || file_name.starts_with('.') {
            continue;
        }

        let metadata = entry.metadata().ok();
        let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        if file_size < 2 {
            continue;
        }
        let last_modified = metadata
            .and_then(|m| m.modified().ok())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            })
            .unwrap_or(0);

        let file_stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(file_name)
            .to_string();
        let unique_id = format!("{}::{}", super::constants::TOOL_REASONIX, file_stem);

        let path_string = path.to_string_lossy().to_string();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        path_string.hash(&mut hasher);
        file_size.hash(&mut hasher);
        last_modified.hash(&mut hasher);

        sessions.push(SessionFile {
            session_id: unique_id,
            tool: super::constants::TOOL_REASONIX.to_string(),
            project_path: String::new(),
            file_path: path_string.clone(),
            transcript_paths: vec![path_string],
            file_size,
            last_modified,
            fingerprint: hasher.finish(),
        });
    }

    sessions
}

pub(super) fn parse_reasonix_session_file(session: &SessionFile) -> ReasonixParsedData {
    let mut meta = SessionMeta {
        session_id: session.session_id.clone(),
        tool: session.tool.clone(),
        file_path: session.file_path.clone(),
        file_size: session.file_size,
        last_modified: session.last_modified,
        start_time: session.last_modified,
        end_time: session.last_modified,
        source: "reasonix_session".to_string(),
        ..Default::default()
    };

    // 模型名来自文件名后缀：<timestamp>-<model>，timestamp 形如
    // 20260604-054135.294110000，故去掉前两段后即模型名。
    let file_stem = Path::new(&session.file_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let mut models_set: BTreeSet<String> = BTreeSet::new();
    if let Some(model) = model_from_stem(file_stem) {
        models_set.insert(model);
    }

    // 旁车 .meta：会话名、workspace_root、时间戳。
    let meta_path = format!("{}.meta", session.file_path);
    if let Ok(content) = fs::read_to_string(&meta_path) {
        if let Ok(branch) = serde_json::from_str::<ReasonixBranchMeta>(&content) {
            if let Some(ws) = branch.workspace_root.filter(|value| !value.is_empty()) {
                meta.cwd = Some(ws.clone());
                meta.project_name = extract_project_name(&ws);
            }
            meta.session_name = branch
                .name
                .filter(|value| !value.trim().is_empty())
                .or_else(|| branch.topic_title.filter(|value| !value.trim().is_empty()));
            if let Some(ts) = branch
                .created_at
                .as_deref()
                .and_then(parse_rfc3339_to_epoch)
            {
                meta.start_time = ts;
            }
            if let Some(ts) = branch
                .updated_at
                .as_deref()
                .and_then(parse_rfc3339_to_epoch)
            {
                meta.end_time = ts;
            }
            // .meta.id 优先作为可读 session_id 来源（仅当本地解析需要时）。
            let _ = branch.id;
        }
    }

    // 扫描消息：取首条 user 消息作为 topic，统计消息数。
    let mut first_user_message: Option<String> = None;
    let mut message_count: u64 = 0;
    if let Ok(file) = fs::File::open(&session.file_path) {
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(message) = serde_json::from_str::<ReasonixMessage>(trimmed) else {
                continue;
            };
            // 仅 assistant 消息计入请求数，与 Claude reader 语义一致
            //（message_count = 请求数 = assistant 消息数）。
            if message.role == "assistant" {
                message_count += 1;
            }
            if message.role == "user"
                && first_user_message.is_none()
                && !message.content.trim().is_empty()
            {
                first_user_message = Some(message.content.clone());
            }
        }
    }

    meta.topic = first_user_message.map(|text| truncate_string(&text, 50));
    meta.models = models_set.into_iter().collect();
    meta.message_count = message_count;

    // Reasonix 本地文件不含 token：用量字段全部保持 0，requests 为空。
    ReasonixParsedData {
        meta,
        requests: Vec::new(),
    }
}

/// 从 `<timestamp>-<model>` 文件名 stem 提取模型名。
/// timestamp 形如 `20260604-054135.294110000`（含一个内部 `-`），
/// 因此模型名为第 3 段及之后用 `-` 连接的部分。
fn model_from_stem(stem: &str) -> Option<String> {
    let parts: Vec<&str> = stem.splitn(3, '-').collect();
    if parts.len() < 3 {
        return None;
    }
    let model = parts[2].trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

fn parse_rfc3339_to_epoch(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn make_session(file_path: &str, file_stem: &str) -> SessionFile {
        SessionFile {
            session_id: format!("reasonix::{file_stem}"),
            tool: super::super::constants::TOOL_REASONIX.to_string(),
            project_path: String::new(),
            file_path: file_path.to_string(),
            transcript_paths: vec![file_path.to_string()],
            file_size: 0,
            last_modified: 1_700_000_000,
            fingerprint: 0,
        }
    }

    #[test]
    fn model_from_stem_extracts_model_after_timestamp() {
        assert_eq!(
            model_from_stem("20260604-054135.294110000-deepseek-v4-pro"),
            Some("deepseek-v4-pro".to_string())
        );
        assert_eq!(model_from_stem("nofields"), None);
    }

    #[test]
    fn parse_reasonix_session_produces_metadata_without_tokens() {
        let temp = tempdir().unwrap();
        let stem = "20260604-054135.294110000-deepseek-v4-pro";
        let jsonl_path = temp.path().join(format!("{stem}.jsonl"));
        {
            let mut file = fs::File::create(&jsonl_path).unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"role":"system","content":"You are Reasonix"})
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"role":"user","content":"Review my refactor"})
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"role":"assistant","content":"Sure"})
            )
            .unwrap();
        }
        {
            let mut meta_file =
                fs::File::create(temp.path().join(format!("{stem}.jsonl.meta"))).unwrap();
            write!(
                meta_file,
                "{}",
                serde_json::json!({
                    "id": stem,
                    "name": "Refactor review",
                    "workspace_root": "/Users/test/work/project-alpha",
                    "created_at": "2026-06-04T05:48:13.639324993Z",
                    "updated_at": "2026-06-04T06:00:00Z"
                })
            )
            .unwrap();
        }

        let session = make_session(&jsonl_path.to_string_lossy(), stem);
        let data = parse_reasonix_session_file(&session);

        assert_eq!(data.meta.tool, "reasonix");
        assert_eq!(data.meta.models, vec!["deepseek-v4-pro".to_string()]);
        assert_eq!(data.meta.session_name, Some("Refactor review".to_string()));
        assert_eq!(data.meta.topic, Some("Review my refactor".to_string()));
        assert_eq!(data.meta.project_name, Some("project-alpha".to_string()));
        assert_eq!(
            data.meta.cwd,
            Some("/Users/test/work/project-alpha".to_string())
        );
        assert_eq!(data.meta.message_count, 1); // 仅 1 条 assistant 消息 = 1 次请求
                                                // 关键：本地链不产出 token 事实。
        assert_eq!(data.requests.len(), 0);
        assert_eq!(data.meta.total_input_tokens, 0);
        assert_eq!(data.meta.total_output_tokens, 0);
    }
}
