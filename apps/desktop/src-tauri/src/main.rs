// Prevents an extra console window on Windows in release; ignored elsewhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ip_desktop_lib::run();
}
