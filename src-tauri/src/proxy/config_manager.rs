//! Claude Code 配置管理器
//!
//! 处理 Claude Code settings.json 的接管和恢复

use super::source_registry::ProxySourceRegistry;
use super::types::ClaudeSettings;
use std::fs;
use std::path::PathBuf;

/// Claude Code 配置管理器
pub struct ClaudeConfigManager {
    /// Claude settings.json 路径
    settings_path: PathBuf,
    /// 备份文件路径
    backup_path: PathBuf,
}

impl ClaudeConfigManager {
    /// 创建新的配置管理器
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let usagemeter_dir = home.join(".usagemeter");

        // 尝试查找 Claude settings.json
        let claude_dir = home.join(".claude");
        let settings_path = if claude_dir.join("settings.json").exists() {
            claude_dir.join("settings.json")
        } else {
            // 默认使用 settings.json（如需要将创建）
            claude_dir.join("settings.json")
        };

        let backup_path = usagemeter_dir.join("claude_settings_backup.json");

        Self {
            settings_path,
            backup_path,
        }
    }

    /// 获取 Claude settings.json 路径
    #[allow(dead_code)]
    pub fn settings_path(&self) -> &PathBuf {
        &self.settings_path
    }

    /// 检查接管是否活跃（设置指向代理）
    pub fn is_takeover_active(&self) -> bool {
        if let Ok(settings) = self.read_settings() {
            if let Some(base_url) = settings.get_base_url() {
                return Self::is_usagemeter_proxy_url(&base_url);
            }
        }
        false
    }

    pub fn is_usagemeter_proxy_url(base_url: &str) -> bool {
        let Ok(url) = reqwest::Url::parse(base_url) else {
            return false;
        };
        Self::is_local_claude_code_proxy_url(&url)
    }

    pub fn is_usagemeter_proxy_url_for_port(base_url: &str, proxy_port: u16) -> bool {
        let Ok(url) = reqwest::Url::parse(base_url) else {
            return false;
        };
        if !Self::is_local_claude_code_proxy_url(&url) {
            return false;
        }

        url.port() == Some(proxy_port)
    }

    fn is_local_claude_code_proxy_url(url: &reqwest::Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        if host != "127.0.0.1" && host != "localhost" {
            return false;
        }

        let path = url.path().trim_end_matches('/');
        path == "/claude-code" || path.starts_with("/claude-code/source/")
    }

    pub fn extract_source_id_from_proxy_url(base_url: &str) -> Option<String> {
        if !Self::is_usagemeter_proxy_url(base_url) {
            return None;
        }

        let marker = "/source/";
        let marker_index = base_url.find(marker)?;
        let rest = &base_url[(marker_index + marker.len())..];
        let source_id = rest
            .split('/')
            .next()
            .unwrap_or_default()
            .split('?')
            .next()
            .unwrap_or_default()
            .trim();

        if source_id.is_empty() {
            None
        } else {
            Some(source_id.to_string())
        }
    }

    /// 检查备份是否存在
    pub fn has_backup(&self) -> bool {
        self.backup_path.exists()
    }

    /// 读取当前 Claude 设置
    pub fn read_settings(&self) -> Result<ClaudeSettings, String> {
        if !self.settings_path.exists() {
            return Ok(ClaudeSettings::default());
        }

        let content = fs::read_to_string(&self.settings_path)
            .map_err(|e| format!("Failed to read Claude settings: {}", e))?;

        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse Claude settings: {}", e))
    }

    /// 写入 Claude 设置
    pub fn write_settings(&self, settings: &ClaudeSettings) -> Result<(), String> {
        // 确保父目录存在
        if let Some(parent) = self.settings_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create Claude config directory: {}", e))?;
        }

        let content = serde_json::to_string_pretty(settings)
            .map_err(|e| format!("Failed to serialize Claude settings: {}", e))?;

        // 原子写入：先写入临时文件，然后重命名
        let temp_path = self.settings_path.with_extension("json.tmp");
        fs::write(&temp_path, content)
            .map_err(|e| format!("Failed to write temp settings: {}", e))?;

        fs::rename(&temp_path, &self.settings_path)
            .map_err(|e| format!("Failed to rename settings file: {}", e))?;

        Ok(())
    }

    /// 接管 Claude 配置
    /// 备份原始设置并修改为指向代理
    #[allow(dead_code)]
    pub fn takeover(&self, proxy_port: u16) -> Result<(), String> {
        self.takeover_with_path_prefix(proxy_port, None)
    }

    /// 接管 Claude 配置，并可追加单端口代理的工具路径前缀。
    pub fn takeover_with_path_prefix(
        &self,
        proxy_port: u16,
        path_prefix: Option<&str>,
    ) -> Result<(), String> {
        self.takeover_with_path_prefix_and_source(proxy_port, path_prefix, None)
    }

    pub fn takeover_with_path_prefix_and_source(
        &self,
        proxy_port: u16,
        path_prefix: Option<&str>,
        source_id: Option<&str>,
    ) -> Result<(), String> {
        // 读取当前设置
        let mut settings = self.read_settings()?;

        // 如果备份不存在则创建备份
        if !self.has_backup() {
            let backup_content = serde_json::to_string_pretty(&settings)
                .map_err(|e| format!("Failed to serialize backup: {}", e))?;

            // 确保备份目录存在
            if let Some(parent) = self.backup_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create backup directory: {}", e))?;
            }

            fs::write(&self.backup_path, backup_content)
                .map_err(|e| format!("Failed to write backup: {}", e))?;
        }

        // 修改设置指向代理
        let proxy_url = match path_prefix
            .map(|p| p.trim().trim_matches('/'))
            .filter(|p| !p.is_empty())
        {
            Some(prefix) => match source_id.map(str::trim).filter(|id| !id.is_empty()) {
                Some(id) => format!("http://127.0.0.1:{}/{}/source/{}", proxy_port, prefix, id),
                None => format!("http://127.0.0.1:{}/{}", proxy_port, prefix),
            },
            None => format!("http://127.0.0.1:{}", proxy_port),
        };
        settings.set_base_url(&proxy_url);

        // 写入修改后的设置
        self.write_settings(&settings)?;

        Ok(())
    }

    pub fn clear_backup(&self) -> Result<(), String> {
        if self.has_backup() {
            fs::remove_file(&self.backup_path)
                .map_err(|e| format!("Failed to remove backup: {}", e))?;
        }
        Ok(())
    }

    pub fn restore_from_active_source_handle(&self) -> Result<bool, String> {
        let settings = self.read_settings()?;
        let Some(base_url) = settings.get_base_url() else {
            return Ok(false);
        };
        let Some(source_id) = Self::extract_source_id_from_proxy_url(&base_url) else {
            return Ok(false);
        };
        let Some(handle) = ProxySourceRegistry::new().get(&source_id) else {
            return Ok(false);
        };

        self.write_settings(&handle.original_settings_snapshot)?;
        self.clear_backup()?;
        Ok(true)
    }

    /// 从备份恢复原始 Claude 配置
    pub fn restore(&self) -> Result<(), String> {
        if !self.has_backup() {
            // 没有备份，无需恢复
            return Ok(());
        }

        // 读取备份
        let backup_content = fs::read_to_string(&self.backup_path)
            .map_err(|e| format!("Failed to read backup: {}", e))?;

        let settings: ClaudeSettings = serde_json::from_str(&backup_content)
            .map_err(|e| format!("Failed to parse backup: {}", e))?;

        // 恢复设置
        self.write_settings(&settings)?;

        // 删除备份文件
        fs::remove_file(&self.backup_path)
            .map_err(|e| format!("Failed to remove backup: {}", e))?;

        Ok(())
    }

    /// 从 Claude 设置获取 API 密钥
    pub fn get_api_key(&self) -> Option<String> {
        self.read_settings().ok()?.get_api_key()
    }

    /// 获取原始基础 URL（如果备份存在则从备份获取，否则从当前设置获取）
    pub fn get_original_base_url(&self) -> Option<String> {
        if self.has_backup() {
            let backup_content = fs::read_to_string(&self.backup_path).ok()?;
            let settings: ClaudeSettings = serde_json::from_str(&backup_content).ok()?;
            settings
                .get_base_url()
                .or_else(|| Some("https://api.anthropic.com".to_string()))
        } else {
            self.read_settings()
                .ok()?
                .get_base_url()
                .or_else(|| Some("https://api.anthropic.com".to_string()))
        }
    }

    /// 检测 Claude Code 配置是否有问题（例如崩溃后残留）
    #[allow(dead_code)]
    pub fn detect_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // 检查孤立备份
        if self.has_backup() && !self.is_takeover_active() {
            issues.push("Backup exists but takeover is not active".to_string());
        }

        // 检查接管但没有备份（正常情况下不应发生）
        if self.is_takeover_active() && !self.has_backup() {
            issues.push("Takeover is active but no backup found".to_string());
        }

        issues
    }

    /// 检测并恢复孤立状态
    ///
    /// 当应用异常崩溃后，可能存在以下情况：
    /// 1. 备份存在但配置未被接管（孤立备份）→ 删除备份文件
    /// 2. 配置被接管但备份不存在（异常情况）→ 清除 ANTHROPIC_BASE_URL
    /// 3. 备份存在且配置被接管（崩溃残留）→ 从备份恢复原始配置
    ///
    /// 返回恢复操作的描述，如果没有需要恢复的则返回 None
    pub fn check_and_recover_orphaned_state(&self) -> Option<String> {
        let has_backup = self.has_backup();
        let is_takeover = self.is_takeover_active();

        match (has_backup, is_takeover) {
            // 情况1：孤立备份（备份存在但未接管）
            // 这可能是上次正常关闭但删除备份失败，或者用户手动修改了配置
            // 安全做法：删除孤立备份
            (true, false) => {
                if let Err(e) = fs::remove_file(&self.backup_path) {
                    return Some(format!("Failed to remove orphaned backup: {}", e));
                }
                Some("Removed orphaned backup file".to_string())
            }

            // 情况2：接管但没有备份（异常情况）
            // source-aware URL 可从 registry 恢复；legacy 接管才清除 BASE_URL。
            (false, true) => {
                match self.restore_from_active_source_handle() {
                    Ok(true) => {
                        return Some(
                            "Restored Claude config from source handle (no backup found)"
                                .to_string(),
                        );
                    }
                    Ok(false) => {}
                    Err(e) => return Some(e),
                }

                // 读取当前设置并清除 BASE_URL
                if let Ok(mut settings) = self.read_settings() {
                    settings.env.remove("ANTHROPIC_BASE_URL");
                    if let Err(e) = self.write_settings(&settings) {
                        return Some(format!("Failed to clear takeover state: {}", e));
                    }
                    return Some("Cleared orphaned takeover state (no backup found)".to_string());
                }
                Some("Failed to read settings while clearing orphaned takeover".to_string())
            }

            // 情况3：备份存在且接管活跃（崩溃残留）
            // source-aware URL 优先从 registry 恢复当前 handle；backup 只作 legacy fallback。
            (true, true) => {
                match self.restore_from_active_source_handle() {
                    Ok(true) => {
                        return Some(
                            "Restored Claude config from source handle (recovered from crash)"
                                .to_string(),
                        );
                    }
                    Ok(false) => {}
                    Err(e) => return Some(e),
                }

                if let Err(e) = self.restore() {
                    return Some(format!("Failed to restore from backup: {}", e));
                }
                Some("Restored Claude config from backup (recovered from crash)".to_string())
            }

            // 正常状态：无备份，未接管
            (false, false) => None,
        }
    }
}

