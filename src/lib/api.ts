// Typed wrappers around the Rust commands (src-tauri/src/commands.rs).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Security = "tls" | "starttls";

export interface AccountConfig {
  email: string;
  display_name: string;
  username: string;
  imap_host: string;
  imap_port: number;
  imap_security: Security;
  smtp_host: string;
  smtp_port: number;
  smtp_security: Security;
  extra_ca_pem: string | null;
}
export interface Account extends AccountConfig { id: number }
export interface Folder { id: number; account_id: number; name: string; role: string | null; unread: number; total: number }
export interface MessageSummary {
  id: number; folder_id: number; uid: number; subject: string; from_name: string; from_addr: string;
  date: number; preview: string; seen: boolean; flagged: boolean; has_attachments: boolean;
}
export interface Attachment { index: number; name: string; mime: string; size: number; risky: boolean }
export interface MessageDetail extends MessageSummary {
  to: string; cc: string; text: string; html: string | null; remote_images_blocked: number;
  remote_images_loaded: number; attachments: Attachment[];
}
export interface MessageView extends MessageDetail {
  document: string | null; remote_images_available: number; remote_images_failed: number; sender_trusted: boolean;
}
export type AttachmentRef = { kind: "file"; token: string } | { kind: "message"; message_id: number; index: number };
export interface ForwardedAttachment { message_id: number; index: number; name: string; mime: string; size: number }
export interface ComposeDraft {
  to: string[]; cc: string[]; bcc: string[]; subject: string; body: string;
  in_reply_to: string | null; references: string[]; attachments: ForwardedAttachment[];
}
export type ComposeMode = "reply" | "reply_all" | "forward" | "draft";
export interface PickedAttachment { token: string; name: string; size: number; mime: string }
export interface SendResult { sent_copy: "saved" | "no_sent_folder" | "failed"; sent_copy_error: string | null; draft_cleanup_error: string | null }
export interface LinkInfo {
  original: string; clean: string; host: string; scheme: string; note: string | null;
  mailto: { to: string[]; cc: string[]; bcc: string[]; subject: string; body: string } | null;
}
export interface ContactSuggestion { name: string; email: string; source: "contacts" | "history" }
export interface ContactsStatus { eds_connected: boolean; books: number; error: string | null }
export interface SearchHit extends MessageSummary { snippet: string }
export interface SyncReport { folders: number; new_messages: number; removed_messages: number; failed_folders: string[] }
export interface Outgoing {
  to: string[]; cc: string[]; bcc: string[]; subject: string; body: string;
  in_reply_to: string | null; references: string[]; attachments: AttachmentRef[];
}
export interface AppInfo {
  secret_backend: "keyring" | "memory";
  cache: { mode: "encrypted" | "memory"; notice: string | null };
  version: string;
}
export interface SyncEvent { account_id: number; report?: SyncReport; error?: string }

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  listAccounts: () => invoke<Account[]>("list_accounts"),
  addAccount: (config: AccountConfig, password: string) => invoke<Account>("add_account", { config, password }),
  removeAccount: (accountId: number) => invoke<void>("remove_account", { accountId }),
  setPassword: (accountId: number, password: string) => invoke<void>("set_password", { accountId, password }),
  syncAccount: (accountId: number) => invoke<SyncReport>("sync_account", { accountId }),
  listFolders: (accountId: number) => invoke<Folder[]>("list_folders", { accountId }),
  listMessages: (folderId: number, offset = 0, limit = 200) =>
    invoke<MessageSummary[]>("list_messages", { folderId, offset, limit }),
  getMessage: (messageId: number, dark: boolean, loadRemote = false) =>
    invoke<MessageView>("get_message", { messageId, dark, loadRemote }),
  setImageSenderTrust: (accountId: number, email: string, trusted: boolean) =>
    invoke<void>("set_image_sender_trust", { accountId, email, trusted }),
  messageSource: (messageId: number) => invoke<string>("message_source", { messageId }),
  search: (accountId: number, query: string) => invoke<SearchHit[]>("search", { accountId, query }),
  composePrefill: (messageId: number, mode: ComposeMode) => invoke<ComposeDraft>("compose_prefill", { messageId, mode }),
  send: (accountId: number, message: Outgoing, replaceDraft: number | null = null) =>
    invoke<SendResult>("send_message", { accountId, message, replaceDraft }),
  saveDraft: (accountId: number, message: Outgoing, replaceDraft: number | null = null) =>
    invoke<void>("save_draft", { accountId, message, replaceDraft }),
  analyzeLink: (url: string) => invoke<LinkInfo>("analyze_link", { url }),
  pickAttachments: () => invoke<{ files: PickedAttachment[]; rejected: string[] }>("pick_attachments"),
  saveAttachment: (messageId: number, index: number) => invoke<string | null>("save_attachment", { messageId, index }),
  openAttachment: (messageId: number, index: number, confirmed: boolean) =>
    invoke<{ status: "opened" | "needs_confirmation"; reason: string | null }>("open_attachment", { messageId, index, confirmed }),
  suggestContacts: (accountId: number, query: string) => invoke<ContactSuggestion[]>("suggest_contacts", { accountId, query }),
  contactsStatus: () => invoke<ContactsStatus>("contacts_status"),
  onSync: (cb: (e: SyncEvent) => void): Promise<UnlistenFn> => listen<SyncEvent>("mail://synced", (e) => cb(e.payload)),
  onLink: (cb: (l: LinkInfo | { error: string; original: string }) => void): Promise<UnlistenFn> =>
    listen<LinkInfo | { error: string; original: string }>("mail://link", (e) => cb(e.payload)),
  onHover: (cb: (url: string | null) => void): Promise<UnlistenFn> => listen<string | null>("mail://hover", (e) => cb(e.payload)),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

export type ComposerResult = { kind: "sent"; warning: string | null } | { kind: "draft" } | { kind: "discarded" };
export const emptyDraft = (): ComposeDraft => ({ to: [], cc: [], bcc: [], subject: "", body: "", in_reply_to: null, references: [], attachments: [] });
