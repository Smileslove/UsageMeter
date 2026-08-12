//! 窗口相关 Tauri 命令

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;

/// 打开或聚焦分享编辑窗口。
#[tauri::command]
pub fn open_share_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("share") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return Ok(());
    }

    let builder =
        WebviewWindowBuilder::new(&app, "share", WebviewUrl::App("index.html#/share".into()))
            .title("UsageMeter Share")
            .inner_size(960.0, 700.0)
            .min_inner_size(840.0, 620.0)
            .resizable(true)
            .decorations(true)
            .transparent(true)
            .always_on_top(false)
            .skip_taskbar(false)
            .center();

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(TitleBarStyle::Overlay)
        .hidden_title(true);

    let window = builder
        .build()
        .map_err(|e| format!("ERR_OPEN_SHARE_WINDOW: {e}"))?;

    let _ = window.set_focus();
    Ok(())
}

// ============================================================================
// 桌面主窗口（label = "desktop"）
// ============================================================================

/// 桌面主窗口导航目标（深链载荷）。
///
/// 序列化/反序列化使用 camelCase（`sourceId`、`sessionKey` 等）；进入命令前先
/// 校验，不接受任意 URL 或超长字段，避免把不受控内容写进事件或路由。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNavigationTarget {
    /// 目标页面，必须在 [`DESKTOP_PAGE_WHITELIST`] 内。
    pub page: String,
    pub window: Option<String>,
    pub source_id: Option<String>,
    pub tool: Option<String>,
    pub session_key: Option<String>,
    pub metric: Option<String>,
    pub view: Option<String>,
}

/// 允许导航的桌面页面白名单。
const DESKTOP_PAGE_WHITELIST: [&str; 6] = [
    "overview",
    "analytics",
    "sessions",
    "activity",
    "gateway",
    "settings",
];

/// 导航目标可选字段的最大长度（字符数）。
const DESKTOP_TARGET_FIELD_MAX_LEN: usize = 200;