impl Default for ClaudeConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_manager_creation() {
        let manager = ClaudeConfigManager::new();
        // 设置路径应该指向 .claude/settings.json
        assert!(manager
            .settings_path()
            .to_string_lossy()
            .contains(".claude"));
    }

    #[test]
    fn test_is_usagemeter_proxy_url_requires_claude_code_path() {
        assert!(ClaudeConfigManager::is_usagemeter_proxy_url(
            "http://127.0.0.1:18765/claude-code/source/src_abc"
        ));
        assert!(ClaudeConfigManager::is_usagemeter_proxy_url(
            "http://localhost:18765/claude-code"
        ));
        assert!(!ClaudeConfigManager::is_usagemeter_proxy_url(
            "http://127.0.0.1:4000/v1"
        ));
        assert!(!ClaudeConfigManager::is_usagemeter_proxy_url(
            "https://api.anthropic.com"
        ));
    }

    #[test]
    fn test_is_usagemeter_proxy_url_for_port_requires_matching_port() {
        assert!(ClaudeConfigManager::is_usagemeter_proxy_url_for_port(
            "http://127.0.0.1:18765/claude-code/source/src_abc",
            18765
        ));
        assert!(!ClaudeConfigManager::is_usagemeter_proxy_url_for_port(
            "http://127.0.0.1:4000/claude-code/source/src_abc",
            18765
        ));
        assert!(!ClaudeConfigManager::is_usagemeter_proxy_url_for_port(
            "http://127.0.0.1:18765/v1",
            18765
        ));
    }
}
