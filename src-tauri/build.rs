// Declaring the app's commands in an app manifest makes Tauri generate an
// `allow-<command>` permission for each one; commands are then only callable
// when a capability grants them (least privilege).
fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "app_info",
            "list_accounts",
            "add_account",
            "remove_account",
            "set_password",
            "sync_account",
            "list_folders",
            "list_messages",
            "get_message",
            "search",
            "send_message",
        ]),
    ))
    .expect("failed to run tauri-build");
}
