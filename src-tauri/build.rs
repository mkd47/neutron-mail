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
            "set_image_sender_trust",
            "message_source",
            "search",
            "compose_prefill",
            "send_message",
            "save_draft",
            "analyze_link",
            "pick_attachments",
            "save_attachment",
            "open_attachment",
            "suggest_contacts",
            "contacts_status",
        ]),
    ))
    .expect("failed to run tauri-build");
}
