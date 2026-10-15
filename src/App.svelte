<script lang="ts">
  import { onMount, untrack } from "svelte";
  import Icon from "./lib/Icon.svelte";
  import AccountSetup from "./lib/AccountSetup.svelte";
  import Compose from "./lib/Compose.svelte";
  import Viewer from "./lib/Viewer.svelte";
  import LinkDialog from "./lib/LinkDialog.svelte";
  import { api, emptyDraft, errorText, type Account, type AppInfo, type ComposeDraft, type ComposerResult, type Folder,
    type LinkInfo, type MessageSummary, type MessageView, type SearchHit } from "./lib/api";
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
  let loadRemote = $state(false);
  let link = $state<LinkInfo | null>(null);
  let hoverUrl = $state<string | null>(null);
  let toast = $state<{ text: string; warn: boolean } | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  function showToast(text: string, warn = false) {
    toast = { text, warn }; clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), warn ? 12000 : 5000);
  }
  let query = $state("");
  let hits = $state<SearchHit[] | null>(null);
  let syncing = $state(false);
  let status = $state("");
  let showSetup = $state(false);
  let compose = $state<ComposeDraft | null>(null);

  const roleIcon: Record<string, string> = { inbox: "inbox", sent: "sent", drafts: "drafts", trash: "trash", archive: "archive", junk: "junk", flagged: "star" };
  const roleOrder = ["inbox", "flagged", "drafts", "sent", "archive", "junk", "trash"];
  const sortedFolders = $derived([...folders].sort((a, b) => {
    const ra = roleOrder.indexOf(a.role ?? ""), rb = roleOrder.indexOf(b.role ?? "");
    return (ra < 0 ? 99 : ra) - (rb < 0 ? 99 : rb) || a.name.localeCompare(b.name);
  }));
  const list = $derived(hits ?? messages);

  async function refreshFolders() {
    if (!account) return;
    folders = await api.listFolders(account.id);
    if (folder) folder = folders.find((f) => f.id === folder!.id) ?? null;
    if (!folder) folder = folders.find((f) => f.role === "inbox") ?? folders[0] ?? null;
  }
  async function refreshMessages() {
    messages = folder ? await api.listMessages(folder.id) : [];
  }
  async function selectAccount(a: Account) {
    account = a; folder = null; detail = null; selectedId = null; hits = null; query = "";
    await refreshFolders(); await refreshMessages();
    void sync();
  }
  async function selectFolder(f: Folder) {
    folder = f; hits = null; query = ""; detail = null; selectedId = null;
    await refreshMessages();
  }
  async function open(m: MessageSummary) {
    selectedId = m.id; loadRemote = false;
    try {
      detail = await api.getMessage(m.id, theme === "dark", false);
      if (!m.seen) { m.seen = true; await refreshFolders(); }
    } catch (e) { status = errorText(e); }
  }
  async function sync() {
    if (!account || syncing) return;
    syncing = true; status = "Syncing…";
    try {
      const r = await api.syncAccount(account.id);
      status = r.failed_folders.length ? `Synced with errors: ${r.failed_folders.join(", ")}` : `Up to date · ${r.new_messages} new`;
      await refreshFolders(); await refreshMessages();
    } catch (e) { status = errorText(e); } finally { syncing = false; }
  }
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  function onSearch() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(async () => {
      if (!account || !query.trim()) { hits = null; return; }
      try { hits = await api.search(account.id, query); } catch (e) { status = errorText(e); }
    }, 200);
  }
  async function reload(remote: boolean) {
    if (selectedId === null) return;
    loadRemote = remote;
    try { detail = await api.getMessage(selectedId, theme === "dark", remote); }
    catch (e) { showToast(errorText(e), true); }
    if (remote && detail && detail.remote_images_failed > 0)
      showToast(`${detail.remote_images_failed} remote image(s) could not be loaded (blocked host, not an image, or too large).`, true);
  }
  // Re-render the open message when the theme changes so the iframe document matches.
  // Only the theme is tracked: opening a message or loading remote images
  // already fetches it, so tracking those too would fetch remote images twice.
  $effect(() => {
    const dark = theme === "dark";
    untrack(() => { if (selectedId !== null) api.getMessage(selectedId, dark, loadRemote).then((d) => (detail = d)).catch(() => {}); });
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
        accounts = await api.listAccounts();
        if (accounts[0]) await selectAccount(accounts[0]); else showSetup = true;
        const u1 = await api.onLink(onLinkEvent);
        const u2 = await api.onHover((u) => (hoverUrl = u));
        const u3 = await api.onSync(async (e) => {
          if (e.account_id !== account?.id) return;
          if (e.error) { status = `Background sync: ${e.error}`; return; }
          if (e.report && e.report.new_messages + e.report.removed_messages > 0) {
            status = `${e.report.new_messages} new message${e.report.new_messages === 1 ? "" : "s"}`;
            await refreshFolders(); await refreshMessages();
          }
        });
        unlisten = () => { u1(); u2(); u3(); };
      } catch (e) { status = errorText(e); }
    })();
    return () => unlisten?.();
  });
</script>

