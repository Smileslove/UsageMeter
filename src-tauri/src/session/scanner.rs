//! 会话文件扫描器
//!
//! 扫描 ~/.claude/projects/ 目录下的 JSONL 会话文件
//! 并从中提取元数据。
//!
//! 使用增量缓存策略：
//! - 首次调用：全量扫描，构建缓存
//! - 后续调用：只扫描新增/修改的文件

use super::meta::{SessionFile, SessionMeta};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// 缓存条目
struct CacheEntry {
    /// 缓存数据
    data: Vec<SessionMeta>,
    /// message_id -> session_id 的索引（用于快速查找）
    message_to_session: std::collections::HashMap<String, String>,
    /// 文件路径 -> 最后修改时间（用于增量更新检测）
    file_mtimes: std::collections::HashMap<PathBuf, i64>,
}

/// 全局会话元数据缓存
static SESSION_CACHE: OnceLock<Arc<Mutex<Option<CacheEntry>>>> = OnceLock::new();

/// 获取缓存实例
fn get_cache() -> &'static Arc<Mutex<Option<CacheEntry>>> {
    SESSION_CACHE.get_or_init(|| Arc::new(Mutex::new(None)))
}

/// 获取会话元数据（带增量缓存）
///
/// 首次调用：全量扫描，初始化缓存
/// 后续调用：只扫描新增/修改的文件，增量更新缓存
pub fn get_all_session_meta_cached() -> Vec<SessionMeta> {
    let cache = get_cache();

    // 检查缓存是否存在
    {
        let cache_guard = cache.lock().unwrap();
        if cache_guard.is_some() {
            // 缓存存在，执行增量更新
            drop(cache_guard);
            return incremental_update_cache();
        }
    }

    // 缓存不存在，执行全量扫描
    full_scan_and_cache()
}

/// 全量扫描并初始化缓存
fn full_scan_and_cache() -> Vec<SessionMeta> {
    let cache = get_cache();
    let session_files = scan_session_files();
    let data: Vec<SessionMeta> = session_files.iter().map(extract_session_meta).collect();

    // 构建 message_id -> session_id 索引
    let mut message_to_session = std::collections::HashMap::new();
    for meta in &data {
        for msg_id in &meta.message_ids {
            message_to_session.insert(msg_id.clone(), meta.session_id.clone());
        }
    }

    // 构建文件修改时间映射
    let mut file_mtimes = std::collections::HashMap::new();
    for file in &session_files {
        file_mtimes.insert(PathBuf::from(&file.file_path), file.last_modified);
    }

    // 更新缓存
    {
        let mut cache_guard = cache.lock().unwrap();
        *cache_guard = Some(CacheEntry {
            data: data.clone(),
            message_to_session,
            file_mtimes,
        });
    }

    data
}

