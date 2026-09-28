// Windows 发布版不弹控制台窗口；调试版保留，方便看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    learning_hub_lib::run()
}
