//! WSL 被动扫描根解析。
//!
//! Windows 用户常在 WSL 内开发，其 Claude Code / Codex transcript 落在 WSL 的
//! `~/.claude/projects` / `~/.codex/sessions`。这些文件可经 UNC 路径
//! `\\wsl$\<distro>\home\<user>\...` 被 Windows 侧直接读取（格式与原生完全相同）。
//!
//! 本模块负责：枚举发行版 → 解析各发行版 `$HOME` → 拼出 UNC 扫描根，供
//! `claude_reader` / `codex_reader` / `opencode_reader` 的 `scan()` 追加扫描。
//! 核心逻辑仅在 Windows 编译；纯字符串辅助函数同时在 `test` 下编译以便跨平台单测。

// === 纯辅助函数（windows 实现与单测共享，故 cfg(any(windows, test))）===

/// 校验 WSL 发行版名（白名单，防止作为 `wsl.exe -d <distro>` 参数时被注入）。
#[cfg(any(windows, test))]
fn is_valid_distro_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 将 `wsl.exe` 的 UTF-16LE 输出解码为字符串（其 stdout 不是 UTF-8）。
#[cfg(any(windows, test))]
fn decode_utf16le_lossy(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// 由发行版名 + Linux `$HOME` + 尾段拼出 UNC 路径：
/// `\\wsl$\Ubuntu` + `/home/alice` + `[.claude, projects]`
/// → `\\wsl$\Ubuntu\home\alice\.claude\projects`
#[cfg(any(windows, test))]
fn unc_join(distro: &str, home: &str, tail: &[&str]) -> std::path::PathBuf {
    let mut s = format!(r"\\wsl$\{distro}");
    for seg in home.split('/').filter(|p| !p.is_empty()) {
        s.push('\\');
        s.push_str(seg);
    }
    for seg in tail {
        s.push('\\');
        s.push_str(seg);
    }
    std::path::PathBuf::from(s)
}

/// 在用户手动指定的 WSL 家目录根（UNC，如 `\\wsl$\Ubuntu\home\alice`）后追加尾段。
#[cfg(any(windows, test))]
fn append_tail(root: &str, tail: &[&str]) -> std::path::PathBuf {
    let mut s = root.trim_end_matches(['\\', '/']).to_string();
    for seg in tail {
        s.push('\\');
        s.push_str(seg);
    }
    std::path::PathBuf::from(s)
}

// === Windows 专属实现 ===

#[cfg(windows)]
mod platform {
    use super::{append_tail, decode_utf16le_lossy, is_valid_distro_name, unc_join};
    use crate::models::{AppSettings, WslScanSettings};
    use std::collections::HashMap;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::{Mutex, OnceLock};

    /// 不弹出控制台黑窗。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// 进程内 distro→home 缓存。解析 `$HOME` 会唤醒发行版，缓存到进程生命周期可把
    /// 唤醒压到每个发行版至多一次（发行版家目录在会话期间基本不变）。
    fn home_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
        static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
        CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// 读取磁盘上的设置；仅当 `wslScan.enabled` 为真时返回配置。默认（无文件/未开启）返回 None。
    pub fn scan_config_if_enabled() -> Option<WslScanSettings> {
        let path = AppSettings::settings_path().ok()?;
        let raw = std::fs::read_to_string(path).ok()?;
        let settings: AppSettings = serde_json::from_str(&raw).ok()?;
        settings.wsl_scan.enabled.then_some(settings.wsl_scan)
    }

    /// 所有 WSL 发行版下的 Claude `projects` 根。
    pub fn claude_projects_roots(cfg: &WslScanSettings) -> Vec<PathBuf> {
        roots_for(cfg, &[".claude", "projects"])
    }

    /// 所有 WSL 发行版下的 Codex `sessions` 根。
    pub fn codex_session_roots(cfg: &WslScanSettings) -> Vec<PathBuf> {
        roots_for(cfg, &[".codex", "sessions"])
    }

    /// 所有 WSL 发行版下的 OpenCode 数据根。
    pub fn opencode_home_roots(cfg: &WslScanSettings) -> Vec<PathBuf> {
        roots_for(cfg, &[".local", "share", "opencode"])
    }

    /// 所有 WSL 发行版下的 Gemini CLI `tmp` 会话根。
    pub fn gemini_tmp_roots(cfg: &WslScanSettings) -> Vec<PathBuf> {
        roots_for(cfg, &[".gemini", "tmp"])
    }

    fn roots_for(cfg: &WslScanSettings, tail: &[&str]) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = distro_homes(cfg)
            .into_iter()
            .map(|(distro, home)| unc_join(&distro, &home, tail))
            .collect();
        // 手动指定的 WSL 家目录根（UNC），作为自动探测失败时的兜底。
        for root in &cfg.extra_roots {
            if !root.trim().is_empty() {
                roots.push(append_tail(root, tail));
            }
        }
        roots
    }

    /// 返回 (distro, home) 列表。`cfg.distros` 为空则自动枚举。
    fn distro_homes(cfg: &WslScanSettings) -> Vec<(String, String)> {
        let distros: Vec<String> = if cfg.distros.is_empty() {
            list_distros()
        } else {
            cfg.distros
                .iter()
                .map(|d| d.trim().to_string())
                .filter(|d| is_valid_distro_name(d))
                .collect()
        };
        distros
            .into_iter()
            .filter_map(|d| home_for(&d).map(|home| (d, home)))
            .collect()
    }

    /// `wsl.exe -l -q` 枚举已注册发行版（输出为 UTF-16LE）。未装 WSL 时静默返回空。
    fn list_distros() -> Vec<String> {
        let output = match Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(out) if out.status.success() => out,
            _ => return Vec::new(),
        };
        decode_utf16le_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim().trim_end_matches('\r').to_string())
            .filter(|name| is_valid_distro_name(name))
            .collect()
    }

    /// 解析发行版 `$HOME`，带进程内缓存。优先问 WSL 自己，失败回退列 `home` 目录。
    fn home_for(distro: &str) -> Option<String> {
        if !is_valid_distro_name(distro) {
            return None;
        }
        if let Ok(cache) = home_cache().lock() {
            if let Some(cached) = cache.get(distro) {
                return cached.clone();
            }
        }
        let resolved = query_home(distro).or_else(|| guess_home_from_dir(distro));
        if let Ok(mut cache) = home_cache().lock() {
            cache.insert(distro.to_string(), resolved.clone());
        }
        resolved
    }

    /// `wsl.exe -d <distro> -- sh -c "echo $HOME"`。distro 名已校验，无注入风险。
    fn query_home(distro: &str) -> Option<String> {
        let output = Command::new("wsl.exe")
            .args(["-d", distro, "--", "sh", "-c", "echo $HOME"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let home = String::from_utf8_lossy(&output.stdout).trim().to_string();
        home.starts_with('/').then_some(home)
    }

    /// 兜底：列 `\\wsl$\<distro>\home\*`，取最近修改的子目录作为家目录。
    fn guess_home_from_dir(distro: &str) -> Option<String> {
        let home_root = PathBuf::from(format!(r"\\wsl$\{distro}\home"));
        let mut candidates: Vec<(i64, String)> = std::fs::read_dir(&home_root)
            .ok()?
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                let mtime = entry
                    .metadata()
                    .ok()?
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_secs() as i64;
                Some((mtime, format!("/home/{name}")))
            })
            .collect();
        candidates.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        candidates.into_iter().next().map(|(_, home)| home)
    }
}