/// 增量更新缓存
///
/// 只扫描新增或修改的文件，更新缓存
fn incremental_update_cache() -> Vec<SessionMeta> {
    let cache = get_cache();

    // 获取当前文件列表和修改时间
    let current_files = scan_session_files();
    let current_mtimes: std::collections::HashMap<PathBuf, i64> = current_files
        .iter()
        .map(|f| (PathBuf::from(&f.file_path), f.last_modified))
        .collect();

    // 获取缓存并检查变化
    let (new_files, modified_files, deleted_paths) = {
        let cache_guard = cache.lock().unwrap();
        let entry = match cache_guard.as_ref() {
            Some(e) => e,
            None => return full_scan_and_cache(),
        };

        let mut new_files: Vec<SessionFile> = Vec::new();
        let mut modified_files: Vec<SessionFile> = Vec::new();
        let mut deleted_paths: Vec<PathBuf> = Vec::new();

        // 检查缓存中的文件是否被删除
        for path in entry.file_mtimes.keys() {
            if !current_mtimes.contains_key(path) {
                deleted_paths.push(path.clone());
            }
        }

        // 检查新文件和修改的文件
        for file in &current_files {
            let path = PathBuf::from(&file.file_path);
            match entry.file_mtimes.get(&path) {
                None => {
                    // 新文件
                    new_files.push(file.clone());
                }
                Some(&cached_mtime) if cached_mtime != file.last_modified => {
                    // 修改的文件
                    modified_files.push(file.clone());
                }
                _ => {
                    // 未变化的文件，跳过
                }
            }
        }

        (new_files, modified_files, deleted_paths)
    };

    // 如果没有变化，直接返回缓存
    if new_files.is_empty() && modified_files.is_empty() && deleted_paths.is_empty() {
        let cache_guard = cache.lock().unwrap();
        return cache_guard
            .as_ref()
            .map(|e| e.data.clone())
            .unwrap_or_default();
    }

    // 有变化，更新缓存
    let mut cache_guard = cache.lock().unwrap();
    let entry = match cache_guard.as_mut() {
        Some(e) => e,
        None => return full_scan_and_cache(),
    };

    // 移除已删除文件的 message_ids
    for path in &deleted_paths {
        // 找到对应的 session_id
        if let Some(meta) = entry
            .data
            .iter()
            .find(|m| m.file_path == path.to_string_lossy())
        {
            // 移除该 session 的 message_ids
            for msg_id in &meta.message_ids {
                entry.message_to_session.remove(msg_id);
            }
        }
    }

    // 从 data 中移除已删除的会话
    entry.data.retain(|m| {
        !deleted_paths
            .iter()
            .any(|p| m.file_path == p.to_string_lossy())
    });

    // 移除已删除文件的 mtimes
    for path in &deleted_paths {
        entry.file_mtimes.remove(path);
    }

    // 处理修改的文件：先移除旧数据，再添加新数据
    for file in &modified_files {
        let path = PathBuf::from(&file.file_path);

        // 移除旧的 message_ids
        if let Some(meta) = entry.data.iter().find(|m| m.file_path == file.file_path) {
            for msg_id in &meta.message_ids {
                entry.message_to_session.remove(msg_id);
            }
        }

        // 从 data 中移除旧的会话数据
        entry.data.retain(|m| m.file_path != file.file_path);

        // 提取新的会话数据
        let new_meta = extract_session_meta(file);

        // 添加新的 message_ids
        for msg_id in &new_meta.message_ids {
            entry
                .message_to_session
                .insert(msg_id.clone(), new_meta.session_id.clone());
        }

        // 添加新的会话数据
        entry.data.push(new_meta);

        // 更新 mtime
        entry.file_mtimes.insert(path, file.last_modified);
    }

    // 处理新文件
    for file in &new_files {
        let path = PathBuf::from(&file.file_path);
        let new_meta = extract_session_meta(file);

        // 添加 message_ids
        for msg_id in &new_meta.message_ids {
            entry
                .message_to_session
                .insert(msg_id.clone(), new_meta.session_id.clone());
        }

        // 添加会话数据
        entry.data.push(new_meta);

        // 添加 mtime
        entry.file_mtimes.insert(path, file.last_modified);
    }

    // 按修改时间排序（最新的在前）
    entry
        .data
        .sort_by_key(|m| std::cmp::Reverse(m.last_modified));

    entry.data.clone()
}

/// 通过 message_id 查找对应的 session_id（使用缓存索引）
///
/// 使用 HashMap 索引，O(1) 时间复杂度
pub fn find_session_id_by_message_id(message_id: &str) -> Option<String> {
    let cache = get_cache();

    // 确保缓存已初始化
    {
        let cache_guard = cache.lock().unwrap();
        if cache_guard.is_none() {
            drop(cache_guard);
            // 初始化缓存
            get_all_session_meta_cached();
        }
    }

    // 从缓存索引查找
    let cache_guard = cache.lock().unwrap();
    if let Some(entry) = cache_guard.as_ref() {
        entry.message_to_session.get(message_id).cloned()
    } else {
        None
    }
}

/// 清除缓存（用于强制刷新）
#[allow(dead_code)]
pub fn invalidate_cache() {
    let cache = get_cache();
    let mut cache_guard = cache.lock().unwrap();
    *cache_guard = None;
}

