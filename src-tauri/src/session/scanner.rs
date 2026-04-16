//! Session file scanner
//!
//! Scans ~/.claude/projects/ directory for JSONL session files
//! and extracts metadata from them.

use super::meta::{SessionFile, SessionMeta};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};

/// Scan all session files from Claude projects directories
pub fn scan_session_files() -> Vec<SessionFile> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".claude").join("projects"));
        roots.push(home.join(".config").join("claude").join("projects"));
    }

    let mut sessions = Vec::new();

    for root in roots {
        if !root.exists() {
            eprintln!("[Session] Root does not exist: {:?}", root);
            continue;
        }

        eprintln!("[Session] Scanning root: {:?}", root);

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

                // Scan JSONL files in project directory
                if let Ok(files) = fs::read_dir(&project_path) {
                    for file in files.flatten() {
                        let path = file.path();
                        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                            continue;
                        }

                        let session_id = path
                            .file_stem()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");

                        // Generate unique ID: project_name::session_id
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

                        // Skip files that are too small (likely empty sessions)
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

    eprintln!("[Session] Found {} session files", sessions.len());

    // Sort by modification time (newest first)
    sessions.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    sessions
}

/// Check if a user message is a system/meta message that should be skipped
fn is_system_message(text: &str) -> bool {
    let trimmed = text.trim();
    // Skip local command caveat
    if trimmed.starts_with("<local-command-caveat>") || trimmed.contains("<local-command-caveat>") {
        return true;
    }
    // Skip command invocations
    if trimmed.starts_with("<command-name>") || trimmed.contains("<command-name>") {
        return true;
    }
    // Skip command stdout
    if trimmed.starts_with("<local-command-stdout>") || trimmed.contains("<local-command-stdout>") {
        return true;
    }
    // Skip system reminders
    if trimmed.starts_with("<system-reminder>") || trimmed.contains("<system-reminder>") {
        return true;
    }
    // Skip very short messages (likely just whitespace or single chars)
    if trimmed.chars().count() < 3 {
        return true;
    }
    false
}

/// Extract project name from cwd path
fn extract_project_name(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    // Get the last component of the path
    let normalized = cwd.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    parts.last().map(|s| s.to_string())
}

/// Extract session metadata from a JSONL file
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
    };

    // Open and read file
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

    // Parse each line
    for line in reader.lines().map_while(Result::ok) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            // Extract cwd (only from first occurrence)
            if cwd_found.is_none() {
                if let Some(cwd) = json.get("cwd").and_then(|v| v.as_str()) {
                    cwd_found = Some(cwd.to_string());
                }
            }

            // Extract session name (slug or customTitle)
            if session_name_found.is_none() {
                if let Some(slug) = json.get("slug").and_then(|v| v.as_str()) {
                    session_name_found = Some(slug.to_string());
                }
                if let Some(title) = json.get("customTitle").and_then(|v| v.as_str()) {
                    session_name_found = Some(title.to_string());
                }
            }

            // Extract timestamp
            if let Some(ts) = extract_timestamp(&json) {
                if first_timestamp.is_none() {
                    first_timestamp = Some(ts);
                }
                last_timestamp = Some(ts);
            }

            // Extract user messages
            let msg_type = json.get("type").and_then(|v| v.as_str());
            if msg_type == Some("user") || msg_type == Some("human") {
                if let Some(text) = extract_user_text(&json) {
                    // Skip system messages for first user message (topic)
                    if first_user_message.is_none() && !is_system_message(&text) {
                        first_user_message = Some(text.clone());
                    }
                    // Always track last user message (but skip system messages)
                    if !is_system_message(&text) {
                        last_user_message = Some(text);
                    }
                }
                meta.message_count += 1;
            }

            // Extract token usage from message or usage field
            if let Some(usage) = extract_token_usage(&json) {
                meta.total_input_tokens += usage.input;
                meta.total_output_tokens += usage.output;
                meta.total_cache_create_tokens += usage.cache_create;
                meta.total_cache_read_tokens += usage.cache_read;
            }

            // Extract model
            if let Some(model) = extract_model(&json) {
                models_set.insert(model);
            }
        }
    }

    // Set extracted results with truncation
    meta.cwd = cwd_found.clone();
    meta.project_name = cwd_found.as_ref().and_then(|cwd| extract_project_name(cwd));
    meta.topic = first_user_message.map(|s| truncate_string(&s, 50));
    meta.last_prompt = last_user_message.map(|s| truncate_string(&s, 100));

    // Keep session_name for backward compatibility (will be used as fallback in UI)
    // Now we prefer separate project_name + topic display
    meta.session_name = session_name_found;

    meta.models = models_set.into_iter().collect();
    meta.start_time = first_timestamp.unwrap_or(file.last_modified);
    meta.end_time = last_timestamp.unwrap_or(file.last_modified);

    meta
}

