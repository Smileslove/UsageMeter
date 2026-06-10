//! UsageMeter - Claude Code 用量监控器
//!
//! 一款用于实时监控 Claude Code 使用情况的系统托盘应用。

mod commands;
mod local_usage;
mod models;
mod net;
mod proxy;
mod session;
mod subscription;
mod sync;
mod unified_usage;
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
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::default(),
            None,
        ));

    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .manage(commands::ProxyState::default())
        .manage(commands::UpdaterState::default())
        .manage(subscription::SubscriptionState::new())
        .on_window_event(|window, event| match event {
            WindowEvent::Focused(false) => {
                if window.label() == "main" {
                    let _ = window.hide();
                }
            }
            WindowEvent::CloseRequested { api, .. } => {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            // HTTP 客户端工厂必须最先初始化：所有后续后台任务（local_usage 同步、
            // WebDAV 同步、订阅查询等）都依赖 HttpClientFactory::global()。
            // 同时缓存 settings 供下方 locale 复用，避免重复 load。
            let initial_settings = commands::load_settings().ok();
            net::HttpClientFactory::init(
                initial_settings
                    .as_ref()
                    .map(|s| s.network_proxy.clone())
                    .unwrap_or_default(),
            );

            {
                tauri::async_runtime::spawn(async move {
                    let _ = tauri::async_runtime::spawn_blocking(|| {
                        if let Err(err) = crate::local_usage::ensure_local_usage_synced() {
                            eprintln!("[UsageMeter] Failed to prewarm local usage database: {err}");
                        }
                    })
                    .await;
                });
            }

            {
                tauri::async_runtime::spawn(async move {
                    if let Some(proxy_db) = crate::proxy::ProxyDatabase::get_global() {
                        if let Err(err) = proxy_db.backfill_unlocked_costs().await {
                            eprintln!("[UsageMeter] Failed to prewarm unlocked proxy costs: {err}");
                        }
                    }
                });
            }

            crate::sync::spawn_background_sync_loop();

            {
                let proxy_state = commands::ProxyState {
                    server: app.state::<commands::ProxyState>().server.clone(),
                    passive_monitor_handle: app
                        .state::<commands::ProxyState>()
                        .passive_monitor_handle
                        .clone(),
                    passive_monitor_shutdown: app
                        .state::<commands::ProxyState>()
                        .passive_monitor_shutdown
                        .clone(),
                };
                tauri::async_runtime::spawn(async move {
                    commands::ensure_passive_proxy_monitor_started(&proxy_state).await;
                });
            }

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

            let locale = initial_settings
                .map(|s| s.locale)
                .unwrap_or_else(models::default_locale);
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

                                // macOS: 托盘在屏幕顶部，窗口向下弹出
                                // Windows: 托盘在屏幕底部，窗口向上弹出，底部贴着工作区底部
                                #[cfg(target_os = "macos")]
                                let y = position.y + 10.0;
                                #[cfg(not(target_os = "macos"))]
                                let y = {
                                    const WORKAREA_MARGIN: f64 = 2.0; // 窗口底部与工作区底部的间距（像素）
                                    let popup_height =
                                        size.map(|s| s.height as f64).unwrap_or(600.0);

                                    let workarea_bottom = app
                                        .available_monitors()
                                        .ok()
                                        .and_then(|monitors| {
                                            monitors.into_iter().find_map(|m| {
                                                let monitor_pos = m.position();
                                                let monitor_size = m.size();
                                                let tray_x = position.x as i32;
                                                let tray_y = position.y as i32;

                                                // 检查托盘图标是否在这个显示器的整体范围内
                                                let in_monitor = tray_x >= monitor_pos.x
                                                    && tray_x < monitor_pos.x + monitor_size.width as i32
                                                    && tray_y >= monitor_pos.y
                                                    && tray_y < monitor_pos.y + monitor_size.height as i32;

                                                if in_monitor {
                                                    let work_area = m.work_area();
                                                    let bottom = work_area.position.y
                                                        + work_area.size.height as i32;
                                                    Some(bottom as f64)
                                                } else {
                                                    None
                                                }
                                            })
                                        })
                                        .unwrap_or_else(|| {
                                            eprintln!(
                                                "[UsageMeter] Warning: No monitor found for tray at ({}, {})",
                                                position.x, position.y
                                            );
                                            position.y
                                        });

                                    workarea_bottom - popup_height - WORKAREA_MARGIN
                                };

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

            // 启动后延迟 10 秒静默检查更新，避免与其他初始化任务竞争资源
            #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    let settings = match commands::load_settings() {
                        Ok(settings) => settings,
                        Err(e) => {
                            eprintln!("[UsageMeter] Failed to load settings for update check: {e}");
                            return;
                        }
                    };
                    if !settings.auto_check_update {
                        return;
                    }
                    let updater = match commands::build_updater(&app_handle) {
                        Ok(u) => u,
                        Err(e) => {
                            eprintln!("[UsageMeter] Updater build failed: {e}");
                            return;
                        }
                    };

                    match updater.check().await {
                        Ok(Some(update)) => {
                            if commands::should_suppress_update(
                                &update.version,
                                &settings.skipped_update_version,
                            ) {
                                return;
                            }
                            let dto = commands::build_dto(&update);
                            // 将 Update 对象存入 UpdaterState 供用户触发安装时使用
                            if let Some(state) = app_handle.try_state::<commands::UpdaterState>() {
                                *state.pending_update.lock().unwrap() = Some(update);
                            }
                            let _ = app_handle.emit("update-available", dto);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            eprintln!("[UsageMeter] Background update check failed: {e}");
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 设置命令
            commands::load_settings,
            commands::save_settings,
            commands::list_wsl_distros,
            // 用量命令
            commands::refresh_usage_bundle,
            commands::get_overview_breakdown,
            commands::get_window_rate_summary,
            commands::get_statistics_summary,
            commands::get_month_activity,
            commands::get_year_activity,
            // 会话命令
            commands::get_sessions,
            commands::get_session_detail,
            commands::get_project_stats,
            commands::get_recent_request_records,
            // 本地缓存维护命令
            commands::get_local_usage_maintenance_stats,
            commands::purge_orphan_local_facts,
            commands::rebuild_local_usage_cache,
            commands::get_opencode_schema_status,
            // 代理命令
            commands::start_proxy,
            commands::stop_proxy,
            commands::stop_proxy_runtime_only,
            commands::get_proxy_status,
            commands::is_proxy_running,
            commands::get_proxy_usage,
            commands::set_takeover_for_app,
            commands::get_takeover_statuses,
            commands::resolve_takeover_conflict,
            // 模型价格命令
            commands::sync_model_pricing_from_api,
            commands::search_model_pricing,
            commands::get_custom_model_pricings,
            commands::count_synced_model_pricings,
            commands::clear_synced_model_pricings,
            commands::add_custom_model_pricing,
            commands::update_custom_model_pricing,
            commands::delete_model_pricing,
            commands::get_all_model_pricings,
            commands::preview_pricing_apply,
            commands::apply_pricing_to_records,
            // 开机自启动命令
            commands::enable_autostart,
            commands::disable_autostart,
            commands::is_autostart_enabled,
            // 来源管理命令
            commands::rename_api_source,
            commands::delete_api_source,
            commands::merge_api_source,
            commands::add_key_prefix_to_source,
            commands::update_api_source_key_note,
            commands::set_active_source_filter,
            commands::get_api_sources,
            // 货币命令
            commands::get_exchange_rates,
            // 网络代理命令
            commands::test_network_proxy,
            // WebDAV 同步命令
            commands::test_webdav_connection,
            commands::sync_now,
            commands::rotate_sync_password,
            commands::get_sync_status,
            commands::list_sync_devices,
            commands::remove_sync_device,
            commands::clear_imported_sync_data,
            commands::get_active_sync_device_id,
            // 退出命令
            commands::prepare_exit,
            commands::confirm_exit,
            // 订阅查询命令
            commands::get_subscription_quota,
            commands::refresh_subscription_quota,
            commands::has_chatgpt_oauth,
            commands::has_claude_oauth,
            commands::get_configured_source_quotas,
            commands::clear_subscription_cache,
            // 更新命令
            commands::check_for_update,
            commands::download_and_install_update,
            commands::skip_update_version,
            // 窗口命令
            commands::open_share_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