/// 扫描 Claude 项目目录中的所有会话文件
pub fn scan_session_files() -> Vec<SessionFile> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".claude").join("projects"));
        roots.push(home.join(".config").join("claude").join("projects"));
    }

    let mut sessions = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }

        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let project_path = entry.path();
                if !project_path.is_dir() {
                    continue;
                }

                let project_name = project_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                // 扫描项目目录中的 JSONL 文件
                if let Ok(files) = fs::read_dir(&project_path) {
                    for file in files.flatten() {
                        let path = file.path();
                        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                            continue;
                        }

                        let session_id = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");

                        // 生成唯一 ID：project_name::session_id
                        let unique_id = format!("{}::{}", project_name, session_id);

                        let metadata = fs::metadata(&path).ok();
                        let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                        let last_modified = metadata
                            .and_then(|m| m.modified().ok())
                            .map(|t| {
                                t.duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs() as i64
                            })
                            .unwrap_or(0);

                        // 跳过太小的文件（可能是空会话）
                        if file_size < 100 {
                            continue;
                        }

                        sessions.push(SessionFile {
                            session_id: unique_id,
                            project_path: project_name.to_string(),
                            file_path: path.to_string_lossy().to_string(),
                            file_size,
                            last_modified,
                        });
                    }
                }
            }
        }
    }

    // 按修改时间排序（最新的在前）
    sessions.sort_by_key(|b| std::cmp::Reverse(b.last_modified));
    sessions
}

/// 检查用户消息是否为系统/元消息，应跳过
fn is_system_message(text: &str) -> bool {
    let trimmed = text.trim();
    // 跳过本地命令提示
    if trimmed.starts_with("<local-command-caveat>") || trimmed.contains("<local-command-caveat>") {
        return true;
    }
    // 跳过命令调用
    if trimmed.starts_with("<command-name>") || trimmed.contains("<command-name>") {
        return true;
    }
    // 跳过命令输出
    if trimmed.starts_with("<local-command-stdout>") || trimmed.contains("<local-command-stdout>") {
        return true;
    }
    // 跳过系统提醒
    if trimmed.starts_with("<system-reminder>") || trimmed.contains("<system-reminder>") {
        return true;
    }
    // 跳过过短消息（可能只是空白或单字符）
    if trimmed.chars().count() < 3 {
        return true;
    }
    false
}

/// 从 cwd 路径提取项目名称
fn extract_project_name(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    // 获取路径的最后一部分
    let normalized = cwd.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    parts.last().map(|s| s.to_string())
}

