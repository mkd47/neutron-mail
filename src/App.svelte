<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { AppRail, Icon, type RailItem } from "@neutron/ui";
  import AccountSetup from "./lib/AccountSetup.svelte";
  import Settings from "./lib/Settings.svelte";
  import Compose from "./lib/Compose.svelte";
  import Viewer from "./lib/Viewer.svelte";
  import Agenda from "./lib/Agenda.svelte";
  import LinkDialog from "./lib/LinkDialog.svelte";
  import Rules from "./lib/Rules.svelte";
  import Notes from "./lib/Notes.svelte";
  import Todo from "./lib/Todo.svelte";
  import Calendar from "./lib/Calendar.svelte";
  import TodayPanel from "./lib/TodayPanel.svelte";
  import { api, emptyDraft, errorText, type Account, type AppInfo, type ComposeDraft, type ComposerResult, type Folder,
    type LinkInfo, type MessageSummary, type MessageView, type SearchHit, type Transfer, type DeleteMode, type EmailLink, type SyncProgress, pimApi } from "./lib/api";
  import { shortDate, snippetParts } from "./lib/format";

  let theme = $state<"dark" | "light">(
    (localStorage.getItem("theme") as "dark" | "light") ??
      (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark"));
  $effect(() => { document.documentElement.dataset.theme = theme; localStorage.setItem("theme", theme); });

  let info = $state<AppInfo | null>(null);
  let accounts = $state<Account[]>([]);
  let account = $state<Account | null>(null);
  let folders = $state<Folder[]>([]);
  let folder = $state<Folder | null>(null);
  let messages = $state<MessageSummary[]>([]);
  let selectedId = $state<number | null>(null);
  let detail = $state<MessageView | null>(null);
  let accountVersion = 0;
  let folderRequest = 0;
  let messageRequest = 0;
  let detailRequest = 0;
  let searchRequest = 0;
  let navigationRequest = 0;
  let loadRemote = $state(false);
  let link = $state<LinkInfo | null>(null);
  let hoverUrl = $state<string | null>(null);
  let toast = $state<{ text: string; warn: boolean; undo?: Transfer } | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function showToast(text: string, warn = false, undo?: Transfer) {
    toast = { text, warn, undo }; clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), warn || undo ? 12000 : 5000);
  }
  let query = $state("");
  let hits = $state<SearchHit[] | null>(null);
  let syncing = $state(false);
  let status = $state("");
  let showSetup = $state(false);
  let showSettings = $state(false);
  let showRules = $state(false);
  // App rail (shared @neutron/ui component): Mail, Calendar, Notes, To-do.
  const railItems: RailItem[] = [
    { id: "mail", icon: "mail", label: "Mail" }, { id: "calendar", icon: "calendar", label: "Calendar" },
    { id: "notes", icon: "note", label: "Notes" }, { id: "todo", icon: "checkcircle", label: "To-do" },
  ];
  type AppView = "mail" | "calendar" | "notes" | "todo";
  let appView = $state<AppView>((localStorage.getItem("appView") as AppView) ?? "mail");
  $effect(() => localStorage.setItem("appView", appView));
  let showPanel = $state(localStorage.getItem("todayPanel") !== "0");
  $effect(() => localStorage.setItem("todayPanel", showPanel ? "1" : "0"));
  let pimKey = $state(0);
  let openNoteId = $state<number | null>(null);
  async function newNoteFromPanel() {
    try { const n = await pimApi.saveNote({ id: null, folder_id: null, title: "Untitled note", html: "<p></p>", tags: [], pinned: false }); openNoteId = n.id; appView = "notes"; }
    catch (e) { showToast(errorText(e), true); }
  }
  async function openEmailLink(l: EmailLink) {
    try {
      const id = await pimApi.resolveEmail(l);
      if (id === null) { showToast(`“${l.subject}” isn't in the local cache (moved out of the sync window or deleted).`, true); return; }
      await openMessageById(id);
    } catch (e) { showToast(errorText(e), true); }
  }
  async function openMessageById(id: number) {
    const navigation = ++navigationRequest;
    detailRequest++;
    try {
      const d = await api.getMessage(id, theme === "dark", false);
      if (navigation !== navigationRequest) return;
      const candidates = account ? [account, ...accounts.filter((x) => x.id !== account?.id)] : accounts;
      for (const a of candidates) {
        const fs = a.id === account?.id ? folders : await api.listFolders(a.id);
        if (navigation !== navigationRequest) return;
        const f = fs.find((x) => x.id === d.folder_id);
        if (!f) continue;
        appView = "mail"; showSettings = false; showRules = false;
        if (a.id !== account?.id) { accountVersion++; folderRequest++; account = a; folders = fs; }
        searchRequest++; clearTimeout(searchTimer);
        folder = f; hits = null; query = ""; messages = []; picked = [id]; selectedId = id; detail = null;
        await refreshMessages();
        if (navigation !== navigationRequest) return;
        detail = d; loadRemote = false;
        await refreshFolders();
        return;
      }
      showToast("The message's folder wasn't found", true);
    } catch (e) { if (navigation === navigationRequest) showToast(errorText(e), true); }
  }
  // Multi-selection in the message list (Ctrl/Shift click).
  let picked = $state<number[]>([]);
  let anchor: number | null = null;
  let lastTransfer = $state<Transfer | null>(null);
  let draggingFolder = $state(false);
  let dropTarget = $state<number | "root" | null>(null);
  let showAll = $state(localStorage.getItem("showUnsubscribed") === "1");
  $effect(() => localStorage.setItem("showUnsubscribed", showAll ? "1" : "0"));
  let menu = $state<{ f: Folder; x: number; y: number } | null>(null);
  let ask = $state<{ title: string; label: string; value: string; ok: string; run: (v: string) => Promise<void> } | null>(null);
  let askError = $state("");
  let confirmDelete = $state<{ f: Folder; count: number; subs: number; mode: DeleteMode } | null>(null);
  let picker = $state<{ copy: boolean } | null>(null);
  let pickerQuery = $state("");

  async function newCompose() {
    if (!account) return;
    try { compose = await api.newDraft(account.id); }
    catch (e) { compose = emptyDraft(); showToast(`Couldn't load your signature: ${errorText(e)}`, true); }
  }
  async function accountRemoved(id: number) {
    showSettings = false;
    accounts = accounts.filter((a) => a.id !== id);
    showToast("Account removed");
    if (accounts[0]) await selectAccount(accounts[0]);
    else { accountVersion++; navigationRequest++; detailRequest++; searchRequest++; account = null; folders = []; messages = []; detail = null; selectedId = null; picked = []; showSetup = true; }
  }
  let compose = $state<ComposeDraft | null>(null);
  let agendaKey = $state(0);
  // Unread badge (window title + dock): unread mail in the inbox.
  const inboxUnread = $derived(folders.find((f) => f.role === "inbox")?.unread ?? 0);
  let lastBadge = -1;
  $effect(() => {
    const n = inboxUnread;
    if (n !== lastBadge) { lastBadge = n; api.setUnreadBadge(n).catch(() => {}); }
  });

  const roleIcon: Record<string, string> = { inbox: "inbox", sent: "sent", drafts: "drafts", trash: "trash", archive: "archive", junk: "junk", flagged: "star" };
  const roleOrder = ["inbox", "flagged", "drafts", "sent", "archive", "junk", "trash"];
  // Folder tree: parents are found by the IMAP hierarchy delimiter.
  const parentName = (f: Folder) => (f.delimiter && f.name.includes(f.delimiter) ? f.name.slice(0, f.name.lastIndexOf(f.delimiter)) : null);
  const leafName = (f: Folder) => (f.role === "inbox" ? "Inbox" : f.delimiter ? f.display.split(f.delimiter).pop()! : f.display);
  const isProtected = (f: Folder) => f.role !== null || f.name.toUpperCase() === "INBOX";
  const sortedFolders = $derived.by(() => {
    const byName = new Map(folders.map((f) => [f.name, f]));
    const kids = new Map<string | null, Folder[]>();
    for (const f of folders) {
      const p = parentName(f); const key = p !== null && byName.has(p) ? p : null;
      kids.set(key, [...(kids.get(key) ?? []), f]);
    }
    const order = (a: Folder, b: Folder) => {
      const ra = roleOrder.indexOf(a.role ?? ""), rb = roleOrder.indexOf(b.role ?? "");
      return (ra < 0 ? 99 : ra) - (rb < 0 ? 99 : rb) || leafName(a).localeCompare(leafName(b));
    };
    const out: { f: Folder; depth: number }[] = [];
    const walk = (key: string | null, depth: number) => {
      for (const f of (kids.get(key) ?? []).sort(order)) {
        if (showAll || f.subscribed || f.role) out.push({ f, depth });
        walk(f.name, depth + 1);
      }
    };
    walk(null, 0);
    return out;
  });
  const descendants = (f: Folder) => folders.filter((x) => f.delimiter && x.name.startsWith(f.name + f.delimiter));
  const list = $derived(hits ?? messages);

  async function refreshFolders() {
    if (!account) return;
    const id = account.id, version = accountVersion, request = ++folderRequest;
    const result = await api.listFolders(id);
    if (version !== accountVersion || account?.id !== id || request !== folderRequest) return;
    folders = result;
    if (folder) folder = folders.find((f) => f.id === folder!.id) ?? null;
    if (!folder) folder = folders.find((f) => f.role === "inbox") ?? folders[0] ?? null;
  }
  const PAGE = 200;
  let loadingMore = false;
  async function refreshMessages() {
    const request = ++messageRequest, version = accountVersion;
    if (!folder) { messages = []; return; }
    // Keep as many rows as were already loaded (paged by the backend's 500 cap).
    const want = Math.max(PAGE, messages.length), id = folder.id;
    const out: MessageSummary[] = [];
    while (out.length < want) {
      const n = Math.min(500, want - out.length);
      const page = await api.listMessages(id, out.length, n);
      if (version !== accountVersion || folder?.id !== id || request !== messageRequest) return;
      out.push(...page);
      if (page.length < n) break;
    }
    messages = out;
  }
  async function loadMore() {
    if (!folder || hits || loadingMore) return;
    const total = folders.find((f) => f.id === folder!.id)?.total ?? 0;
    if (messages.length >= total) return;
    loadingMore = true;
    const request = messageRequest, version = accountVersion, id = folder.id;
    try {
      const page = await api.listMessages(id, messages.length, PAGE);
      if (version === accountVersion && folder?.id === id && request === messageRequest) {
        const existing = new Set(messages.map((m) => m.id));
        messages = [...messages, ...page.filter((m) => !existing.has(m.id))];
      }
    } catch (e) { if (version === accountVersion && folder?.id === id) showToast(errorText(e), true); }
    finally { loadingMore = false; }
  }
  function onListScroll(e: Event) {
    const el = e.currentTarget as HTMLElement;
    if (el.scrollTop + el.clientHeight > el.scrollHeight - 600) void loadMore();
  }
  async function selectAccount(a: Account) {
    const version = ++accountVersion;
    navigationRequest++; detailRequest++; searchRequest++; messageRequest++;
    clearTimeout(searchTimer);
    account = a; folder = null; messages = []; detail = null; selectedId = null; hits = null; query = ""; picked = []; anchor = null;
    await refreshFolders();
    if (version !== accountVersion) return;
    await refreshMessages();
    if (version === accountVersion) void sync();
  }
  async function selectFolder(f: Folder) {
    navigationRequest++; detailRequest++; searchRequest++;
    clearTimeout(searchTimer);
    folder = f; hits = null; query = ""; detail = null; selectedId = null; messages = []; picked = [];
    await refreshMessages();
  }
  function clickItem(e: MouseEvent, m: MessageSummary) {
    if (e.ctrlKey || e.metaKey) {
      picked = picked.includes(m.id) ? picked.filter((x) => x !== m.id) : [...picked, m.id];
      anchor = m.id; return;
    }
    if (e.shiftKey && anchor !== null) {
      const ids = list.map((x) => x.id), a = ids.indexOf(anchor), b = ids.indexOf(m.id);
      if (a >= 0 && b >= 0) { picked = ids.slice(Math.min(a, b), Math.max(a, b) + 1); return; }
    }
    anchor = m.id; picked = [m.id];
    void open(m);
  }
  const focus = (el: HTMLElement) => { el.focus(); };
  const selection = () => (picked.length ? picked : selectedId !== null ? [selectedId] : []);

  // ---- moving / copying mail ----
  async function transfer(ids: number[], target: Folder, copy: boolean) {
    if (!ids.length) return;
    if (!copy && ids.every((id) => list.find((m) => m.id === id)?.folder_id === target.id)) return;
    try {
      status = `${copy ? "Copying" : "Moving"} ${ids.length}…`;
      const t = await api.moveMessages(ids, target.id, copy);
      lastTransfer = t;
      if (!copy) {
        messages = messages.filter((m) => !ids.includes(m.id));
        if (hits) hits = hits.filter((m) => !ids.includes(m.id));
        if (selectedId !== null && ids.includes(selectedId)) { detail = null; selectedId = null; }
        picked = [];
      }
      status = "";
      showToast(`${copy ? "Copied" : "Moved"} ${t.count} message${t.count === 1 ? "" : "s"} to ${leafName(target)}`, false, t);
      await refreshFolders();
    } catch (e) { status = ""; showToast(errorText(e), true); }
  }
  async function undo(t: Transfer | null = lastTransfer) {
    if (!t) return;
    toast = null; lastTransfer = null;
    try {
      const n = await api.undoTransfer(t);
      showToast(`Undone: ${n} message${n === 1 ? "" : "s"} ${t.copy ? "removed" : "moved back"}`);
      await refreshFolders(); await refreshMessages();
    } catch (e) { showToast(`Undo failed: ${errorText(e)}`, true); }
  }
  const MSG_TYPE = "application/x-neutron-messages", FOLDER_TYPE = "application/x-neutron-folder";
  function dragMessages(e: DragEvent, m: MessageSummary) {
    const ids = picked.includes(m.id) ? picked : [m.id];
    if (!picked.includes(m.id)) picked = [m.id];
    e.dataTransfer!.setData(MSG_TYPE, JSON.stringify(ids));
    e.dataTransfer!.effectAllowed = "copyMove";
    const ghost = document.createElement("div");
    ghost.className = "drag-ghost"; ghost.textContent = ids.length === 1 ? (m.subject || "(no subject)") : `${ids.length} messages`;
    document.body.appendChild(ghost); e.dataTransfer!.setDragImage(ghost, 10, 10);
    setTimeout(() => ghost.remove(), 0);
  }
  function dragFolder(e: DragEvent, f: Folder) {
    if (isProtected(f)) { e.preventDefault(); return; }
    e.dataTransfer!.setData(FOLDER_TYPE, String(f.id)); draggingFolder = true;
    e.dataTransfer!.effectAllowed = "move";
  }
  function canDrop(e: DragEvent, f: Folder | null) {
    const types = e.dataTransfer?.types ?? [];
    if (types.includes(MSG_TYPE)) return f !== null;
    return types.includes(FOLDER_TYPE);
  }
  function over(e: DragEvent, f: Folder | null) {
    if (!canDrop(e, f)) return;
    e.preventDefault();
    const isMsg = e.dataTransfer!.types.includes(MSG_TYPE);
    e.dataTransfer!.dropEffect = isMsg && (e.ctrlKey || e.metaKey) ? "copy" : "move";
    dropTarget = f ? f.id : "root";
  }
  async function drop(e: DragEvent, f: Folder | null) {
    e.preventDefault(); dropTarget = null; draggingFolder = false;
    const msgs = e.dataTransfer!.getData(MSG_TYPE);
    if (msgs && f) { await transfer(JSON.parse(msgs) as number[], f, e.ctrlKey || e.metaKey); return; }
    const fid = Number(e.dataTransfer!.getData(FOLDER_TYPE));
    const src = folders.find((x) => x.id === fid);
    if (!src || src.id === f?.id) return;
    if (f && (descendants(src).some((d) => d.id === f.id))) { showToast("A folder can't be moved into itself", true); return; }
    if ((f ? f.name : null) === parentName(src)) return;
    try {
      await api.renameFolder(src.id, null, f ? f.id : null, f === null);
      showToast(`Moved “${leafName(src)}” ${f ? `into ${leafName(f)}` : "to the top level"}`);
      await refreshFolders();
    } catch (err) { showToast(errorText(err), true); }
  }

  // ---- folder management ----
  function newFolder(parent: Folder | null) {
    menu = null; askError = "";
    ask = { title: parent ? `New folder in ${leafName(parent)}` : "New folder", label: "Name", value: "", ok: "Create",
      run: async (v) => { const f = await api.createFolder(account!.id, parent?.id ?? null, v); await refreshFolders(); showToast(`Created “${f.display}”`); } };
  }
  function renameFolder(f: Folder) {
    menu = null; askError = "";
    ask = { title: `Rename “${leafName(f)}”`, label: "New name", value: leafName(f), ok: "Rename",
      run: async (v) => { if (v !== leafName(f)) { await api.renameFolder(f.id, v, null); await refreshFolders(); } } };
  }
  async function submitAsk() {
    if (!ask || !ask.value.trim()) return;
    try { await ask.run(ask.value.trim()); ask = null; } catch (e) { askError = errorText(e); }
  }
  function askDelete(f: Folder) {
    menu = null;
    const subs = descendants(f);
    confirmDelete = { f, subs: subs.length, count: [f, ...subs].reduce((n, x) => n + x.total, 0), mode: "move_to_trash" };
  }
  async function doDelete() {
    const c = confirmDelete!; confirmDelete = null;
    try {
      const n = await api.deleteFolder(c.f.id, c.mode);
      if (folder && (folder.id === c.f.id || descendants(c.f).some((d) => d.id === folder!.id))) folder = null;
      await refreshFolders(); await refreshMessages();
      showToast(`Deleted “${leafName(c.f)}”${c.mode === "move_to_trash" && n ? `; ${n} message${n === 1 ? "" : "s"} moved to Trash` : ""}`);
    } catch (e) { showToast(errorText(e), true); }
  }
  async function toggleSubscribed(f: Folder) {
    menu = null;
    try { await api.setFolderSubscribed(f.id, !f.subscribed); await refreshFolders(); showToast(`${f.subscribed ? "Unsubscribed from" : "Subscribed to"} “${leafName(f)}”`); }
    catch (e) { showToast(errorText(e), true); }
  }
  const pickerFolders = $derived(sortedFolders.filter(({ f }) => f.id !== folder?.id && (!pickerQuery || f.display.toLowerCase().includes(pickerQuery.toLowerCase()))));
  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (t.closest("input, textarea, select, [contenteditable], .modal") || compose || showSettings || showRules) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z" && lastTransfer) { e.preventDefault(); void undo(); }
    else if (e.key.toLowerCase() === "v" && !e.ctrlKey && !e.metaKey && selection().length) { e.preventDefault(); pickerQuery = ""; picker = { copy: e.shiftKey }; }
    else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a" && t.closest(".list")) { e.preventDefault(); picked = list.map((m) => m.id); }
    else if (e.key === "Escape") { picked = []; menu = null; }
  }

  async function open(m: MessageSummary) {
    navigationRequest++;
    const request = ++detailRequest, version = accountVersion;
    selectedId = m.id; loadRemote = false; detail = null;
    try {
      const result = await api.getMessage(m.id, theme === "dark", false);
      if (version !== accountVersion || request !== detailRequest || selectedId !== m.id) return;
      detail = result;
      if (!m.seen) { m.seen = true; await refreshFolders(); }
    } catch (e) { if (version === accountVersion && request === detailRequest) status = errorText(e); }
  }
  const syncingAccounts = new Set<number>();
  async function onSyncProgress(e: SyncProgress) {
    if (e.account_id !== account?.id) return;
    if (e.stage === "connecting") status = "Connecting to mail server…";
    else if (e.stage === "listing") status = "Discovering folders…";
    else if (e.stage === "folders") status = `Found ${e.total_folders} folders · downloading mail…`;
    else status = `Syncing ${e.folder ?? "mail"} · folder ${e.completed_folders + 1}/${e.total_folders}${e.total_messages ? ` · ${e.downloaded}/${e.total_messages} messages` : ""}`;
    if (e.stage === "folders" || (e.stage === "messages" && e.downloaded > 0)) await refreshForProgress();
  }
  // Progress arrives once per downloaded page: run at most one refresh at a
  // time, queue at most one more, and space them a second apart, so a large
  // first sync doesn't reload the folder and message lists for every page.
  let progressRefreshing = false, progressQueued = false;
  async function refreshForProgress() {
    if (progressRefreshing) { progressQueued = true; return; }
    progressRefreshing = true;
    try {
      do {
        progressQueued = false;
        const version = accountVersion;
        await refreshFolders();
        if (version === accountVersion) await refreshMessages();
        if (progressQueued) await new Promise((r) => setTimeout(r, 1000));
      } while (progressQueued);
    } catch { /* the sync result reports errors */ }
    finally { progressRefreshing = false; }
  }
  async function sync() {
    if (!account || syncingAccounts.has(account.id)) return;
    const id = account.id, version = accountVersion;
    syncingAccounts.add(id); syncing = true; status = "Syncing…";
    try {
      const r = await api.syncAccount(id);
      if (version !== accountVersion || account?.id !== id) return;
      status = r.failed_folders.length ? `Synced with errors: ${r.failed_folders.join(", ")}` : `Up to date · ${r.new_messages} new${r.rules_matched ? ` · ${r.rules_matched} matched rules` : ""}`;
      if (r.rule_errors.length) showToast(`Rules: ${r.rule_errors.join("; ")}`, true);
      await refreshFolders();
      if (version === accountVersion) await refreshMessages();
    } catch (e) { if (version === accountVersion) status = errorText(e); }
    finally { syncingAccounts.delete(id); syncing = account !== null && syncingAccounts.has(account.id); }
  }
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  function onSearch() {
    clearTimeout(searchTimer);
    const request = ++searchRequest, version = accountVersion, id = account?.id, text = query;
    if (!id || !text.trim()) { hits = null; return; }
    searchTimer = setTimeout(async () => {
      try {
        const result = await api.search(id, text);
        if (version === accountVersion && request === searchRequest && query === text) hits = result;
      } catch (e) { if (version === accountVersion && request === searchRequest) status = errorText(e); }
    }, 200);
  }
  async function reload(remote: boolean, quiet = false) {
    if (selectedId === null) return;
    const id = selectedId, request = ++detailRequest, version = accountVersion;
    loadRemote = remote;
    try {
      const result = await api.getMessage(id, theme === "dark", remote);
      if (version !== accountVersion || request !== detailRequest || selectedId !== id) return;
      detail = result;
      if (!quiet && remote && result.remote_images_failed > 0)
        showToast(`${result.remote_images_failed} remote image(s) could not be loaded (blocked host, not an image, or too large).`, true);
    } catch (e) { if (!quiet && version === accountVersion && request === detailRequest) showToast(errorText(e), true); }
  }
  // Only theme changes trigger this effect; selection and image actions fetch separately.
  $effect(() => {
    void theme;
    untrack(() => { if (selectedId !== null) void reload(loadRemote, true); });
  });
  async function composerDone(r: ComposerResult) {
    if (r.kind === "sent") { showToast(r.warning ?? "Message sent", r.warning !== null); await sync(); }
    else if (r.kind === "draft") { showToast("Draft saved to Drafts"); await sync(); }
  }
  function onLinkEvent(l: LinkInfo | { error: string; original: string }) {
    if ("error" in l) { showToast(`Blocked link: ${l.error}`, true); return; }
    if (l.mailto) {
      compose = { ...emptyDraft(), to: l.mailto.to, cc: l.mailto.cc, bcc: l.mailto.bcc, subject: l.mailto.subject, body: l.mailto.body };
      return;
    }
    link = l;
  }

  onMount(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        info = await api.appInfo();
        if (info.cache.notice) showToast(info.cache.notice, info.cache.mode !== "encrypted" || info.cache.notice.includes("could not"));
        accounts = await api.listAccounts();
        const u1 = await api.onLink(onLinkEvent);
        const u2 = await api.onHover((u) => (hoverUrl = u));
        const u3 = await api.onSync(async (e) => {
          if (e.account_id !== account?.id) return;
          if (e.error) { status = `Background sync: ${e.error}`; return; }
          if (e.report?.rule_errors.length) showToast(`Rules: ${e.report.rule_errors.join("; ")}`, true);
          if (e.report) {
            status = e.report.failed_folders.length ? `Synced with errors: ${e.report.failed_folders.join(", ")}` : `Up to date · ${e.report.new_messages} new message${e.report.new_messages === 1 ? "" : "s"}${e.report.rules_matched ? ` · ${e.report.rules_matched} sorted by rules` : ""}`;
            await refreshFolders(); await refreshMessages();
          }
        });
        const u4 = await pimApi.onReminder((t) => { showToast(`⏰ Reminder: ${t.title}`); pimKey++; });
        const u5 = await api.onSyncProgress(onSyncProgress);
        unlisten = () => { u1(); u2(); u3(); u4(); u5(); };
        if (accounts[0]) await selectAccount(accounts[0]); else showSetup = true;
      } catch (e) { status = errorText(e); }
    })();
    return () => unlisten?.();
  });
