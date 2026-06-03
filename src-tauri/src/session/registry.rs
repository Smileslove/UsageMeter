//! 本地会话 adapter 注册与调度层
//!
//! 当前仅统一文件型工具（Claude Code / Codex）的发现与解析。
//! OpenCode 仍走独立 SQLite 链路，但通过 scanner 统一汇总。

use super::claude_reader;
use super::codex_reader;
use super::meta::{LocalRequestRecord, SessionFile, SessionMeta};
use super::shared::extract_project_name;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

const TOOL_CODEX: &str = "codex";

pub fn scan_session_files() -> Vec<SessionFile> {
    let mut sessions = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for root in [
            home.join(".claude").join("projects"),
            home.join(".config").join("claude").join("projects"),
        ] {
            if root.exists() {
                sessions.extend(claude_reader::collect_claude_session_files_from_root(&root));
            }
        }

        let codex_root = home.join(".codex").join("sessions");
        if codex_root.exists() {
            sessions.extend(collect_codex_session_files(&codex_root));
        }
    }

    sessions.sort_by_key(|session| std::cmp::Reverse(session.last_modified));
    sessions
}

pub fn parse_session_file_for_storage(
    session: &SessionFile,
) -> (SessionMeta, Vec<LocalRequestRecord>) {
    if session.tool == TOOL_CODEX {
        let data = codex_reader::parse_codex_session_file(session);
        return (data.meta, data.requests);
    }

    claude_reader::parse_claude_session_file(session)
}

fn collect_codex_session_files(root: &Path) -> Vec<SessionFile> {
    #[derive(Default)]
    struct SessionGroupBuilder {
        session_id: String,
        project_path: String,
        primary_file_path: Option<String>,
        transcript_paths: Vec<String>,
        file_size: u64,
        last_modified: i64,
        fingerprint: u64,
    }

    let mut groups: HashMap<String, SessionGroupBuilder> = HashMap::new();

    for path in codex_reader::collect_codex_rollout_files(root) {
        let Some(identity) = codex_reader::inspect_codex_rollout_identity(&path) else {
            continue;
        };

        let metadata = std::fs::metadata(path.as_path()).ok();
        let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        if file_size < 10 {
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

        let unique_id = format!("{TOOL_CODEX}::{}", identity.root_session_id);
        let project_name = identity
            .cwd
            .as_deref()
            .and_then(extract_project_name)
            .unwrap_or_default();
        let group = groups
            .entry(unique_id.clone())
            .or_insert_with(|| SessionGroupBuilder {
                session_id: unique_id.clone(),
                project_path: project_name.to_string(),
                ..Default::default()
            });

        let path_string = path.to_string_lossy().to_string();
        if group.primary_file_path.is_none() || !identity.is_subagent {
            group.primary_file_path = Some(path_string.clone());
        }
        group.transcript_paths.push(path_string.clone());
        group.file_size += file_size;
        group.last_modified = group.last_modified.max(last_modified);

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        path_string.hash(&mut hasher);
        file_size.hash(&mut hasher);
        last_modified.hash(&mut hasher);
        group.fingerprint ^= hasher.finish();
    }

    groups
        .into_values()
        .map(|mut group| {
            group.transcript_paths.sort();
            SessionFile {
                session_id: group.session_id,
                tool: TOOL_CODEX.to_string(),
                project_path: group.project_path,
                file_path: group
                    .primary_file_path
                    .or_else(|| group.transcript_paths.first().cloned())
                    .unwrap_or_default(),
                transcript_paths: group.transcript_paths,
                file_size: group.file_size,
                last_modified: group.last_modified,
                fingerprint: group.fingerprint,
            }
        })
        .filter(|session| !session.file_path.is_empty() && !session.transcript_paths.is_empty())
        .collect()
}
