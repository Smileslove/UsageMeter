use crate::session::opencode_reader::{
    opencode_message_storage_root, OpenCodeFileCacheState, OpenCodeFileEntryState,
    OpenCodeMessageSnapshot,
};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(in crate::session) fn refresh_legacy_file_messages(
    state: &mut OpenCodeFileCacheState,
) -> std::collections::HashMap<String, OpenCodeMessageSnapshot> {
    let root = opencode_message_storage_root();
    if !root.exists() {
        state.files.clear();
        state.messages.clear();
        return state.messages.clone();
    }

    let files = collect_legacy_message_files(&root);
    let current_paths: HashSet<String> = files
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();

    let stale_paths: Vec<String> = state
        .files
        .keys()
        .filter(|path| !current_paths.contains(*path))
        .cloned()
        .collect();
    for stale_path in stale_paths {
        if let Some(entry) = state.files.remove(&stale_path) {
            if let Some(message_key) = entry.message_identity_key {
                state.messages.remove(&message_key);
            }
        }
    }

    for path in files {
        let path_string = path.to_string_lossy().to_string();
        let metadata = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let size = metadata.len();
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);

        let unchanged = state
            .files
            .get(&path_string)
            .map(|entry| entry.size == size && entry.mtime_ms == mtime_ms)
            .unwrap_or(false);
        if unchanged {
            continue;
        }

        let previous_identity = state
            .files
            .get(&path_string)
            .and_then(|entry| entry.message_identity_key.clone());
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let json = match serde_json::from_str::<Value>(&content) {
            Ok(json) => json,
            Err(_) => {
                state.files.insert(
                    path_string,
                    OpenCodeFileEntryState {
                        size,
                        mtime_ms,
                        message_identity_key: previous_identity,
                    },
                );
                continue;
            }
        };

        if let Some(ref identity) = previous_identity {
            state.messages.remove(identity);
        }

        let raw_session_id = json
            .get("sessionID")
            .or_else(|| json.get("sessionId"))
            .or_else(|| json.get("session_id"))
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let raw_message_id = json
            .get("id")
            .or_else(|| json.get("messageID"))
            .or_else(|| json.get("messageId"))
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        let snapshot = super::message::parse_message_snapshot(
            raw_session_id,
            raw_message_id,
            &json,
            0,
            "opencode_file",
        );

        state.files.insert(
            path_string,
            OpenCodeFileEntryState {
                size,
                mtime_ms,
                message_identity_key: snapshot.as_ref().map(|entry| entry.message_identity_key()),
            },
        );

        if let Some(snapshot) = snapshot {
            state
                .messages
                .insert(snapshot.message_identity_key(), snapshot);
        }
    }

    state.messages.clone()
}

fn collect_legacy_message_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                queue.push_back(path);
                continue;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("msg_") && name.ends_with(".json"))
                .unwrap_or(false)
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
