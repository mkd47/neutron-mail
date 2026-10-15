// Declaring the app's commands in an app manifest makes Tauri generate an
// `allow-<command>` permission for each one; commands are then only callable
// when a capability grants them (least privilege).
fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "app_info",
            "list_accounts",
            "add_account",
            "list_providers",
            "detect_provider",
            "autoconfig_lookup",
            "oauth_sign_in",
            "oauth_configured",
            "remove_account",
            "set_password",
            "sync_account",
            "list_folders",
            "list_messages",
            "get_message",
            "set_image_sender_trust",
            "update_account",
            "get_account_settings",
            "set_account_settings",
            "list_image_senders",
            "new_draft",
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
            "sanitize_compose_html",
            "upcoming_events",
            "set_unread_badge",
            "get_invite",
            "respond_invite",
        ]),
    ))
    .expect("failed to run tauri-build");
}
