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
export interface Attachment { name: string; mime: string; size: number }
export interface MessageDetail extends MessageSummary {
  to: string; cc: string; text: string; html: string | null; remote_images_blocked: number; attachments: Attachment[];
}
export interface SearchHit extends MessageSummary { snippet: string }
export interface SyncReport { folders: number; new_messages: number; removed_messages: number; failed_folders: string[] }
export interface Outgoing { to: string[]; cc: string[]; subject: string; body: string }
export interface AppInfo { secret_backend: "keyring" | "memory"; version: string }
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
  getMessage: (messageId: number, dark: boolean) => invoke<MessageDetail & { document: string | null }>("get_message", { messageId, dark }),
  search: (accountId: number, query: string) => invoke<SearchHit[]>("search", { accountId, query }),
  send: (accountId: number, message: Outgoing) => invoke<void>("send_message", { accountId, message }),
  onSync: (cb: (e: SyncEvent) => void): Promise<UnlistenFn> => listen<SyncEvent>("mail://synced", (e) => cb(e.payload)),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
