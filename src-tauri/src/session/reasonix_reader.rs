//! Reasonix 本地会话读取模块（仅元数据补全）
//!
//! Reasonix v2 的本地会话文件 `<config_dir>/reasonix/sessions/<ts>-<model>.jsonl`
//! 每行只是一条对话消息（role/content/reasoning_content/tool_calls），
//! transcript 本体 **不包含任何 per-request token 用量**。
//!
//! 因此本适配器只产出会话/项目级元数据（标题、消息数、模型、时间、cwd），
//! `requests` 恒为空——准确 per-request 事实与性能指标一律由代理链提供。
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
    /// Reasonix v2 新增：会话作用域（"project" | "global"）
    #[serde(default)]
    scope: Option<String>,
}

/// Reasonix 会话 JSONL 单行消息（仅取需要的字段）
#[derive(Debug, Default, Deserialize)]
struct ReasonixMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: String,
}

#[derive(Debug, Default, Deserialize)]
struct ReasonixTurnFileRef {
    #[serde(default)]
    path: String,
}

#[derive(Debug, Default, Deserialize)]
struct ReasonixTurnCheckpoint {
    #[serde(default)]
    files: Vec<ReasonixTurnFileRef>,
}

fn reasonix_session_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut reasonix_bases: Vec<PathBuf> = Vec::new();

    if let Some(config_dir) = dirs::config_dir() {
        reasonix_bases.push(config_dir.join("reasonix"));
    }
    // Linux 上 dirs::config_dir() 即 ~/.config；其它平台额外兼容 ~/.config/reasonix。
    if let Some(home) = dirs::home_dir() {
        let xdg = home.join(".config").join("reasonix");
        if !reasonix_bases.contains(&xdg) {
            reasonix_bases.push(xdg);
        }
    }

    for base in &reasonix_bases {
        // 全局会话目录
        roots.push(base.join("sessions"));

        // 项目级会话目录：<config_dir>/reasonix/projects/<slug>/sessions/
        // Reasonix v2 在项目目录启动时（CLI 和桌面端）将会话存于此处。
        let projects_dir = base.join("projects");
        if let Ok(read_dir) = fs::read_dir(&projects_dir) {
            for entry in read_dir.flatten() {
                let slug_path = entry.path();
                if slug_path.is_dir() {
                    let sessions = slug_path.join("sessions");
                    if !roots.contains(&sessions) {
                        roots.push(sessions);
                    }
                }
            }
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

        // 用父目录路径的 hash 作为 session_id 的目录标识，避免全局目录与
        // projects/<slug>/sessions/ 下同名文件产生相同 ID 导致 transcript_map 覆盖。
        let dir_hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            root.to_string_lossy().hash(&mut h);
            format!("{:x}", std::hash::Hasher::finish(&h))
        };
        let unique_id = format!(
            "{}::{}::{}",
            super::constants::TOOL_REASONIX,
            dir_hash,
            file_stem
        );

        let path_string = path.to_string_lossy().to_string();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        path_string.hash(&mut hasher);
        file_size.hash(&mut hasher);
        last_modified.hash(&mut hasher);
        hash_reasonix_session_companions(&path, &mut hasher);

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
            if let Some(scope) = branch.scope.filter(|s| !s.trim().is_empty()) {
                meta.scope = Some(scope);
            }
            let _ = branch.id;
        }
    }
    if meta.cwd.is_none() {
        if let Some(ws) = infer_workspace_root_from_checkpoint(&session.file_path) {
            meta.project_name = extract_project_name(&ws);
            meta.cwd = Some(ws);
        }
    }

    // 扫描消息：取首条 user 消息作为 topic，统计消息数。
    let mut first_user_message: Option<String> = None;
    let mut last_user_message: Option<String> = None;
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
            if message.role == "user" && !message.content.trim().is_empty() {
                last_user_message = Some(message.content.clone());
            }
        }
    }

    meta.topic = first_user_message.map(|text| truncate_string(&text, 50));
    meta.last_prompt = last_user_message.map(|text| truncate_string(&text, 100));
    meta.models = models_set.into_iter().collect();
    meta.message_count = message_count;

    // Reasonix transcript 本体不含 per-request token：用量字段保持 0，requests 为空。
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
    if parts[0].len() != 8 || !parts[0].chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    if !parts[1]
        .chars()
        .next()
        .map(|ch| ch.is_ascii_digit())
        .unwrap_or(false)
    {
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

fn hash_reasonix_session_companions(
    session_path: &Path,
    hasher: &mut std::collections::hash_map::DefaultHasher,
) {
    let meta_path = PathBuf::from(format!("{}.meta", session_path.to_string_lossy()));
    hash_path_metadata(&meta_path, hasher);

    if let Some(checkpoint_dir) = checkpoint_dir_for_session(&session_path.to_string_lossy()) {
        let Ok(read_dir) = fs::read_dir(checkpoint_dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = read_dir.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for entry_path in entries {
            hash_path_metadata(&entry_path, hasher);
        }
    }
}

fn hash_path_metadata(path: &Path, hasher: &mut std::collections::hash_map::DefaultHasher) {
    path.to_string_lossy().hash(hasher);
    if let Ok(metadata) = fs::metadata(path) {
        metadata.len().hash(hasher);
        metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
            .hash(hasher);
    }
}

fn infer_workspace_root_from_checkpoint(session_file_path: &str) -> Option<String> {
    let checkpoint_dir = checkpoint_dir_for_session(session_file_path)?;
    let read_dir = fs::read_dir(checkpoint_dir).ok()?;
    let mut candidate_dirs = Vec::new();

    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !file_name.starts_with("turn-") || !file_name.ends_with(".json") {
            continue;
        }
        if let Some(dir) = infer_workspace_root_from_turn_file(&path) {
            candidate_dirs.push(dir);
        }
    }

    select_workspace_root(&candidate_dirs).map(|path| path.to_string_lossy().to_string())
}

fn checkpoint_dir_for_session(session_file_path: &str) -> Option<PathBuf> {
    let session_path = Path::new(session_file_path);
    let parent = session_path.parent()?;
    let stem = session_path.file_stem()?.to_str()?;
    Some(parent.join(format!("{stem}.ckpt")))
}

fn infer_workspace_root_from_turn_file(turn_path: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(turn_path).ok()?;
    let checkpoint = serde_json::from_str::<ReasonixTurnCheckpoint>(&content).ok()?;
    let mut parent_dirs = Vec::new();

    for file in checkpoint.files {
        let trimmed = file.path.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = PathBuf::from(trimmed);
        let parent = path.parent()?.to_path_buf();
        parent_dirs.push(parent);
    }

    select_workspace_root(&parent_dirs)
}

fn select_workspace_root(paths: &[PathBuf]) -> Option<PathBuf> {
    let common = longest_common_directory(paths).filter(|path| is_trusted_workspace_root(path));
    common.or_else(|| most_frequent_trusted_directory(paths))
}

fn most_frequent_trusted_directory(paths: &[PathBuf]) -> Option<PathBuf> {
    use std::collections::BTreeMap;

    let mut counts: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for path in paths {
        if !is_trusted_workspace_root(path) {
            continue;
        }
        *counts.entry(path.clone()).or_default() += 1;
    }

    counts
        .into_iter()
        .max_by(|(left_path, left_count), (right_path, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| {
                    left_path
                        .components()
                        .count()
                        .cmp(&right_path.components().count())
                })
                // When usage count and depth are identical, prefer the lexicographically
                // smaller path so workspace-root inference stays deterministic.
                .then_with(|| right_path.cmp(left_path))
        })
        .map(|(path, _)| path)
}