</script>

<svelte:window onkeydown={onKey} onclick={() => (menu = null)} />
{#if showRules && account}
  <main class="setup-wrap">
    <Rules {account} {folders} testMessage={detail ? { id: detail.id, subject: detail.subject } : null}
      onClose={async () => { showRules = false; await refreshFolders(); await refreshMessages(); }} />
  </main>
{:else if showSettings && account}
  <main class="setup-wrap">
    <Settings {account} onClose={() => (showSettings = false)} onRemoved={accountRemoved}
      onUpdated={(a) => { accounts = accounts.map((x) => (x.id === a.id ? a : x)); account = a; }} />
  </main>
{:else if showSetup || !account}
  <main class="setup-wrap">
    <AccountSetup
      onAdded={async (a) => { accounts = [...accounts, a]; showSetup = false; await selectAccount(a); }}
      onCancel={account ? () => (showSetup = false) : undefined} />
    {#if info?.secret_backend === "memory"}
      <p class="warn">No system keyring found: passwords and the mail cache are kept in memory only for this session. Nothing is written to disk.</p>
    {/if}
  </main>
{:else}
  <div class="app">
  <AppRail items={railItems} active={appView} {theme}
    onSelect={(v) => { appView = v as AppView; if (v !== "notes") openNoteId = null; }}
    onToggleTheme={() => (theme = theme === "dark" ? "light" : "dark")} />
  {#if appView === "notes"}
    <Notes openId={openNoteId} onOpenEmail={openEmailLink} onStatus={(t, w) => showToast(t, w)} />
  {:else if appView === "todo"}
    <Todo refreshKey={pimKey} onOpenEmail={openEmailLink} onOpenMessage={openMessageById} onStatus={(t, w) => showToast(t, w)} />
  {:else if appView === "calendar"}
    <Calendar />
  {:else}
  <div class="shell" class:withpanel={showPanel}>
    <aside class="sidebar">
      <button class="btn primary compose" onclick={newCompose}>
        <Icon name="compose" /> Compose
      </button>
      <div class="navhead muted">
        <span>Folders</span>
        <button class="icon-btn sm" title={showAll ? "Hide unsubscribed folders" : "Show unsubscribed folders"} aria-pressed={showAll} onclick={() => (showAll = !showAll)}>{showAll ? "All" : "Subscribed"}</button>
        <button class="icon-btn sm" title="New folder" aria-label="New folder" onclick={() => newFolder(null)}><Icon name="plus" size={14} /></button>
      </div>
      <nav aria-label="Folders" class:rootdrop={dropTarget === "root"}
        ondragover={(e) => { if (e.target === e.currentTarget) over(e, null); }} ondragleave={(e) => { if (e.target === e.currentTarget) dropTarget = null; }}
        ondrop={(e) => { if (e.target === e.currentTarget) void drop(e, null); }}>
        {#each sortedFolders as { f, depth } (f.id)}
          <button class="folder" class:active={folder?.id === f.id} class:droptarget={dropTarget === f.id} class:unsub={!f.subscribed}
            style:padding-left="{12 + depth * 16}px" draggable={!isProtected(f)} title={f.display}
            onclick={() => selectFolder(f)} ondragstart={(e) => dragFolder(e, f)}
            ondragover={(e) => over(e, f)} ondragleave={() => (dropTarget = null)} ondrop={(e) => drop(e, f)} ondragend={() => { dropTarget = null; draggingFolder = false; }}
            oncontextmenu={(e) => { e.preventDefault(); e.stopPropagation(); menu = { f, x: e.clientX, y: e.clientY }; }}>
            <Icon name={roleIcon[f.role ?? ""] ?? "folder"} />
            <span class="fname">{leafName(f)}</span>
            {#if f.unread > 0}<span class="badge">{f.unread}</span>{/if}
          </button>
        {/each}
        <div class="droproot muted small" class:on={dropTarget === "root"} class:show={draggingFolder}
          ondragover={(e) => over(e, null)} ondragleave={() => (dropTarget = null)} ondrop={(e) => drop(e, null)} role="presentation">Drop here for top level</div>
      </nav>
      <div class="spacer"></div>
      <div class="accounts">
        {#each accounts as a (a.id)}
          <button class="acct" class:active={a.id === account.id} onclick={() => selectAccount(a)} title={a.email}>{a.email}</button>
        {/each}
        <button class="acct add" onclick={() => (showSetup = true)}><Icon name="plus" size={14} /> Add account</button>
      </div>
      <div class="foot">
        <button class="icon-btn" onclick={() => (showPanel = !showPanel)} aria-pressed={showPanel} aria-label="Today and notes panel" title="Today / Notes panel">
          <Icon name="panel" />
        </button>
        <button class="icon-btn" onclick={() => (showSettings = true)} aria-label="Account settings" title="Account settings">
          <Icon name="settings" />
        </button>
        <button class="icon-btn" onclick={() => (showRules = true)} aria-label="Rules" title="Rules">
          <Icon name="filter" />
        </button>
        <button class="icon-btn" onclick={sync} disabled={syncing} aria-label="Sync now" title="Sync now">
          <span class:spin={syncing}><Icon name="sync" /></span>
        </button>
        {#if info?.secret_backend === "memory"}<span class="muted small" title="No keyring: password kept in memory only">⚠ no keyring</span>{/if}
        {#if info?.cache.mode === "encrypted"}<span class="muted small cache-ok" title="Mail cache encrypted at rest (SQLCipher, key in the system keyring)"><Icon name="shield" size={13} /> Encrypted</span>
        {:else if info}<span class="muted small" title="Mail cache kept in memory only (no keyring)">⚠ cache in memory</span>{/if}
      </div>
    </aside>

    <section class="list">
      <div class="search">
        <Icon name="search" />
        <input placeholder="Search messages…" bind:value={query} oninput={onSearch} aria-label="Search messages" />
      </div>
      <div class="meta muted">
        {hits ? `${hits.length} result${hits.length === 1 ? "" : "s"}` : `${(folders.find((f) => f.id === folder?.id)?.total ?? messages.length).toLocaleString()} message${(folders.find((f) => f.id === folder?.id)?.total ?? messages.length) === 1 ? "" : "s"}`}
        <span class="status" title={status}>{status}</span>
      </div>
      {#if picked.length > 1 || (picked.length === 1 && selectedId === null)}
        <div class="selbar">
          <span>{picked.length} selected</span>
          <button class="btn sm" onclick={() => { pickerQuery = ""; picker = { copy: false }; }} title="Move to… (V)">Move to…</button>
          <button class="btn sm" onclick={() => { pickerQuery = ""; picker = { copy: true }; }} title="Copy to… (Shift+V)">Copy to…</button>
          <button class="icon-btn sm" aria-label="Clear selection" onclick={() => (picked = [])}><Icon name="close" size={13} /></button>
        </div>
      {/if}
      <ul onscroll={onListScroll}>
        {#each list as m (m.id)}
          <li>
            <button class="item" class:selected={selectedId === m.id || picked.includes(m.id)} class:unread={!m.seen}
              draggable="true" ondragstart={(e) => dragMessages(e, m)} ondragend={() => (dropTarget = null)}
              onclick={(e) => clickItem(e, m)} aria-pressed={picked.includes(m.id)}>
              <span class="dot"></span>
              <span class="row1"><span class="from">{m.from_name || m.from_addr}</span><span class="date">{#if m.flagged}<span class="star" title="Starred"><Icon name="star" size={13} /></span> {/if}{shortDate(m.date)}</span></span>
              <span class="subject">{m.subject || "(no subject)"}{#if m.has_attachments} <Icon name="clip" size={13} />{/if}</span>
              <span class="preview">
                {#if "snippet" in m}
                  {#each snippetParts((m as SearchHit).snippet) as p, i (i)}{#if p.hit}<mark>{p.text}</mark>{:else}{p.text}{/if}{/each}
                {:else}{m.preview}{/if}
              </span>
            </button>
          </li>
        {:else}
          <li class="empty muted">{hits ? "No matches" : "No messages"}</li>
        {/each}
      </ul>
    </section>

    <section class="reader">
      <Agenda refreshKey={agendaKey} />
      {#if detail}
        <div class="msgbar">
          <button class="btn sm" onclick={() => { pickerQuery = ""; picker = { copy: false }; }} title="Move to… (V)"><Icon name="folder" size={14} /> Move to…</button>
          <button class="btn sm" onclick={() => { pickerQuery = ""; picker = { copy: true }; }} title="Copy to… (Shift+V)">Copy to…</button>
        </div>
      {/if}
      <Viewer message={detail} {account} isDraft={folder?.role === "drafts"} {hoverUrl}
        onPimChanged={() => pimKey++} onOpenNote={() => {}}
        onReload={reload} onStatus={(t, w) => showToast(t, w)} onComposerDone={composerDone}
        onCalendarChanged={() => agendaKey++} />
    </section>
    {#if showPanel}
      <TodayPanel refreshKey={pimKey} onClose={() => (showPanel = false)} onOpenTodo={() => (appView = "todo")}
        onOpenNote={(id) => { openNoteId = id; appView = "notes"; }} onNewNote={newNoteFromPanel} onStatus={(t, w) => showToast(t, w)} />
    {/if}
  </div>
  {/if}
  </div>
  {#if compose}
    <Compose {account} draft={compose} onClose={(r) => { compose = null; void composerDone(r); }} />
  {/if}
  {#if link}
    <LinkDialog {link} onClose={(u) => { link = null; if (u) showToast(`Opened ${new URL(u).host} in your browser`); }} />
  {/if}
  {#if menu}
    {@const f = menu.f}
    <div class="ctx" style:left="{menu.x}px" style:top="{menu.y}px" role="menu">
      <button role="menuitem" onclick={() => newFolder(f)}>New subfolder…</button>
      <button role="menuitem" onclick={() => renameFolder(f)} disabled={isProtected(f)}>Rename…</button>
      {#if parentName(f) !== null && !isProtected(f)}
        <button role="menuitem" onclick={async () => { menu = null; try { await api.renameFolder(f.id, null, null, true); await refreshFolders(); } catch (e) { showToast(errorText(e), true); } }}>Move to top level</button>
      {/if}
      <button role="menuitem" onclick={() => toggleSubscribed(f)} disabled={isProtected(f)}>{f.subscribed ? "Unsubscribe" : "Subscribe"}</button>
      <button role="menuitem" onclick={() => { menu = null; showRules = true; }}>Rules…</button>
      <hr />
      <button role="menuitem" class="danger" onclick={() => askDelete(f)} disabled={isProtected(f)}>Delete…</button>
      {#if isProtected(f)}<div class="muted small pad">Special folder: protected</div>{/if}
    </div>
  {/if}
  {#if ask}
    <div class="modal" role="dialog" aria-modal="true" aria-label={ask.title}>
      <form class="card" onsubmit={(e) => { e.preventDefault(); void submitAsk(); }}>
        <h3>{ask.title}</h3>
        <label>{ask.label} <input bind:value={ask.value} maxlength="200" use:focus /></label>
        <p class="muted small">Any language works (e.g. “Zürich”, “Проекты”, “日本”). Special characters * % and the folder separator aren't allowed.</p>
        {#if askError}<p class="err small">{askError}</p>{/if}
        <div class="row"><button type="button" class="btn" onclick={() => (ask = null)}>Cancel</button><button class="btn primary" disabled={!ask.value.trim()}>{ask.ok}</button></div>
      </form>
    </div>
  {/if}
  {#if confirmDelete}
    {@const c = confirmDelete}
    <div class="modal" role="dialog" aria-modal="true" aria-label="Delete folder">
      <div class="card">
        <h3>Delete “{leafName(c.f)}”?</h3>
        <p>{c.count.toLocaleString()} message{c.count === 1 ? "" : "s"}{c.subs ? ` in this folder and ${c.subs} subfolder${c.subs === 1 ? "" : "s"}` : " in this folder"}.</p>
        {#if c.count > 0}
          <label class="opt"><input type="radio" bind:group={c.mode} value="move_to_trash" /> Move the messages to Trash, then delete the folder{c.subs ? "s" : ""}</label>
          <label class="opt"><input type="radio" bind:group={c.mode} value="delete_messages" /> Delete the messages permanently</label>
        {/if}
        <div class="row"><button class="btn" onclick={() => (confirmDelete = null)}>Cancel</button>
          <button class="btn primary danger-fill" onclick={doDelete}>Delete folder</button></div>
      </div>
    </div>
  {/if}
  {#if picker}
    <div class="modal" role="dialog" aria-modal="true" aria-label={picker.copy ? "Copy to folder" : "Move to folder"}>
      <div class="card picker">
        <h3>{picker.copy ? "Copy" : "Move"} {selection().length} message{selection().length === 1 ? "" : "s"} to…</h3>
        <input placeholder="Filter folders" bind:value={pickerQuery} use:focus
          onkeydown={(e) => { if (e.key === "Enter" && pickerFolders[0]) { const p = picker!; picker = null; void transfer(selection(), pickerFolders[0].f, p.copy); } if (e.key === "Escape") picker = null; }} />
        <div class="plist">
          {#each pickerFolders as { f, depth } (f.id)}
            <button style:padding-left="{10 + depth * 14}px" onclick={() => { const p = picker!; picker = null; void transfer(selection(), f, p.copy); }}>
              <Icon name={roleIcon[f.role ?? ""] ?? "folder"} size={15} /> {leafName(f)}</button>
          {/each}
        </div>
        <div class="row"><button class="btn" onclick={() => (picker = null)}>Cancel</button></div>
      </div>
    </div>
  {/if}
  {#if toast}
    <div class="toast" class:warn={toast.warn} role="status">{toast.text}
      {#if toast.undo}{@const u = toast.undo}<button class="btn sm" onclick={() => undo(u)} title="Undo (Ctrl+Z)">Undo</button>{/if}
      <button class="icon-btn" aria-label="Dismiss" onclick={() => (toast = null)}><Icon name="close" size={14} /></button></div>
  {/if}
{/if}

<style>
  .setup-wrap { height: 100%; overflow: auto; }
  .warn { text-align: center; color: #f5b14c; }
  .app { display: grid; grid-template-columns: 76px 1fr; height: 100%; min-height: 0; }
  .app > :global(*) { min-width: 0; min-height: 0; }
  .shell { display: grid; grid-template-columns: 240px minmax(300px, 380px) 1fr; height: 100%; }
  .shell.withpanel { grid-template-columns: 220px minmax(280px, 340px) 1fr 290px; }
  .sidebar { background: var(--panel); border-right: 1px solid var(--border); display: flex; flex-direction: column; padding: 14px 12px; gap: 6px; min-height: 0; }
  .compose { display: flex; gap: 8px; justify-content: center; align-items: center; padding: 10px; margin-bottom: 8px; }
  nav { display: flex; flex-direction: column; gap: 2px; overflow: auto; }
  .folder { display: flex; gap: 12px; align-items: center; border: 0; background: transparent; padding: 9px 12px; border-radius: 8px; text-align: left; }
  .folder:hover { background: var(--hover); }
  .folder.active { background: var(--selected); }
  .fname { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .badge { background: var(--accent); color: #fff; border-radius: 10px; font-size: 12px; padding: 1px 8px; }
  .spacer { flex: 1; }
  .accounts { display: flex; flex-direction: column; gap: 2px; border-top: 1px solid var(--border); padding-top: 8px; }
  .acct { border: 0; background: transparent; text-align: left; padding: 6px 12px; border-radius: 8px; font-size: 13px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; display: flex; gap: 6px; align-items: center; }
  .acct.active { color: var(--text); } .acct:hover { background: var(--hover); }
  .foot { display: flex; align-items: center; gap: 4px; padding-top: 6px; }
  .small { font-size: 12px; }
  .spin { display: inline-flex; animation: spin 1s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .list { background: var(--panel-2); border-right: 1px solid var(--border); display: flex; flex-direction: column; min-height: 0; }
  .search { display: flex; align-items: center; gap: 8px; margin: 14px 14px 6px; padding: 0 10px; border: 1px solid var(--border); border-radius: 10px; background: var(--panel); color: var(--muted); }
  .search input { border: 0; background: transparent; flex: 1; padding: 10px 0; outline: none; }
  .meta { display: flex; justify-content: space-between; gap: 8px; font-size: 12px; padding: 4px 18px 8px; }
  .status { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 60%; }
  ul { list-style: none; margin: 0; padding: 0 8px 8px; overflow: auto; flex: 1; }
  .item { width: 100%; display: grid; grid-template-columns: 14px 1fr; column-gap: 6px; text-align: left; border: 0; background: transparent; padding: 10px 10px; border-radius: 10px; border-bottom: 1px solid var(--border); }
  .item:hover { background: var(--hover); }
  .item.selected { background: var(--selected); }
  .dot { grid-row: 1; width: 8px; height: 8px; border-radius: 50%; margin-top: 6px; }
  .unread .dot { background: var(--unread); }
  .row1 { display: flex; justify-content: space-between; gap: 8px; }
  .from { font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .unread .from, .unread .subject { font-weight: 700; }
  .star { color: #f5b14c; display: inline-flex; vertical-align: -2px; }
  .date { white-space: nowrap; color: var(--muted); font-size: 12px; flex: none; }
  .subject, .preview { grid-column: 2; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
  .preview { color: var(--muted); }
  mark { background: var(--accent-soft); color: var(--unread); border-radius: 3px; }
  .empty { padding: 24px; text-align: center; }
  .reader { background: var(--panel); min-width: 0; min-height: 0; display: flex; flex-direction: column; }
  .reader > :global(:not(.agenda)) { flex: 1; min-height: 0; }
  .toast { position: fixed; bottom: 18px; left: 50%; transform: translateX(-50%); z-index: 60; display: flex; gap: 8px; align-items: center;
    background: var(--panel); border: 1px solid var(--border); border-left: 4px solid var(--accent); border-radius: 10px;
    padding: 8px 8px 8px 14px; box-shadow: 0 10px 30px #0005; max-width: 70vw; font-size: 13px; }
  .toast.warn { border-left-color: #f5b14c; }
  .navhead { display: flex; align-items: center; gap: 4px; font-size: 12px; padding: 0 4px 0 12px; text-transform: uppercase; letter-spacing: .04em; }
  .navhead span { flex: 1; }
  .sm { padding: 3px 8px; font-size: 12px; }
  .folder.droptarget { background: var(--accent-soft); outline: 2px dashed var(--accent); outline-offset: -2px; }
  .folder.unsub { opacity: .55; }
  .droproot { border: 1px dashed transparent; border-radius: 8px; padding: 6px 12px; display: none; }
  .droproot.show { display: block; }
  .droproot.on, nav.rootdrop .droproot { border-color: var(--accent); background: var(--accent-soft); color: var(--text); display: block; }
  .selbar { display: flex; align-items: center; gap: 8px; margin: 0 14px 6px; padding: 6px 10px; border-radius: 10px; background: var(--accent-soft); font-size: 13px; }
  .selbar span { flex: 1; }
  .msgbar { display: flex; gap: 6px; padding: 8px 16px 0; flex: none !important; }
  .ctx { position: fixed; z-index: 70; background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 4px; box-shadow: 0 10px 30px #0006; min-width: 190px; display: flex; flex-direction: column; }
  .ctx button { border: 0; background: transparent; text-align: left; padding: 7px 12px; border-radius: 6px; }
  .ctx button:hover:not(:disabled) { background: var(--hover); } .ctx button:disabled { opacity: .45; }
  .ctx hr { border: 0; border-top: 1px solid var(--border); margin: 4px 0; } .ctx .danger { color: #e5534b; } .pad { padding: 4px 12px; }
  .modal { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 80; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 18px 20px; width: min(440px, 90vw); display: flex; flex-direction: column; gap: 10px; }
  .card h3 { margin: 0; } .card label { display: flex; flex-direction: column; gap: 4px; } .card label.opt { flex-direction: row; gap: 8px; align-items: center; }
  .card .row { display: flex; justify-content: flex-end; gap: 8px; } .err { color: #e5534b; }
  .danger-fill { background: #d64541 !important; border-color: #d64541 !important; }
  .picker .plist { max-height: 320px; overflow: auto; display: flex; flex-direction: column; }
  .picker .plist button { border: 0; background: transparent; text-align: left; padding: 7px 10px; border-radius: 6px; display: flex; gap: 8px; align-items: center; }
  .picker .plist button:hover { background: var(--hover); }
  :global(.drag-ghost) { position: fixed; top: -100px; padding: 6px 12px; background: var(--accent); color: #fff; border-radius: 8px; font-size: 13px; }
</style>