/// 从 JSONL 文件提取会话元数据
pub fn extract_session_meta(file: &SessionFile) -> SessionMeta {
    let mut meta = SessionMeta {
        session_id: file.session_id.clone(),
        cwd: None,
        project_name: None,
        topic: None,
        last_prompt: None,
        session_name: None,
        file_path: file.file_path.clone(),
        file_size: file.file_size,
        last_modified: file.last_modified,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_create_tokens: 0,
        total_cache_read_tokens: 0,
        models: Vec::new(),
        message_count: 0,
        start_time: 0,
        end_time: 0,
        source: "jsonl".to_string(),
        message_ids: Vec::new(),
    };

    // 打开并读取文件
    let file_handle = match fs::File::open(&file.file_path) {
        Ok(f) => f,
        Err(_) => return meta,
    };
    let reader = BufReader::new(file_handle);

    let mut first_user_message: Option<String> = None;
    let mut last_user_message: Option<String> = None;
    let mut cwd_found: Option<String> = None;
    let mut session_name_found: Option<String> = None;
    let mut models_set: HashSet<String> = HashSet::new();
    let mut first_timestamp: Option<i64> = None;
    let mut last_timestamp: Option<i64> = None;

    // 解析每一行
    for line in reader.lines().map_while(Result::ok) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            // 提取 cwd（仅从首次出现）
            if cwd_found.is_none() {
                if let Some(cwd) = json.get("cwd").and_then(|v| v.as_str()) {
                    cwd_found = Some(cwd.to_string());
                }
            }

            // 提取会话名称（slug 或 customTitle）
            if session_name_found.is_none() {
                if let Some(slug) = json.get("slug").and_then(|v| v.as_str()) {
                    session_name_found = Some(slug.to_string());
                }
                if let Some(title) = json.get("customTitle").and_then(|v| v.as_str()) {
                    session_name_found = Some(title.to_string());
                }
            }

            // 提取时间戳
            if let Some(ts) = extract_timestamp(&json) {
                if first_timestamp.is_none() {
                    first_timestamp = Some(ts);
                }
                last_timestamp = Some(ts);
            }

            // 提取用户消息
            let msg_type = json.get("type").and_then(|v| v.as_str());
            if msg_type == Some("user") || msg_type == Some("human") {
                if let Some(text) = extract_user_text(&json) {
                    // 跳过系统消息作为首个用户消息（话题）
                    if first_user_message.is_none() && !is_system_message(&text) {
                        first_user_message = Some(text.clone());
                    }
                    // 始终跟踪最后用户消息（但跳过系统消息）
                    if !is_system_message(&text) {
                        last_user_message = Some(text);
                    }
                }
                meta.message_count += 1;
            }

            // 从消息或 usage 字段提取 Token 使用量
            if let Some(usage) = extract_token_usage(&json) {
                meta.total_input_tokens += usage.input;
                meta.total_output_tokens += usage.output;
                meta.total_cache_create_tokens += usage.cache_create;
                meta.total_cache_read_tokens += usage.cache_read;
            }

            // 提取模型
            if let Some(model) = extract_model(&json) {
                models_set.insert(model);
            }

            // 从 assistant 消息提取 message.id（用于关联代理数据）
            // message.id 格式如 "msg_73424337149139748084"，与代理数据库中的 message_id 一致
            if msg_type == Some("assistant") {
                if let Some(msg_id) = json
                    .get("message")
                    .and_then(|m| m.get("id"))
                    .and_then(|v| v.as_str())
                {
                    meta.message_ids.push(msg_id.to_string());
                }
            }
        }
    }

    // 设置提取结果并进行截断处理
    meta.cwd = cwd_found.clone();
    meta.project_name = cwd_found.as_ref().and_then(|cwd| extract_project_name(cwd));
    meta.topic = first_user_message.map(|s| truncate_string(&s, 50));
    meta.last_prompt = last_user_message.map(|s| truncate_string(&s, 100));

    // 保留 session_name 以向后兼容（将作为 UI 中的备用显示）
    // 现在我们优先使用分离的 project_name + topic 显示
    meta.session_name = session_name_found;

    meta.models = models_set.into_iter().collect();
    meta.start_time = first_timestamp.unwrap_or(file.last_modified);
    meta.end_time = last_timestamp.unwrap_or(file.last_modified);

    meta
}

/// 从 JSON 消息提取用户文本
fn extract_user_text(json: &serde_json::Value) -> Option<String> {
    // 尝试从 message.content 获取
    if let Some(message) = json.get("message") {
        // 字符串内容
        if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
            return Some(content.to_string());
        }

        // 数组内容（内容块）
        if let Some(content_arr) = message.get("content").and_then(|v| v.as_array()) {
            for item in content_arr {
                if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                        return Some(text.to_string());
                    }
                }
            }
        }
    }

    None
}

/// 从 JSON 提取的 Token 使用量
struct TokenUsage {
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
}

