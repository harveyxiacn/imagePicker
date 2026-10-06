const COMMANDS: &[&str] = &[
    "request_permission",
    "list_albums",
    "share",
    "trash",
    "start_foreground",
    "stop_foreground",
    "keep_awake",
    "publish_exports",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
