// 防止 Windows 发布版产生额外控制台窗口，请勿删除！！
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
fn configure_linux_window_backend() {
    // GTK/Wayland does not allow an application to position an xdg_toplevel.
    // Use XWayland when it is available so the tray panel can be anchored to
    // the system panel. Desktop sessions often export GDK_BACKEND=wayland;
    // allow an explicit UsageMeter override for users who need native Wayland.
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_some()
        && std::env::var_os("USAGEMETER_GDK_BACKEND").is_none()
    {
        let backend = std::env::var("GDK_BACKEND").unwrap_or_default();
        if backend.is_empty() || backend.eq_ignore_ascii_case("wayland") {
            std::env::set_var("GDK_BACKEND", "x11");
        }
    } else if let Some(backend) = std::env::var_os("USAGEMETER_GDK_BACKEND") {
        std::env::set_var("GDK_BACKEND", backend);
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    configure_linux_window_backend();

    usagemeter_lib::run()
}