#[cfg(windows)]
pub use platform::{
    claude_projects_roots, codex_session_roots, gemini_tmp_roots, opencode_home_roots,
    scan_config_if_enabled,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distro_name_whitelist() {
        assert!(is_valid_distro_name("Ubuntu"));
        assert!(is_valid_distro_name("Ubuntu-22.04"));
        assert!(is_valid_distro_name("openSUSE-Leap-15.2"));
        assert!(is_valid_distro_name("my_distro"));
        assert!(!is_valid_distro_name(""));
        assert!(!is_valid_distro_name("has space"));
        assert!(!is_valid_distro_name("evil;rm -rf"));
        assert!(!is_valid_distro_name(&"x".repeat(65)));
    }

    #[test]
    fn utf16le_decode() {
        // "Ubuntu\n" in UTF-16LE
        let bytes: Vec<u8> = "Ubuntu\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_utf16le_lossy(&bytes), "Ubuntu\n");
        // odd trailing byte must not panic
        let mut odd = bytes.clone();
        odd.push(0x00);
        assert!(decode_utf16le_lossy(&odd).starts_with("Ubuntu"));
    }

    #[test]
    fn unc_join_builds_path() {
        assert_eq!(
            unc_join("Ubuntu", "/home/alice", &[".claude", "projects"]),
            std::path::PathBuf::from(r"\\wsl$\Ubuntu\home\alice\.claude\projects"),
        );
        // extra slashes / nested home are normalized
        assert_eq!(
            unc_join("Debian", "/root", &[".codex", "sessions"]),
            std::path::PathBuf::from(r"\\wsl$\Debian\root\.codex\sessions"),
        );
    }

    #[test]
    fn append_tail_trims_trailing_separators() {
        assert_eq!(
            append_tail(r"\\wsl$\Ubuntu\home\alice\", &[".claude", "projects"]),
            std::path::PathBuf::from(r"\\wsl$\Ubuntu\home\alice\.claude\projects"),
        );
    }

    #[test]
    fn opencode_home_tail_builds_expected_path() {
        assert_eq!(
            append_tail(
                r"\\wsl$\Ubuntu\home\alice",
                &[".local", "share", "opencode"]
            ),
            std::path::PathBuf::from(r"\\wsl$\Ubuntu\home\alice\.local\share\opencode"),
        );
    }
}