/// 从 JSON 消息提取 Token 使用量
fn extract_token_usage(json: &serde_json::Value) -> Option<TokenUsage> {
    // 先尝试 message.usage
    let usage = json
        .get("message")
        .and_then(|m| m.get("usage"))
        .or_else(|| json.get("usage"));

    if let Some(usage) = usage {
        let input = usage
            .get("input_tokens")
            .or_else(|| usage.get("inputTokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let output = usage
            .get("output_tokens")
            .or_else(|| usage.get("outputTokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let cache_create = usage
            .get("cache_creation_input_tokens")
            .or_else(|| usage.get("cacheCreationInputTokens"))
            .or_else(|| usage.get("cache_create_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let cache_read = usage
            .get("cache_read_input_tokens")
            .or_else(|| usage.get("cacheReadInputTokens"))
            .or_else(|| usage.get("cache_read_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if input > 0 || output > 0 {
            return Some(TokenUsage {
                input,
                output,
                cache_create,
                cache_read,
            });
        }
    }

    None
}

/// 从 JSON 提取模型名称
fn extract_model(json: &serde_json::Value) -> Option<String> {
    // 先尝试 message.model
    let model = json
        .get("message")
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .or_else(|| json.get("model").and_then(|v| v.as_str()));

    // 过滤掉合成/内部模型名称
    if let Some(m) = model {
        // 跳过合成模型（无响应的内部消息）
        if m.starts_with('<') && m.ends_with('>') {
            return None;
        }
        // 跳过空或占位符模型名称
        if m.is_empty() || m == "unknown" {
            return None;
        }
        return Some(m.to_string());
    }

    None
}

/// 从 JSON 提取时间戳
fn extract_timestamp(json: &serde_json::Value) -> Option<i64> {
    // 尝试各种时间戳字段
    let ts = json
        .get("timestamp")
        .or_else(|| json.get("createdAt"))
        .or_else(|| json.get("created_at"))
        .or_else(|| json.get("time"));

    if let Some(ts) = ts {
        // 尝试作为数字（可能是秒或毫秒）
        if let Some(num) = ts.as_u64() {
            // 如果大于 100 亿，假设为毫秒
            return Some(if num > 10_000_000_000 {
                (num / 1000) as i64
            } else {
                num as i64
            });
        }
        // 尝试作为字符串（ISO 格式）
        if let Some(s) = ts.as_str() {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                return Some(dt.timestamp());
            }
        }
    }

    None
}

/// 将字符串截断到最大长度，如被截断则添加省略号
fn truncate_string(s: &str, max_len: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_len {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

/// 根据会话 ID 获取会话元数据
pub fn get_session_meta_by_id(session_id: &str) -> Option<SessionMeta> {
    let files = scan_session_files();

    // 先尝试精确匹配
    for file in &files {
        if file.session_id == session_id {
            return Some(extract_session_meta(file));
        }
    }

    // 尝试匹配 session_id 的最后一部分（:: 之后）
    let id_suffix = session_id.split("::").last().unwrap_or(session_id);
    for file in &files {
        let file_suffix = file
            .session_id
            .split("::")
            .last()
            .unwrap_or(&file.session_id);
        if file_suffix == id_suffix {
            return Some(extract_session_meta(file));
        }
    }

    None
}

/// 获取所有会话元数据（限制数量）
#[allow(dead_code)]
pub fn get_all_session_meta(limit: usize) -> Vec<SessionMeta> {
    scan_session_files()
        .into_iter()
        .take(limit)
        .map(|f| extract_session_meta(&f))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 5), "hello...");
    }

    #[test]
    fn test_is_system_message() {
        // 应跳过系统消息
        assert!(is_system_message("<local-command-caveat>some text"));
        assert!(is_system_message("prefix <local-command-caveat> content"));
        assert!(is_system_message("<command-name>/model</command-name>"));
        assert!(is_system_message(
            "<local-command-stdout>Set model to...</local-command-stdout>"
        ));
        assert!(is_system_message(
            "<system-reminder>Some reminder</system-reminder>"
        ));
        assert!(is_system_message("ab")); // 太短

        // 不应跳过真实用户消息
        assert!(!is_system_message("请帮我分析这段代码"));
        assert!(!is_system_message("How do I fix this bug?"));
        assert!(!is_system_message("分析一下项目结构"));
    }

    #[test]
    fn test_extract_project_name() {
        assert_eq!(
            extract_project_name("/Users/test/projects/my-app"),
            Some("my-app".to_string())
        );
        assert_eq!(
            extract_project_name("/home/user/code/UsageMeter"),
            Some("UsageMeter".to_string())
        );
        assert_eq!(
            extract_project_name("C:\\Users\\test\\project"),
            Some("project".to_string())
        );
        assert_eq!(extract_project_name(""), None);
        assert_eq!(extract_project_name("/"), None);
    }
}