fn is_trusted_workspace_root(path: &Path) -> bool {
    let depth = path.components().count();
    depth >= 4
}

fn longest_common_directory(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut iter = paths.iter();
    let first = iter.next()?.clone();
    let mut prefix = first;

    for path in iter {
        prefix = common_path_prefix(&prefix, path)?;
    }

    Some(prefix)
}

fn common_path_prefix(left: &Path, right: &Path) -> Option<PathBuf> {
    let mut prefix = PathBuf::new();
    let mut matched_any = false;

    for (lhs, rhs) in left.components().zip(right.components()) {
        if lhs != rhs {
            break;
        }
        prefix.push(lhs.as_os_str());
        matched_any = true;
    }

    if matched_any {
        Some(prefix)
    } else {
        None
    }
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
    fn parse_reasonix_session_reads_v2_sidecar_metadata() {
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
        assert_eq!(
            data.meta.last_prompt,
            Some("Review my refactor".to_string())
        );
        assert_eq!(data.requests.len(), 0);
        assert_eq!(data.meta.total_input_tokens, 0);
        assert_eq!(data.meta.total_output_tokens, 0);
    }

    #[test]
    fn parse_reasonix_session_falls_back_to_checkpoint_workspace_root() {
        let temp = tempdir().unwrap();
        let stem = "20260605-022603.538980000-mimo-v2.5";
        let jsonl_path = temp.path().join(format!("{stem}.jsonl"));
        {
            let mut file = fs::File::create(&jsonl_path).unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"role":"user","content":"Update my deck"})
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({"role":"assistant","content":"Working on it"})
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
                    "created_at": "2026-06-05T02:31:16.583226433Z",
                    "updated_at": "2026-06-05T02:47:05.124594Z"
                })
            )
            .unwrap();
        }
        let checkpoint_dir = temp.path().join(format!("{stem}.ckpt"));
        fs::create_dir_all(&checkpoint_dir).unwrap();
        {
            let mut turn_file = fs::File::create(checkpoint_dir.join("turn-0.json")).unwrap();
            write!(
                turn_file,
                "{}",
                serde_json::json!({
                    "files": [
                        {"path": "/Users/test/work/project-beta/index.html"},
                        {"path": "/Users/test/work/project-beta/css/custom.css"}
                    ]
                })
            )
            .unwrap();
        }

        let session = make_session(&jsonl_path.to_string_lossy(), stem);
        let data = parse_reasonix_session_file(&session);

        assert_eq!(
            data.meta.cwd,
            Some("/Users/test/work/project-beta".to_string())
        );
        assert_eq!(data.meta.project_name, Some("project-beta".to_string()));
    }

    #[test]
    fn workspace_root_ignores_shallow_common_prefixes() {
        let root = select_workspace_root(&[
            PathBuf::from("/Users/test/project-alpha/src"),
            PathBuf::from("/Users/test/project-beta/docs"),
        ]);

        assert_eq!(root, Some(PathBuf::from("/Users/test/project-alpha/src")));
    }

    #[test]
    fn workspace_root_tie_breaker_is_stable_when_count_and_depth_match() {
        let root = select_workspace_root(&[
            PathBuf::from("/Users/test/project-beta/docs"),
            PathBuf::from("/Users/test/project-alpha/src"),
        ]);

        assert_eq!(root, Some(PathBuf::from("/Users/test/project-alpha/src")));
    }

    #[test]
    fn collect_session_files_from_project_sessions_dir() {
        let temp = tempdir().unwrap();
        // 模拟 projects/<slug>/sessions/ 结构
        let slug_dir = temp
            .path()
            .join("projects")
            .join("-Users-test-work-my-project")
            .join("sessions");
        fs::create_dir_all(&slug_dir).unwrap();

        let stem = "20260610-090000.000000000-deepseek-v4-pro";
        let jsonl_path = slug_dir.join(format!("{stem}.jsonl"));
        {
            let mut f = fs::File::create(&jsonl_path).unwrap();
            writeln!(
                f,
                "{}",
                serde_json::json!({"role":"user","content":"hello"})
            )
            .unwrap();
            writeln!(
                f,
                "{}",
                serde_json::json!({"role":"assistant","content":"hi"})
            )
            .unwrap();
        }

        let sessions = collect_reasonix_session_files(&slug_dir);
        assert_eq!(sessions.len(), 1);

        let session = &sessions[0];
        // session_id 包含目录 hash，不会与全局目录下同名文件碰撞
        assert!(session.session_id.starts_with("reasonix::"));
        assert!(session.session_id.ends_with(stem));
        assert_eq!(session.tool, "reasonix");

        // 解析验证
        let data = parse_reasonix_session_file(session);
        assert_eq!(data.meta.models, vec!["deepseek-v4-pro".to_string()]);
        assert_eq!(data.meta.message_count, 1);
    }

    #[test]
    fn session_ids_differ_across_global_and_project_dirs() {
        let temp = tempdir().unwrap();
        let global_dir = temp.path().join("sessions");
        let project_dir = temp
            .path()
            .join("projects")
            .join("-Users-test-work-proj")
            .join("sessions");
        fs::create_dir_all(&global_dir).unwrap();
        fs::create_dir_all(&project_dir).unwrap();

        let stem = "20260610-100000.000000000-deepseek-v4-pro";
        for dir in [&global_dir, &project_dir] {
            let mut f = fs::File::create(dir.join(format!("{stem}.jsonl"))).unwrap();
            writeln!(f, "{}", serde_json::json!({"role":"user","content":"x"})).unwrap();
            writeln!(
                f,
                "{}",
                serde_json::json!({"role":"assistant","content":"y"})
            )
            .unwrap();
        }

        let global_sessions = collect_reasonix_session_files(&global_dir);
        let project_sessions = collect_reasonix_session_files(&project_dir);
        assert_eq!(global_sessions.len(), 1);
        assert_eq!(project_sessions.len(), 1);

        assert_ne!(
            global_sessions[0].session_id, project_sessions[0].session_id,
            "同名文件在不同目录下应产生不同的 session_id"
        );
    }

    #[test]
    fn scope_field_parsed_from_branch_meta() {
        let temp = tempdir().unwrap();
        let stem = "20260610-110000.000000000-deepseek-v4-pro";
        let jsonl_path = temp.path().join(format!("{stem}.jsonl"));
        {
            let mut f = fs::File::create(&jsonl_path).unwrap();
            writeln!(
                f,
                "{}",
                serde_json::json!({"role":"user","content":"hello"})
            )
            .unwrap();
            writeln!(
                f,
                "{}",
                serde_json::json!({"role":"assistant","content":"hi"})
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
                    "workspace_root": "/Users/test/work/proj",
                    "created_at": "2026-06-10T11:00:00Z",
                    "updated_at": "2026-06-10T11:30:00Z",
                    "scope": "project"
                })
            )
            .unwrap();
        }

        let session = make_session(&jsonl_path.to_string_lossy(), stem);
        let data = parse_reasonix_session_file(&session);
        assert_eq!(data.meta.scope, Some("project".to_string()));
    }
}
