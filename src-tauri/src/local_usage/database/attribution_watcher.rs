use super::LocalUsageDatabase;
use crate::proxy::{ClaudeConfigManager, CodexConfigManager, GeminiConfigManager};
use crate::subscription::GeminiSubscriptionProvider;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::{mpsc, Once};
use std::time::Duration;

static PASSIVE_ATTRIBUTION_WATCHER_STARTED: Once = Once::new();

fn watched_config_paths() -> Vec<PathBuf> {
    let codex = CodexConfigManager::new();
    let claude = ClaudeConfigManager::new();
    let gemini = GeminiConfigManager::new();
    vec![
        codex.config_path().clone(),
        codex.auth_path().clone(),
        claude.settings_path().clone(),
        gemini.config_path().clone(),
        GeminiSubscriptionProvider::oauth_credentials_path(),
    ]
}

fn event_touches_config(event_paths: &[PathBuf], config_paths: &[PathBuf]) -> bool {
    event_paths.iter().any(|event_path| {
        config_paths
            .iter()
            .any(|config_path| config_path == event_path)
    })
}

fn observe_config_change() {
    let Ok(settings) = crate::settings::load_settings_blocking() else {
        return;
    };
    let Ok(database) = LocalUsageDatabase::get_global() else {
        return;
    };
    if database
        .observe_passive_attribution(&settings, chrono::Utc::now().timestamp_millis())
        .unwrap_or(false)
    {
        crate::unified_usage::clear_runtime_caches();
    }
}

fn run_watcher(config_paths: Vec<PathBuf>) -> Result<(), String> {
    let mut watched_dirs = Vec::new();
    for config_path in &config_paths {
        let Some(parent) = config_path.parent() else {
            continue;
        };
        if parent.exists() && !watched_dirs.iter().any(|path| path == parent) {
            watched_dirs.push(parent.to_path_buf());
        }
    }
    if watched_dirs.is_empty() {
        return Ok(());
    }

    let (sender, receiver) = mpsc::channel();
    let callback_paths = config_paths.clone();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            if let Ok(event) = result {
                if event_touches_config(&event.paths, &callback_paths) {
                    let _ = sender.send(());
                }
            }
        })
        .map_err(|error| format!("Failed to create passive attribution watcher: {error}"))?;
    for directory in watched_dirs {
        watcher
            .watch(&directory, RecursiveMode::NonRecursive)
            .map_err(|error| format!("Failed to watch passive attribution config: {error}"))?;
    }

    while receiver.recv().is_ok() {
        while receiver.recv_timeout(Duration::from_millis(250)).is_ok() {}
        observe_config_change();
    }
    Ok(())
}

/// Starts one native filesystem watcher for direct Codex, Claude, and Gemini configuration files.
/// Scanner-based observation remains as a fallback for tools whose config directories do not
/// exist yet at startup.
pub fn start_passive_attribution_watcher() {
    PASSIVE_ATTRIBUTION_WATCHER_STARTED.call_once(|| {
        let config_paths = watched_config_paths();
        if let Err(error) = std::thread::Builder::new()
            .name("usagemeter-attribution-watch".to_string())
            .spawn(move || {
                if let Err(error) = run_watcher(config_paths) {
                    eprintln!("[UsageMeter] Passive attribution watcher stopped: {error}");
                }
            })
        {
            eprintln!("[UsageMeter] Failed to start passive attribution watcher: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_ignores_unrelated_file_events() {
        let config = PathBuf::from("/tmp/config.toml");
        assert!(event_touches_config(
            &[config.clone()],
            std::slice::from_ref(&config)
        ));
        assert!(!event_touches_config(
            &[PathBuf::from("/tmp/other.toml")],
            &[config]
        ));
    }
}