/// Extract user text from a JSON message
fn extract_user_text(json: &serde_json::Value) -> Option<String> {
    // Try to get from message.content
    if let Some(message) = json.get("message") {
        // String content
        if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
            return Some(content.to_string());
        }

        // Array content (content blocks)
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

/// Token usage extracted from JSON
struct TokenUsage {
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
}

/// Extract token usage from JSON message
fn extract_token_usage(json: &serde_json::Value) -> Option<TokenUsage> {
    // Try message.usage first
    let usage = json.get("message").and_then(|m| m.get("usage"))
        .or_else(|| json.get("usage"));

    if let Some(usage) = usage {
        let input = usage.get("input_tokens")
            .or_else(|| usage.get("inputTokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let output = usage.get("output_tokens")
            .or_else(|| usage.get("outputTokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let cache_create = usage.get("cache_creation_input_tokens")
            .or_else(|| usage.get("cacheCreationInputTokens"))
            .or_else(|| usage.get("cache_create_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let cache_read = usage.get("cache_read_input_tokens")
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

/// Extract model name from JSON
fn extract_model(json: &serde_json::Value) -> Option<String> {
    // Try message.model first
    json.get("message")
        .and_then(|m| m.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            json.get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// Extract timestamp from JSON
fn extract_timestamp(json: &serde_json::Value) -> Option<i64> {
    // Try various timestamp fields
    let ts = json.get("timestamp")
        .or_else(|| json.get("createdAt"))
        .or_else(|| json.get("created_at"))
        .or_else(|| json.get("time"));

    if let Some(ts) = ts {
        // Try as number (could be seconds or milliseconds)
        if let Some(num) = ts.as_u64() {
            // If > 10 billion, assume milliseconds
            return Some(if num > 10_000_000_000 { (num / 1000) as i64 } else { num as i64 });
        }
        // Try as string (ISO format)
        if let Some(s) = ts.as_str() {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                return Some(dt.timestamp());
            }
        }
    }

    None
}

/// Truncate a string to max length, adding ellipsis if truncated
fn truncate_string(s: &str, max_len: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_len {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

/// Get session metadata by session ID
pub fn get_session_meta_by_id(session_id: &str) -> Option<SessionMeta> {
    let files = scan_session_files();

    // Try exact match first
    for file in &files {
        if file.session_id == session_id {
            return Some(extract_session_meta(file));
        }
    }

    // Try matching by the last part of session_id (after ::)
    let id_suffix = session_id.split("::").last().unwrap_or(session_id);
    for file in &files {
        let file_suffix = file.session_id.split("::").last().unwrap_or(&file.session_id);
        if file_suffix == id_suffix {
            return Some(extract_session_meta(file));
        }
    }

    None
}

/// Get all session metadata (limited)
pub fn get_all_session_meta(limit: usize) -> Vec<SessionMeta> {
    scan_session_files()
        .into_iter()
        .take(limit)
        .map(|f| extract_session_meta(&f))
        .collect()
}

/// Get all session metadata without limit (for pagination)
pub fn get_all_session_meta_raw() -> Vec<SessionMeta> {
    scan_session_files()
        .into_iter()
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
        // Should skip system messages
        assert!(is_system_message("<local-command-caveat>some text"));
        assert!(is_system_message("prefix <local-command-caveat> content"));
        assert!(is_system_message("<command-name>/model</command-name>"));
        assert!(is_system_message("<local-command-stdout>Set model to...</local-command-stdout>"));
        assert!(is_system_message("<system-reminder>Some reminder</system-reminder>"));
        assert!(is_system_message("ab")); // too short

        // Should NOT skip real user messages
        assert!(!is_system_message("请帮我分析这段代码"));
        assert!(!is_system_message("How do I fix this bug?"));
        assert!(!is_system_message("分析一下项目结构"));
    }

    #[test]
    fn test_extract_project_name() {
        assert_eq!(extract_project_name("/Users/test/projects/my-app"), Some("my-app".to_string()));
        assert_eq!(extract_project_name("/home/user/code/UsageMeter"), Some("UsageMeter".to_string()));
        assert_eq!(extract_project_name("C:\\Users\\test\\project"), Some("project".to_string()));
        assert_eq!(extract_project_name(""), None);
        assert_eq!(extract_project_name("/"), None);
    }
}
