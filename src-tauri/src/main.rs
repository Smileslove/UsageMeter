// 防止 Windows 发布版产生额外控制台窗口，请勿删除！！
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    usagemeter_lib::run()
}