{#if showSetup || !account}
  <main class="setup-wrap">
    <AccountSetup
      onAdded={async (a) => { accounts = [...accounts, a]; showSetup = false; await selectAccount(a); }}
      onCancel={account ? () => (showSetup = false) : undefined} />
    {#if info?.secret_backend === "memory"}
      <p class="warn">No system keyring found: passwords are kept in memory only for this session.</p>
    {/if}
  </main>
{:else}
  <div class="shell">
    <aside class="sidebar">
      <div class="brand"><span class="logo">N</span> Neutron Mail</div>
      <button class="btn primary compose" onclick={() => (compose = emptyDraft())}>
        <Icon name="compose" /> Compose
      </button>
      <nav aria-label="Folders">
        {#each sortedFolders as f (f.id)}
          <button class="folder" class:active={folder?.id === f.id} onclick={() => selectFolder(f)}>
            <Icon name={roleIcon[f.role ?? ""] ?? "folder"} />
            <span class="fname">{f.role === "inbox" ? "Inbox" : f.name}</span>
            {#if f.unread > 0}<span class="badge">{f.unread}</span>{/if}
          </button>
        {/each}
      </nav>
      <div class="spacer"></div>
      <div class="accounts">
        {#each accounts as a (a.id)}
          <button class="acct" class:active={a.id === account.id} onclick={() => selectAccount(a)} title={a.email}>{a.email}</button>
        {/each}
        <button class="acct add" onclick={() => (showSetup = true)}><Icon name="plus" size={14} /> Add account</button>
      </div>
      <div class="foot">
        <button class="icon-btn" onclick={() => (theme = theme === "dark" ? "light" : "dark")} aria-label="Toggle theme" title="Toggle theme">
          <Icon name={theme === "dark" ? "sun" : "moon"} />
        </button>
        <button class="icon-btn" onclick={sync} disabled={syncing} aria-label="Sync now" title="Sync now">
          <span class:spin={syncing}><Icon name="sync" /></span>
        </button>
        {#if info?.secret_backend === "memory"}<span class="muted small" title="No keyring: password kept in memory only">⚠ no keyring</span>{/if}
      </div>
    </aside>

    <section class="list">
      <div class="search">
        <Icon name="search" />
        <input placeholder="Search messages…" bind:value={query} oninput={onSearch} aria-label="Search messages" />
      </div>
      <div class="meta muted">
        {hits ? `${hits.length} result${hits.length === 1 ? "" : "s"}` : `${messages.length} messages`}
        <span class="status" title={status}>{status}</span>
      </div>
      <ul>
        {#each list as m (m.id)}
          <li>
            <button class="item" class:selected={selectedId === m.id} class:unread={!m.seen} onclick={() => open(m)}>
              <span class="dot"></span>
              <span class="row1"><span class="from">{m.from_name || m.from_addr}</span><span class="date">{shortDate(m.date)}</span></span>
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
      <Viewer message={detail} {account} isDraft={folder?.role === "drafts"} {hoverUrl}
        onReload={reload} onStatus={(t) => showToast(t)} onComposerDone={composerDone} />
    </section>
  </div>
  {#if compose}
    <Compose {account} draft={compose} onClose={(r) => { compose = null; void composerDone(r); }} />
  {/if}
  {#if link}
    <LinkDialog {link} onClose={(u) => { link = null; if (u) showToast(`Opened ${new URL(u).host} in your browser`); }} />
  {/if}
  {#if toast}
    <div class="toast" class:warn={toast.warn} role="status">{toast.text}
      <button class="icon-btn" aria-label="Dismiss" onclick={() => (toast = null)}><Icon name="close" size={14} /></button></div>
  {/if}
{/if}

<style>
  .setup-wrap { height: 100%; overflow: auto; }
  .warn { text-align: center; color: #f5b14c; }
  .shell { display: grid; grid-template-columns: 240px minmax(300px, 380px) 1fr; height: 100%; }
  .sidebar { background: var(--panel); border-right: 1px solid var(--border); display: flex; flex-direction: column; padding: 14px 12px; gap: 6px; min-height: 0; }
  .brand { display: flex; align-items: center; gap: 10px; font-weight: 600; padding: 4px 8px 12px; }
  .logo { width: 26px; height: 26px; border-radius: 8px; background: var(--accent); color: #fff; display: grid; place-items: center; font-weight: 700; }
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
  .date { color: var(--muted); font-size: 12px; flex: none; }
  .subject, .preview { grid-column: 2; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
  .preview { color: var(--muted); }
  mark { background: var(--accent-soft); color: var(--unread); border-radius: 3px; }
  .empty { padding: 24px; text-align: center; }
  .reader { background: var(--panel); min-width: 0; min-height: 0; }
  .toast { position: fixed; bottom: 18px; left: 50%; transform: translateX(-50%); z-index: 60; display: flex; gap: 8px; align-items: center;
    background: var(--panel); border: 1px solid var(--border); border-left: 4px solid var(--accent); border-radius: 10px;
    padding: 8px 8px 8px 14px; box-shadow: 0 10px 30px #0005; max-width: 70vw; font-size: 13px; }
  .toast.warn { border-left-color: #f5b14c; }
</style>
