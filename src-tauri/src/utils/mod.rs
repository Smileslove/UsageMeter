//! 工具函数

use std::path::PathBuf;

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