impl DesktopNavigationTarget {
    /// 校验导航目标合法性；非法返回 Err（调用方映射为 `ERR_INVALID_DESKTOP_TARGET`）。
    pub fn validate(&self) -> Result<(), String> {
        if !DESKTOP_PAGE_WHITELIST.contains(&self.page.as_str()) {
            return Err(format!("unsupported page {:?}", self.page));
        }
        for (name, value) in [
            ("window", self.window.as_deref()),
            ("sourceId", self.source_id.as_deref()),
            ("tool", self.tool.as_deref()),
            ("sessionKey", self.session_key.as_deref()),
            ("metric", self.metric.as_deref()),
            ("view", self.view.as_deref()),
        ] {
            if let Some(value) = value {
                // trim 后为空（空串/纯空白）视为非法，避免把无意义字段写进路由。
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(format!("field {name} must not be blank"));
                }
                if trimmed.chars().count() > DESKTOP_TARGET_FIELD_MAX_LEN {
                    return Err(format!(
                        "field {name} exceeds {DESKTOP_TARGET_FIELD_MAX_LEN} chars"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// 跨窗口共享的待处理桌面导航。
///
/// 新窗口首次加载完成前 Rust 侧暂存一次导航目标，前端 DesktopApp mount 后调用
/// `take_pending_desktop_navigation` 取走（取走即清空）。
#[derive(Default)]
pub struct PendingDesktopNavigation(Mutex<Option<DesktopNavigationTarget>>);

/// 打开或聚焦桌面主窗口（单例，label 固定为 "desktop"）。
///
/// - 已存在：show -> unminimize -> set_focus，并向窗口 emit `desktop-navigation`
///   （payload 为 target，可能为 None 表示仅聚焦）；WebView 可能仍在加载（监听
///   未注册）导致事件丢失，因此 target 为 Some 时同时写入 pending 通道；
/// - 不存在：按规格新建；若传入 target 则暂存为 pending，供前端 mount 后取走。
#[tauri::command]
pub fn open_desktop_window(
    app: tauri::AppHandle,
    target: Option<DesktopNavigationTarget>,
) -> Result<(), String> {
    if let Some(target) = &target {
        target
            .validate()
            .map_err(|e| format!("ERR_INVALID_DESKTOP_TARGET: {e}"))?;
    }

    if let Some(window) = app.get_webview_window("desktop") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        if let Some(target) = target {
            // WebView 可能仍在加载（前端监听未注册），仅 emit 会丢事件；与新建
            // 分支一致同时写入 pending 通道，前端两个通道都会拿到，属幂等。
            let _ = window.emit("desktop-navigation", &target);
            let pending = app.state::<PendingDesktopNavigation>();
            *pending.0.lock().unwrap_or_else(|err| err.into_inner()) = Some(target);
        }
        return Ok(());
    }

    let builder = WebviewWindowBuilder::new(
        &app,
        "desktop",
        WebviewUrl::App("index.html#/desktop".into()),
    )
    .title("UsageMeter")
    .inner_size(1180.0, 760.0)
    .min_inner_size(960.0, 640.0)
    .resizable(true)
    .decorations(true)
    .transparent(false)
    .always_on_top(false)
    .skip_taskbar(false);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(TitleBarStyle::Overlay)
        .hidden_title(true);

    // 恢复已保存的位置/尺寸/最大化；校验失败或文件损坏则忽略并首次居中。
    // 注意：不能同时调用 center() 与 position()（center 会覆盖 position）。
    let (builder, restore_maximized) = match restored_desktop_window_state(&app) {
        Some(restored) => (
            builder
                .position(restored.x, restored.y)
                .inner_size(restored.width, restored.height),
            restored.maximized,
        ),
        None => (builder.center(), false),
    };

    let window = builder
        .build()
        .map_err(|e| format!("ERR_OPEN_DESKTOP_WINDOW: {e}"))?;

    if restore_maximized {
        let _ = window.maximize();
    }
    let _ = window.set_focus();

    // 新窗口首次加载期间暂存导航目标，前端 DesktopApp mount 后取走。
    if let Some(target) = target {
        let pending = app.state::<PendingDesktopNavigation>();
        *pending.0.lock().unwrap_or_else(|err| err.into_inner()) = Some(target);
    }

    Ok(())
}

/// 取走暂存的桌面导航（取走即清空）；前端 DesktopApp mount 后调用一次。
#[tauri::command]
pub fn take_pending_desktop_navigation(app: tauri::AppHandle) -> Option<DesktopNavigationTarget> {
    app.state::<PendingDesktopNavigation>()
        .0
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .take()
}

// ============================================================================
// 桌面窗口状态持久化
//
// 位置/尺寸独立保存到 ~/.usagemeter/desktop_window_state.json，不写入
// AppSettings/settings.json/app_config.db，避免前端陈旧快照覆盖问题。
// ============================================================================

/// 桌面主窗口的持久化窗口状态（位置/尺寸为逻辑坐标）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
    pub updated_at: String,
}

/// 桌面窗口最小尺寸（逻辑像素），与窗口规格一致。
pub const DESKTOP_MIN_WIDTH: u32 = 960;
pub const DESKTOP_MIN_HEIGHT: u32 = 640;

/// 恢复校验：窗口至少要有 120x80 逻辑像素区域落在任一可用工作区。
const DESKTOP_MIN_VISIBLE_WIDTH: i32 = 120;
const DESKTOP_MIN_VISIBLE_HEIGHT: i32 = 80;

/// 桌面窗口状态文件路径。
fn desktop_window_state_path() -> Result<PathBuf, String> {
    Ok(crate::utils::usagemeter_dir()?.join("desktop_window_state.json"))
}

/// 从指定路径读取窗口状态；文件缺失、损坏或字段不全时返回 None。
fn load_desktop_window_state_from_path(path: &Path) -> Option<DesktopWindowState> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 将窗口状态写入指定路径。
fn save_desktop_window_state_to_path(
    path: &Path,
    state: &DesktopWindowState,
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("ERR_SERIALIZE_DESKTOP_WINDOW_STATE: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("ERR_SAVE_DESKTOP_WINDOW_STATE: {e}"))
}

/// 采集 desktop 窗口当前状态并写入状态文件（CloseRequested 时调用）。
///
/// 任何一步失败都静默忽略，不影响关闭流程。
pub fn persist_desktop_window_state(window: &tauri::Window) {
    let Some(path) = desktop_window_state_path().ok() else {
        return;
    };
    let Ok(position) = window.inner_position() else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    if scale <= 0.0 {
        return;
    }
    let state = DesktopWindowState {
        // inner_position 为物理坐标，inner_size 为物理尺寸；统一换算为逻辑坐标。
        x: (position.x as f64 / scale).round() as i32,
        y: (position.y as f64 / scale).round() as i32,
        width: (size.width as f64 / scale).round() as u32,
        height: (size.height as f64 / scale).round() as u32,
        maximized: window.is_maximized().unwrap_or(false),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(err) = save_desktop_window_state_to_path(&path, &state) {
        eprintln!("[UsageMeter] Failed to persist desktop window state: {err}");
    }
}

/// 恢复所需的窗口状态（逻辑坐标，f64 供 builder 直接使用）。
struct RestoredDesktopWindowState {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    maximized: bool,
}

/// 读取保存的窗口状态并校验是否可用于恢复；文件缺失/损坏/校验失败返回 None
/// （调用方忽略并居中）。
fn restored_desktop_window_state(app: &tauri::AppHandle) -> Option<RestoredDesktopWindowState> {
    let path = desktop_window_state_path().ok()?;
    let state = load_desktop_window_state_from_path(&path)?;

    // 把每个可用显示器的工作区换算为逻辑坐标后参与可见性校验。
    let monitors = app.available_monitors().ok()?;
    let work_areas: Vec<(i32, i32, u32, u32)> = monitors
        .iter()
        .map(|monitor| {
            let scale = monitor.scale_factor();
            if scale <= 0.0 {
                return (0, 0, 0, 0);
            }
            let area = monitor.work_area();
            (
                (area.position.x as f64 / scale).round() as i32,
                (area.position.y as f64 / scale).round() as i32,
                (area.size.width as f64 / scale).round() as u32,
                (area.size.height as f64 / scale).round() as u32,
            )
        })
        .collect();

    if !validate_desktop_window_state(&state, &work_areas) {
        return None;
    }
    Some(RestoredDesktopWindowState {
        x: state.x as f64,
        y: state.y as f64,
        width: state.width as f64,
        height: state.height as f64,
        maximized: state.maximized,
    })
}

/// 校验保存的窗口状态是否可用于恢复（纯逻辑，可测试）：
///
/// - 尺寸不小于桌面窗口最小尺寸（[`DESKTOP_MIN_WIDTH`] x [`DESKTOP_MIN_HEIGHT`]）；
/// - 至少 120x80 区域落在任一可用显示器工作区（工作区矩形为逻辑坐标，来自
///   `app.available_monitors()` 的 position/size）。
fn validate_desktop_window_state(
    state: &DesktopWindowState,
    work_areas: &[(i32, i32, u32, u32)],
) -> bool {
    if state.width < DESKTOP_MIN_WIDTH || state.height < DESKTOP_MIN_HEIGHT {
        return false;
    }
    let window_right = state.x.saturating_add(state.width as i32);
    let window_bottom = state.y.saturating_add(state.height as i32);
    work_areas.iter().any(|&(area_x, area_y, area_w, area_h)| {
        let area_right = area_x.saturating_add(area_w as i32);
        let area_bottom = area_y.saturating_add(area_h as i32);
        let overlap_width = window_right.min(area_right) - state.x.max(area_x);
        let overlap_height = window_bottom.min(area_bottom) - state.y.max(area_y);
        overlap_width >= DESKTOP_MIN_VISIBLE_WIDTH && overlap_height >= DESKTOP_MIN_VISIBLE_HEIGHT
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_state(x: i32, y: i32, width: u32, height: u32) -> DesktopWindowState {
        DesktopWindowState {
            x,
            y,
            width,
            height,
            maximized: false,
            updated_at: "2025-01-01T00:00:00Z".to_string(),
        }
    }

    /// 逻辑坐标工作区：主屏 1920x1080 + 右侧副屏 1920x1080。
    fn two_monitor_areas() -> Vec<(i32, i32, u32, u32)> {
        vec![(0, 0, 1920, 1080), (1920, 0, 1920, 1080)]
    }

    fn target(page: &str) -> DesktopNavigationTarget {
        DesktopNavigationTarget {
            page: page.to_string(),
            ..Default::default()
        }
    }

    // ---- 尺寸下限 ----

    #[test]
    fn rejects_size_below_minimum() {
        let areas = two_monitor_areas();
        assert!(!validate_desktop_window_state(
            &sample_state(0, 0, DESKTOP_MIN_WIDTH - 1, DESKTOP_MIN_HEIGHT),
            &areas
        ));
        assert!(!validate_desktop_window_state(
            &sample_state(0, 0, DESKTOP_MIN_WIDTH, DESKTOP_MIN_HEIGHT - 1),
            &areas
        ));
    }

    #[test]
    fn accepts_minimum_size_inside_work_area() {
        assert!(validate_desktop_window_state(
            &sample_state(0, 0, DESKTOP_MIN_WIDTH, DESKTOP_MIN_HEIGHT),
            &two_monitor_areas()
        ));
    }

    // ---- 工作区可见性 ----

    #[test]
    fn rejects_window_fully_offscreen() {
        assert!(!validate_desktop_window_state(
            &sample_state(5000, 5000, 1280, 800),
            &two_monitor_areas()
        ));
    }

    #[test]
    fn rejects_insufficient_visible_overlap() {
        // 只露出 60 逻辑像素宽（< 120）
        assert!(!validate_desktop_window_state(
            &sample_state(-900, 0, DESKTOP_MIN_WIDTH, DESKTOP_MIN_HEIGHT),
            &two_monitor_areas()
        ));
        // 只露出 60 逻辑像素高（< 80）
        assert!(!validate_desktop_window_state(
            &sample_state(0, -640, DESKTOP_MIN_WIDTH, 700),
            &two_monitor_areas()
        ));
    }

    #[test]
    fn accepts_window_on_secondary_monitor() {
        assert!(validate_desktop_window_state(
            &sample_state(1920, 0, 1280, 800),
            &two_monitor_areas()
        ));
    }

    #[test]
    fn accepts_window_straddling_two_monitors() {
        // 跨两屏，每屏可见区域都足够大（同时满足尺寸下限）
        assert!(validate_desktop_window_state(
            &sample_state(1800, 0, DESKTOP_MIN_WIDTH, DESKTOP_MIN_HEIGHT),
            &two_monitor_areas()
        ));
    }

    #[test]
    fn rejects_when_no_monitor_available() {
        assert!(!validate_desktop_window_state(
            &sample_state(0, 0, DESKTOP_MIN_WIDTH, DESKTOP_MIN_HEIGHT),
            &[]
        ));
    }

    // ---- 状态文件容错与读写 ----

    #[test]
    fn corrupted_json_is_ignored() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("desktop_window_state.json");
        std::fs::write(&path, "{ not valid json !!").unwrap();
        assert!(load_desktop_window_state_from_path(&path).is_none());
    }

    #[test]
    fn missing_fields_are_ignored() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("desktop_window_state.json");
        std::fs::write(&path, r#"{"x":10,"width":1280}"#).unwrap();
        assert!(load_desktop_window_state_from_path(&path).is_none());
    }

    #[test]
    fn extra_fields_are_tolerated() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("desktop_window_state.json");
        std::fs::write(
            &path,
            r#"{"x":10,"y":20,"width":1280,"height":800,"maximized":false,"updatedAt":"2025-01-01T00:00:00Z","futureField":1}"#,
        )
        .unwrap();
        assert!(load_desktop_window_state_from_path(&path).is_some());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("desktop_window_state.json");
        let state = sample_state(100, 50, 1280, 800);
        save_desktop_window_state_to_path(&path, &state).unwrap();
        assert_eq!(load_desktop_window_state_from_path(&path).unwrap(), state);
    }

    #[test]
    fn state_serializes_camel_case() {
        let state = sample_state(1, 2, 3, 4);
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"updatedAt\""));
        assert!(json.contains("\"maximized\""));
        assert!(!json.contains("updated_at"));
    }

    // ---- 导航目标校验 ----

    #[test]
    fn target_accepts_whitelisted_pages() {
        for page in [
            "overview",
            "analytics",
            "sessions",
            "activity",
            "gateway",
            "settings",
        ] {
            assert!(
                target(page).validate().is_ok(),
                "page {page} should be accepted"
            );
        }
    }

    #[test]
    fn target_rejects_unknown_or_empty_page() {
        assert!(target("").validate().is_err());
        assert!(target("Overview").validate().is_err());
        assert!(target("javascript:alert(1)").validate().is_err());
        assert!(target("https://evil.example/x").validate().is_err());
    }

    #[test]
    fn target_rejects_overlong_optional_fields() {
        let mut too_long = target("overview");
        too_long.session_key = Some("x".repeat(DESKTOP_TARGET_FIELD_MAX_LEN + 1));
        assert!(too_long.validate().is_err());

        // 多字节字符按字符数计
        let mut cjk = target("overview");
        cjk.view = Some("用".repeat(DESKTOP_TARGET_FIELD_MAX_LEN + 1));
        assert!(cjk.validate().is_err());
    }

    #[test]
    fn target_accepts_boundary_length_fields() {
        let mut boundary = target("overview");
        boundary.session_key = Some("x".repeat(DESKTOP_TARGET_FIELD_MAX_LEN));
        assert!(boundary.validate().is_ok());
    }

    #[test]
    fn target_rejects_blank_optional_fields() {
        // 空串与纯空白都必须被拒绝（trim 后为空视为非法）。
        for (field, value) in [
            ("window", Some("   ".to_string())),
            ("sourceId", Some("   ".to_string())),
            ("tool", Some("   ".to_string())),
            ("sessionKey", Some("   ".to_string())),
            ("metric", Some("   ".to_string())),
            ("view", Some("   ".to_string())),
            ("window", Some(String::new())),
            ("sourceId", Some(String::new())),
            ("tool", Some(String::new())),
            ("sessionKey", Some(String::new())),
            ("metric", Some(String::new())),
            ("view", Some(String::new())),
        ] {
            let mut t = target("overview");
            match field {
                "window" => t.window = value.clone(),
                "sourceId" => t.source_id = value.clone(),
                "tool" => t.tool = value.clone(),
                "sessionKey" => t.session_key = value.clone(),
                "metric" => t.metric = value.clone(),
                _ => t.view = value.clone(),
            };
            assert!(
                t.validate().is_err(),
                "blank/empty field {field} should be rejected"
            );
        }
    }

    #[test]
    fn target_serializes_camel_case() {
        let mut t = target("sessions");
        t.session_key = Some("abc".to_string());
        t.source_id = Some("src-1".to_string());
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"sessionKey\":\"abc\""));
        assert!(json.contains("\"sourceId\":\"src-1\""));
        assert!(json.contains("\"page\":\"sessions\""));
    }
}
