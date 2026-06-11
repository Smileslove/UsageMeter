//! 窗口相关 Tauri 命令

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

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
