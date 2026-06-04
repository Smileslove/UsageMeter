use serde_json::Value;

pub(in crate::session) fn extract_project_name(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }

    let normalized = cwd.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    parts.last().map(|value| value.to_string())
}

pub(in crate::session) fn extract_u64_by_keys(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(parse_u64_from_value))
        .unwrap_or(0)
}

pub(in crate::session) fn parse_u64_from_value(value: &Value) -> Option<u64> {
    if let Some(num) = value.as_u64() {
        return Some(num);
    }
    if let Some(num) = value.as_i64() {
        return Some(num.max(0) as u64);
    }
    if let Some(num) = value.as_f64() {
        return Some(num.max(0.0) as u64);
    }
    None
}

pub(in crate::session) fn extract_model(json: &Value) -> Option<String> {
    let model = json
        .get("message")
        .and_then(|message| message.get("model"))
        .and_then(|value| value.as_str())
        .or_else(|| json.get("model").and_then(|value| value.as_str()));

    let model = model?;
    if model.is_empty() || model == "unknown" {
        return None;
    }
    if model.starts_with('<') && model.ends_with('>') {
        return None;
    }
    Some(model.to_string())
}

pub(in crate::session) fn extract_timestamp(json: &Value) -> Option<i64> {
    let ts = json
        .get("timestamp")
        .or_else(|| json.get("createdAt"))
        .or_else(|| json.get("created_at"))
        .or_else(|| json.get("time"))
        .or_else(|| json.get("date"));

    let ts = ts?;
    if let Some(num) = ts.as_u64() {
        return Some(if num > 10_000_000_000 {
            (num / 1000) as i64
        } else {
            num as i64
        });
    }
    if let Some(text) = ts.as_str() {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
            return Some(dt.timestamp());
        }
    }
    None
}

pub(in crate::session) fn truncate_string(value: &str, max_len: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_len {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_string_keeps_or_truncates_text() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 5), "hello...");
    }

    #[test]
    fn extract_project_name_supports_posix_and_windows_paths() {
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
