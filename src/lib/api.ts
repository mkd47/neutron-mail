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
  auth?: AuthMethod;
}
export interface Account extends AccountConfig { id: number }
export type AuthMethod = "password" | "oauth_google" | "oauth_microsoft";
export interface Provider {
  id: string; name: string; domains: string[];
  imap_host: string; imap_port: number; imap_security: Security;
  smtp_host: string; smtp_port: number; smtp_security: Security;
  consumer_smtp_host: string | null;
  auth: AuthMethod[]; password_label: string; hint: string; help_url: string; source: string;
  oauth_ready: boolean;
}
export interface Discovered {
  imap_host: string; imap_port: number; imap_security: Security;
  smtp_host: string; smtp_port: number; smtp_security: Security;
  username: string; source: string; oauth2: boolean;
}
export interface Folder {
  id: number; account_id: number;
  /** Server (wire) name, e.g. "Projects.2026" or modified UTF-7. */
  name: string;
  /** Decoded full path for display (same delimiter). */
  display: string; delimiter: string | null; subscribed: boolean;
  role: string | null; unread: number; total: number;
}
export interface MessageSummary {
  id: number; folder_id: number; uid: number; subject: string; from_name: string; from_addr: string;
  date: number; preview: string; seen: boolean; flagged: boolean; has_attachments: boolean;
}
export interface ThreadSummary extends MessageSummary { thread_size: number; thread_unread: number; thread_flagged: boolean; thread_ids: number[] }
export interface ConversationItem extends MessageSummary { folder_name: string; folder_role: string | null }
export interface Attachment { index: number; name: string; mime: string; size: number; risky: boolean;
  /** An image already shown inside the message body (e.g. a signature logo). */
  inline: boolean }
