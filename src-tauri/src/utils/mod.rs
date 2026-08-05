//! 工具函数

pub mod business_time;

use std::path::{Path, PathBuf};

/// 获取用户主目录
#[allow(dead_code)]
pub fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "ERR_HOME_DIR_NOT_FOUND".to_string())
}

/// 获取 UsageMeter 配置目录
#[allow(dead_code)]
pub fn usagemeter_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".usagemeter"))
}

/// Deletes only a named UsageMeter-owned state file directly inside ~/.usagemeter.
pub fn remove_usagemeter_state_file(path: &Path, file_name: &str) -> Result<(), String> {
    let expected = usagemeter_dir()?.join(file_name);
    if path != expected {
        return Err("ERR_INVALID_USAGEMETER_STATE_FILE_PATH".to_string());
    }
    std::fs::remove_file(expected).map_err(|e| format!("ERR_REMOVE_USAGEMETER_STATE_FILE:{e}"))
}

/// 获取 Claude 配置目录
#[allow(dead_code)]
pub fn claude_config_dir() -> Result<PathBuf, String> {
    let home = home_dir()?;

    // 先尝试新位置
    let new_path = home.join(".config").join("claude");
    if new_path.exists() {
        return Ok(new_path);
    }

    // 回退到旧位置
    let old_path = home.join(".claude");
    if old_path.exists() {
        return Ok(old_path);
    }

    // 默认使用新位置
    Ok(new_path)
}

/// 获取 Claude settings.json 路径
#[allow(dead_code)]
pub fn claude_settings_path() -> Result<PathBuf, String> {
    let dir = claude_config_dir()?;
    let settings = dir.join("settings.json");
    if settings.exists() {
        return Ok(settings);
    }

    // 尝试旧版文件名
    let legacy = dir.join("claude.json");
    if legacy.exists() {
        return Ok(legacy);
    }

    // 默认使用 settings.json
    Ok(settings)
}
