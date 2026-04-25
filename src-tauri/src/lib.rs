//! UsageMeter - Claude Code 用量监控器
//!
//! 一款用于实时监控 Claude Code 使用情况的系统托盘应用。

mod commands;
mod models;
mod proxy;
mod session;
mod utils;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tauri::{PhysicalPosition, Position, WindowEvent};

#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;

fn menu_labels(locale: &str) -> (&'static str, &'static str) {
    if locale == "en-US" {
        ("Open Panel", "Quit")
    } else {
        ("打开面板", "退出")
    }
}

#[cfg(target_os = "macos")]
fn make_window_rounded(window: &tauri::WebviewWindow) {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_app_kit::{NSColor, NSWindow, NSWindowButton, NSWindowStyleMask};

    let ns_window = window.ns_window().unwrap() as *mut AnyClass;
    unsafe {
        let window: &NSWindow = &*ns_window.cast();

        // 设置无边框样式
        window.setStyleMask(NSWindowStyleMask::FullSizeContentView);

        // 隐藏标题栏
        window.setTitlebarAppearsTransparent(true);

        // 隐藏关闭、最小化、缩放按钮
        if let Some(close_button) = window.standardWindowButton(NSWindowButton::CloseButton) {
            close_button.setHidden(true);
        }
        if let Some(min_button) = window.standardWindowButton(NSWindowButton::MiniaturizeButton) {
            min_button.setHidden(true);
        }
        if let Some(zoom_button) = window.standardWindowButton(NSWindowButton::ZoomButton) {
            zoom_button.setHidden(true);
        }

        // 设置透明背景
        let clear_color = NSColor::clearColor();
        window.setBackgroundColor(Some(&clear_color));

        // 启用内容视图圆角
        if let Some(content_view) = window.contentView() {
            content_view.setWantsLayer(true);
            if let Some(layer) = content_view.layer() {
                let layer_ptr = objc2::rc::Retained::as_ptr(&layer);
                let _: () = msg_send![layer_ptr, setCornerRadius: 24.0f64];
                layer.setMasksToBounds(true);
            }
        }

        // 窗口设置
        window.setOpaque(false);
        window.setMovableByWindowBackground(false);

        // 设置动画行为
        let _: () = msg_send![window, setAnimationBehavior: 0];
    }
    let _ = window.set_shadow(true);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::default(),
            None,
        ))
        .manage(commands::ProxyState::default())
        .on_window_event(|window, event| match event {
            WindowEvent::Focused(false) => {
                let _ = window.hide();
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            // 启动时检测并恢复孤立的代理状态
            // 如果上次应用异常崩溃，可能存在备份文件残留或配置未恢复的情况
            {
                use proxy::ClaudeConfigManager;
                let config_manager = ClaudeConfigManager::new();
                if let Some(message) = config_manager.check_and_recover_orphaned_state() {
                    eprintln!("[UsageMeter] Startup recovery: {}", message);
                }
            }

            // 启动时执行 session_stats 表数据迁移（一次性）
            // 将现有 usage_records 中的数据聚合到 session_stats 表
            {
                let db_path = dirs::home_dir()
                    .map(|h| h.join(".usagemeter").join("proxy_data.db"));

                if let Some(path) = db_path {
                    if path.exists() {
                        match proxy::ProxyDatabase::new_with_path(&path) {
                            Ok(db) => {
                                let db_clone = std::sync::Arc::new(db);
                                tauri::async_runtime::spawn(async move {
                                    match db_clone.migrate_to_session_stats().await {
                                        Ok(count) => {
                                            if count > 0 {
                                                eprintln!("[UsageMeter] Migrated {} sessions to session_stats table", count);
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!("[UsageMeter] Failed to migrate session stats: {}", e);
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                eprintln!("[UsageMeter] Failed to open proxy DB for migration: {}", e);
                            }
                        }
                    }
                }
            }

            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                make_window_rounded(&window);
            }

            let locale = commands::load_settings()
                .map(|s| s.locale)
                .unwrap_or_else(|_| models::default_locale());
            let (show_label, quit_label) = menu_labels(&locale);

            let show_item = MenuItem::with_id(app, "show", show_label, true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", quit_label, true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .icon(
                    app.default_window_icon()
                        .ok_or("ERR_MISSING_DEFAULT_APP_ICON")?
                        .clone(),
                )
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            #[cfg(target_os = "macos")]
                            {
                                make_window_rounded(&window);
                            }
                            let _ = window.set_always_on_top(true);
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        // 发送事件给前端，让前端处理清理后再退出
                        let _ = app.emit("app-quit-requested", ());
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let visible = window.is_visible().unwrap_or(false);
                            if visible {
                                let _ = window.hide();
                            } else {
                                #[cfg(target_os = "macos")]
                                {
                                    make_window_rounded(&window);
                                }
                                let _ = window.set_always_on_top(true);
                                let size = window.outer_size().ok();
                                let popup_width = size.map(|s| s.width as f64).unwrap_or(420.0);
                                let x = position.x - (popup_width / 2.0);
                                let y = position.y + 10.0;
                                let _ = window.set_position(Position::Physical(
                                    PhysicalPosition::new(x.round() as i32, y.round() as i32),
                                ));
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 设置命令
            commands::load_settings,
            commands::save_settings,
            // 用量命令
            commands::get_usage_snapshot,
            commands::get_window_rate_summary,
            // 会话命令
            commands::get_sessions,
            commands::get_session_detail,
            commands::get_project_stats,
            // 代理命令
            commands::start_proxy,
            commands::stop_proxy,
            commands::get_proxy_status,
            commands::is_proxy_running,
            commands::get_proxy_usage,
            // 模型价格命令
            commands::sync_model_pricing_from_api,
            commands::search_model_pricing,
            commands::add_custom_model_pricing,
            commands::update_custom_model_pricing,
            commands::delete_model_pricing,
            commands::get_all_model_pricings,
            // 开机自启动命令
            commands::enable_autostart,
            commands::disable_autostart,
            commands::is_autostart_enabled,
            // 退出命令
            commands::prepare_exit,
            commands::confirm_exit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