export interface MailAddress { name: string; email: string }
export interface AddressInfo { contacts_available: boolean; in_contacts: boolean; trusted: boolean }
export interface MessageDetail extends MessageSummary {
  to: string; cc: string; to_list: MailAddress[]; cc_list: MailAddress[]; text: string; html: string | null; remote_images_blocked: number;
  remote_images_loaded: number; attachments: Attachment[];
}
export interface MessageView extends MessageDetail {
  document: string | null; remote_images_available: number; remote_images_failed: number; sender_trusted: boolean;
}
export type AttachmentRef = { kind: "file"; token: string } | { kind: "message"; message_id: number; index: number };
export interface ForwardedAttachment { message_id: number; index: number; name: string; mime: string; size: number }
export interface ComposeDraft {
  to: string[]; cc: string[]; bcc: string[]; subject: string; body: string;
  /** Initial HTML for the rich editor (sanitized by the backend), or null. */
  html: string | null;
  in_reply_to: string | null; references: string[]; attachments: ForwardedAttachment[];
  /** Alias to send from (preselected for replies to mail sent to it); null = the account's address. */
  from?: string | null;
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
export interface SyncReport { folders: number; new_messages: number; removed_messages: number; failed_folders: string[]; rules_matched: number; rule_errors: string[] }
export interface Outgoing {
  to: string[]; cc: string[]; bcc: string[]; subject: string; body: string;
  /** Rich body; sent as multipart/alternative. The backend sanitizes it again. */
  html_body: string | null;
  in_reply_to: string | null; references: string[]; attachments: AttachmentRef[];
  /** Sender address: the account's or one of its aliases (null = the account's). */
  from?: string | null;
}
export interface AppInfo {
  secret_backend: "keyring" | "memory";
  cache: { mode: "encrypted" | "memory"; notice: string | null };
  version: string;
}
export interface SyncEvent { account_id: number; report?: SyncReport; error?: string }
export interface SyncProgress { account_id: number; stage: string; folder: string | null; completed_folders: number; total_folders: number; downloaded: number; total_messages: number }

export interface AccountSettings {
  signature_html: string;
  /** 0 = all mail */
  sync_days: number;
  /** Extra sender addresses (aliases). */
  identities: MailAddress[];
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  listAccounts: () => invoke<Account[]>("list_accounts"),
  addAccount: (config: AccountConfig, password: string) => invoke<Account>("add_account", { config, password }),
  listProviders: () => invoke<Provider[]>("list_providers"),
  detectProvider: (email: string) => invoke<string | null>("detect_provider", { email }),
  autoconfigLookup: (email: string) => invoke<Discovered>("autoconfig_lookup", { email }),
  oauthSignIn: (config: AccountConfig, accountId: number | null) =>
    invoke<Account>("oauth_sign_in", { config, accountId }),
  removeAccount: (accountId: number) => invoke<void>("remove_account", { accountId }),
  updateAccount: (accountId: number, config: AccountConfig, password: string) =>
    invoke<Account>("update_account", { accountId, config, password }),
  getAccountSettings: (accountId: number) => invoke<AccountSettings>("get_account_settings", { accountId }),
  setAccountSettings: (accountId: number, settings: AccountSettings) =>
    invoke<AccountSettings>("set_account_settings", { accountId, settings }),
  newDraft: (accountId: number) => invoke<ComposeDraft>("new_draft", { accountId }),
  listImageSenders: (accountId: number) => invoke<string[]>("list_image_senders", { accountId }),
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
  saveAllAttachments: (messageId: number) => invoke<{ folder: string; saved: number } | null>("save_all_attachments", { messageId }),
  openAttachment: (messageId: number, index: number, confirmed: boolean) =>
    invoke<{ status: "opened" | "needs_confirmation"; reason: string | null }>("open_attachment", { messageId, index, confirmed }),
  suggestContacts: (accountId: number, query: string) => invoke<ContactSuggestion[]>("suggest_contacts", { accountId, query }),
  sanitizeComposeHtml: (html: string) => invoke<string>("sanitize_compose_html", { html }),
  setUnreadBadge: (count: number) => invoke<void>("set_unread_badge", { count }),
  contactsStatus: () => invoke<ContactsStatus>("contacts_status"),
  addressInfo: (accountId: number, email: string) => invoke<AddressInfo>("address_info", { accountId, email }),
  addContact: (name: string, email: string) => invoke<void>("add_contact", { name, email }),
  createFolder: (accountId: number, parentId: number | null, name: string) =>
    invoke<Folder>("create_folder", { accountId, parentId, name }),
  renameFolder: (folderId: number, name: string | null, newParentId: number | null, toRoot = false) =>
    invoke<Folder>("rename_folder", { folderId, name, newParentId, toRoot }),
  deleteFolder: (folderId: number, mode: DeleteMode) => invoke<number>("delete_folder", { folderId, mode }),
  setFolderSubscribed: (folderId: number, subscribed: boolean) =>
    invoke<void>("set_folder_subscribed", { folderId, subscribed }),
  moveMessages: (messageIds: number[], targetFolderId: number, copy: boolean) =>
    invoke<Transfer>("move_messages", { messageIds, targetFolderId, copy }),
  undoTransfer: (transfer: Transfer) => invoke<number>("undo_transfer", { transfer }),
  setMessageFlag: (messageIds: number[], flag: "seen" | "flagged", on: boolean) =>
    invoke<number>("set_message_flag", { messageIds, flag, on }),
  deleteMessages: (messageIds: number[]) => invoke<number>("delete_messages", { messageIds }),
  printMessage: (messageId: number) => invoke<void>("print_message", { messageId }),
  listThreads: (folderId: number, offset: number, limit: number) => invoke<ThreadSummary[]>("list_threads", { folderId, offset, limit }),
  conversation: (messageId: number) => invoke<ConversationItem[]>("conversation", { messageId }),
  onSync: (cb: (e: SyncEvent) => void): Promise<UnlistenFn> => listen<SyncEvent>("mail://synced", (e) => cb(e.payload)),
  onOpenMessage: (cb: (e: { account_id: number; message_id: number }) => void): Promise<UnlistenFn> =>
    listen<{ account_id: number; message_id: number }>("mail://open-message", (e) => cb(e.payload)),
  onSyncProgress: (cb: (e: SyncProgress) => void): Promise<UnlistenFn> => listen<SyncProgress>("mail://sync-progress", (e) => cb(e.payload)),
  onLink: (cb: (l: LinkInfo | { error: string; original: string }) => void): Promise<UnlistenFn> =>
    listen<LinkInfo | { error: string; original: string }>("mail://link", (e) => cb(e.payload)),
  onHover: (cb: (url: string | null) => void): Promise<UnlistenFn> => listen<string | null>("mail://hover", (e) => cb(e.payload)),
};

export interface CalendarEvent {
  uid: string; summary: string; location: string; start: number; end: number; all_day: boolean; calendar: string; color: string;
}
export interface Agenda { available: boolean; error: string | null; events: CalendarEvent[] }
export type PartStat = "ACCEPTED" | "TENTATIVE" | "DECLINED";
export interface Attendee { email: string; name: string; partstat: string }
export interface InviteEvent {
  uid: string; summary: string; location: string; description: string; start: number; end: number; all_day: boolean;
  organizer_email: string; organizer_name: string; attendees: Attendee[]; sequence: number; status: string; rrule: string | null;
}
export interface InviteView {
  method: string; event: InviteEvent; my_partstat: string; responded: boolean; in_calendar: boolean; can_respond: boolean;
}
export interface InviteResult { invite: InviteView; sent_copy: "saved" | "no_sent_folder" | "failed"; calendar_error: string | null }
export const calendarApi = {
  upcoming: (days: number) => invoke<Agenda>("upcoming_events", { days }),
  getInvite: (messageId: number) => invoke<InviteView | null>("get_invite", { messageId }),
  respond: (messageId: number, partstat: PartStat, addToCalendar: boolean) =>
    invoke<InviteResult>("respond_invite", { messageId, partstat, addToCalendar }),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

export type ComposerResult = { kind: "sent"; warning: string | null } | { kind: "draft" } | { kind: "discarded" };
export const emptyDraft = (): ComposeDraft => ({ to: [], cc: [], bcc: [], subject: "", body: "", html: null, in_reply_to: null, references: [], attachments: [], from: null });

export type DeleteMode = "move_to_trash" | "delete_messages";
export interface Transfer { account_id: number; target_folder_id: number; copy: boolean; items: [number, string][]; untracked: number; count: number }

export type RuleField = "from" | "to" | "cc" | "subject" | "body" | "header" | "size" | "has_attachment" | "account";
export type RuleOp = "contains" | "equals" | "regex" | "starts_with" | "greater" | "less" | "is" | "is_not";
export interface Condition { field: RuleField; op: RuleOp; value: string; header: string }
export type Action =
  | { type: "move"; folder: string } | { type: "copy"; folder: string } | { type: "mark_read" } | { type: "flag" }
  | { type: "label"; keyword: string } | { type: "delete" } | { type: "forward"; to: string } | { type: "stop" };
export interface Rule { id: number; name: string; enabled: boolean; match_all: boolean; conditions: Condition[]; actions: Action[]; server_side: boolean }
export interface Plan {
  rule_ids: number[]; mark_read: boolean; flag: boolean; keywords: string[]; copies: string[];
  move_to: string | null; delete: boolean; forwards: string[]; skipped: string[];
}
export interface RuleTest { matched: boolean; conditions: boolean[]; plan: Plan }
export interface RunReport { examined: number; matched: number; moved: number; copied: number; flagged: number; forwarded: number; skipped: string[]; errors: string[] }
export interface SieveExport { script: string; exported: number[]; local_only: [number, string][] }
export interface SieveStatus { settings: { port: number; active: boolean }; preview: SieveExport; oauth: boolean }
export const rulesApi = {
  list: () => invoke<Rule[]>("list_rules"),
  save: (rules: Rule[]) => invoke<Rule[]>("save_rules", { rules }),
  test: (rule: Rule, messageId: number) => invoke<RuleTest>("test_rule", { rule, messageId }),
  runNow: (folderId: number) => invoke<RunReport>("run_rules_now", { folderId }),
  sieveStatus: (accountId: number) => invoke<SieveStatus>("sieve_status", { accountId }),
  sieveUpload: (accountId: number, port: number) => invoke<SieveExport>("sieve_upload", { accountId, port }),
  sieveDisable: (accountId: number) => invoke<void>("sieve_disable", { accountId }),
};

// ---- Notes and To-do (local, encrypted cache) ----
export interface EmailLink { account_id: number | null; message_id: string; subject: string; sender: string; date: number }
export interface NoteFolder { id: number; name: string; count: number }
export interface Tag { name: string; color: string; count: number }
export interface NoteSummary { id: number; folder_id: number | null; title: string; snippet: string; modified: number; pinned: boolean; tags: string[]; linked: boolean }
export interface Note { id: number; uid: string; folder_id: number | null; title: string; html: string; created: number; modified: number; pinned: boolean; tags: string[]; links: EmailLink[] }
export interface NoteInput { id: number | null; folder_id: number | null; title: string; html: string; tags: string[]; pinned: boolean }
export interface NoteQuery { folder_id: number | null; tag: string | null; query: string }
export interface TaskList { id: number; name: string; color: string; open: number }
/** priority: iCalendar scale, 0 none, 1 high, 5 medium, 9 low. */
export interface Task {
  id: number; uid: string; list_id: number | null; parent_id: number | null; title: string; notes: string;
  due: number | null; due_time: boolean; reminder: number | null; priority: 0 | 1 | 5 | 9; completed: number | null;
  created: number; modified: number; email: EmailLink | null; subtasks: Task[];
}
export interface TaskInput {
  id: number | null; list_id: number | null; parent_id: number | null; title: string; notes: string;
  due: number | null; due_time: boolean; reminder: number | null; priority: number; email: EmailLink | null;
}
export type TaskView = { kind: "today"; day_end: number } | { kind: "upcoming"; day_end: number } | { kind: "from_email" } | { kind: "completed" } | { kind: "list"; id: number };
export interface TaskCounts { today: number; upcoming: number; from_email: number; completed: number }
export const taskInput = (t: Task): TaskInput => ({ id: t.id, list_id: t.list_id, parent_id: t.parent_id, title: t.title, notes: t.notes,
  due: t.due, due_time: t.due_time, reminder: t.reminder, priority: t.priority, email: null });
export const pimApi = {
  noteFolders: () => invoke<NoteFolder[]>("note_folders"),
  createNoteFolder: (name: string) => invoke<number>("create_note_folder", { name }),
  renameNoteFolder: (id: number, name: string) => invoke<void>("rename_note_folder", { id, name }),
  deleteNoteFolder: (id: number) => invoke<void>("delete_note_folder", { id }),
  tags: () => invoke<Tag[]>("note_tags"),
  notes: (query: NoteQuery) => invoke<NoteSummary[]>("list_notes", { query }),
  note: (id: number) => invoke<Note>("get_note", { id }),
  saveNote: (note: NoteInput) => invoke<Note>("save_note", { note }),
  deleteNote: (id: number) => invoke<void>("delete_note", { id }),
  noteFromMessage: (messageId: number, folderId: number | null) => invoke<Note>("note_from_message", { messageId, folderId }),
  unlinkNoteEmail: (noteId: number, messageId: string) => invoke<Note>("unlink_note_email", { noteId, messageId }),
  emailLink: (messageId: number) => invoke<EmailLink>("email_link", { messageId }),
  resolveEmail: (link: EmailLink) => invoke<number | null>("resolve_email_link", { link }),
  taskLists: () => invoke<TaskList[]>("task_lists"),
  createTaskList: (name: string, color: string) => invoke<number>("create_task_list", { name, color }),
  renameTaskList: (id: number, name: string) => invoke<void>("rename_task_list", { id, name }),
  deleteTaskList: (id: number) => invoke<void>("delete_task_list", { id }),
  tasks: (view: TaskView) => invoke<Task[]>("list_tasks", { view }),
  taskCounts: (dayEnd: number) => invoke<TaskCounts>("task_counts", { dayEnd }),
  saveTask: (task: TaskInput) => invoke<Task>("save_task", { task }),
  setTaskCompleted: (id: number, done: boolean) => invoke<Task>("set_task_completed", { id, done }),
  flaggedEmails: () => invoke<MessageSummary[]>("flagged_emails"),
  deleteTask: (id: number) => invoke<void>("delete_task", { id }),
  onReminder: (cb: (t: Task) => void): Promise<UnlistenFn> => listen<Task>("pim://reminder", (e) => cb(e.payload)),
};
/** Unix seconds at the end of the local day `offset` days from today. */
export function dayEnd(offset = 0): number {
  const d = new Date(); d.setHours(24, 0, 0, 0); d.setDate(d.getDate() + offset); return Math.floor(d.getTime() / 1000);
}
/** Unix seconds at local noon `offset` days from today (date-only due dates). */
export function dayAt(offset: number): number {
  const d = new Date(); d.setHours(12, 0, 0, 0); d.setDate(d.getDate() + offset); return Math.floor(d.getTime() / 1000);
}
export function dueLabel(due: number | null, withTime = false): string {
  if (due === null) return "";
  const d = new Date(due * 1000), t = new Date(); t.setHours(0, 0, 0, 0);
  const days = Math.round((new Date(d).setHours(0, 0, 0, 0) - t.getTime()) / 86400000);
  const time = withTime ? " " + d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : "";
  if (days === 0) return "Today" + time;
  if (days === 1) return "Tomorrow" + time;
  if (days === -1) return "Yesterday" + time;
  return d.toLocaleDateString([], { month: "short", day: "numeric" }) + time;
}
