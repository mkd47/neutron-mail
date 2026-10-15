<script lang="ts">
  import { onDestroy } from "svelte";
  import { Icon } from "@neutron/ui";
  import Composer from "./Composer.svelte";
  import InviteCard from "./InviteCard.svelte";
  import { initials, shortDate, size } from "./format";
  import { api, dayAt, errorText, pimApi, type Account, type ComposeDraft, type ComposeMode, type ComposerResult, type MessageView, type TaskList } from "./api";

  let { message, account, isDraft, hoverUrl, onReload, onStatus, onComposerDone, onCalendarChanged = () => {}, onPimChanged = () => {}, onOpenNote = () => {} }: {
    message: MessageView | null; account: Account; isDraft: boolean; hoverUrl: string | null;
    onReload: (loadRemote: boolean) => void; onStatus: (s: string, warn?: boolean) => void; onComposerDone: (r: ComposerResult) => void;
    onCalendarChanged?: () => void; onPimChanged?: () => void; onOpenNote?: (id: number) => void;
  } = $props();

  type Selection = { messageId: number; accountId: number; generation: number };
  let generation = 0;
  let alive = true;
  let lastSelection = "";
  let composeRequest = 0;
  let sourceRequest = 0;
  let attachmentRequest = 0;
  let todoRequest = 0;

  // A result belongs to the selection that started it, even after A → B → A.
  function selection(): Selection | null {
    return message ? { messageId: message.id, accountId: account.id, generation } : null;
  }
  function isCurrent(origin: Selection) {
    return alive && origin.generation === generation && origin.messageId === message?.id && origin.accountId === account.id;
  }
  onDestroy(() => { alive = false; });

  // "Add to To-do" popover (prefilled from the subject) and "Save to Notes".
  let todo = $state<{ title: string; due: string; list: number | null; priority: number } | null>(null);
  let lists = $state<TaskList[]>([]);
  let todoBusy = $state(false);
  let notesBusy = $state(false);
  let trustBusy = $state(false);
  async function openTodo() {
    const origin = selection();
    if (!origin || !message) return;
    const request = ++todoRequest;
    if (todo) { todo = null; return; }
    const title = message.subject || "(no subject)";
    try {
      const loaded = await pimApi.taskLists();
      if (!isCurrent(origin) || request !== todoRequest) return;
      lists = loaded;
      todo = { title, due: "1", list: loaded[0]?.id ?? null, priority: 0 };
    } catch (e) { if (isCurrent(origin) && request === todoRequest) onStatus(errorText(e), true); }
  }
  async function addTodo() {
    const origin = selection();
    if (!origin || !todo || !todo.title.trim() || todoBusy) return;
    const form = todo;
    const task = { ...form };
    todoBusy = true;
    try {
      const email = await pimApi.emailLink(origin.messageId);
      await pimApi.saveTask({ id: null, list_id: task.list, parent_id: null, title: task.title, notes: "",
        due: task.due === "" ? null : dayAt(Number(task.due)), due_time: false, reminder: null, priority: task.priority, email });
      onPimChanged();
      if (!isCurrent(origin)) return;
      onStatus(`Added “${task.title}” to To-do`);
      if (todo === form) todo = null;
    } catch (e) { if (isCurrent(origin)) onStatus(errorText(e), true); }
    finally { if (isCurrent(origin)) todoBusy = false; }
  }
  async function saveToNotes() {
    const origin = selection();
    if (!origin || notesBusy) return;
    notesBusy = true;
    try {
      const n = await pimApi.noteFromMessage(origin.messageId, null);
      onPimChanged();
      if (!isCurrent(origin)) return;
      onStatus(`Saved to Notes: “${n.title}”`); onOpenNote(n.id);
    } catch (e) { if (isCurrent(origin)) onStatus(errorText(e), true); }
    finally { if (isCurrent(origin)) notesBusy = false; }
  }

  let reply = $state<{ mode: ComposeMode; draft: ComposeDraft; key: number; origin: Selection; account: Account } | null>(null);
  let source = $state<string | null>(null);
  let sourceLoading = $state(false);
  let confirmOpen = $state<{ origin: Selection; index: number; reason: string } | null>(null);
  $effect.pre(() => {
    // Reset before rendering another selection; requests also check live props.
    const next = `${account.id}:${message?.id ?? -1}`;
    if (next !== lastSelection) {
      lastSelection = next; generation++;
      source = null; sourceLoading = false; reply = null; confirmOpen = null; todo = null;
      todoBusy = false; notesBusy = false; trustBusy = false;
    }
  });

  const titles: Record<ComposeMode, string> = { reply: "Reply", reply_all: "Reply all", forward: "Forward", draft: "Edit draft" };

  async function startCompose(mode: ComposeMode) {
    const origin = selection();
    if (!origin) return;
    const request = ++composeRequest;
    const sender = { ...account };
    try {
      const draft = await api.composePrefill(origin.messageId, mode);
      if (!isCurrent(origin) || request !== composeRequest) return;
      reply = { mode, draft, key: request, origin, account: sender };
      sourceRequest++; sourceLoading = false; source = null;
    } catch (e) { if (isCurrent(origin) && request === composeRequest) onStatus(errorText(e)); }
  }
  async function toggleSource() {
    const origin = selection();
    if (!origin) return;
    const request = ++sourceRequest;
    if (source !== null || sourceLoading) { source = null; sourceLoading = false; return; }
    sourceLoading = true;
    try {
      const raw = await api.messageSource(origin.messageId);
      if (isCurrent(origin) && request === sourceRequest) source = raw;
    } catch (e) { if (isCurrent(origin) && request === sourceRequest) onStatus(errorText(e)); }
    finally { if (isCurrent(origin) && request === sourceRequest) sourceLoading = false; }
  }
  async function trustSender() {
    const origin = selection();
    if (!origin || !message || trustBusy) return;
    const trust = !message.sender_trusted;
    const sender = message.from_addr;
    trustBusy = true;
    try {
      await api.setImageSenderTrust(origin.accountId, sender, trust);
      if (isCurrent(origin)) onReload(trust);
    } catch (e) { if (isCurrent(origin)) onStatus(errorText(e), true); }
    finally { if (isCurrent(origin)) trustBusy = false; }
  }
  async function save(index: number) {
    const origin = selection();
    if (!origin) return;
    try { const p = await api.saveAttachment(origin.messageId, index); if (p && isCurrent(origin)) onStatus(`Saved to ${p}`); }
    catch (e) { if (isCurrent(origin)) onStatus(errorText(e)); }
  }
  async function openAtt(index: number, confirmed = false, origin = selection()) {
    if (!origin || !isCurrent(origin)) return;
    const request = ++attachmentRequest;
    confirmOpen = null;
    try {
      const r = await api.openAttachment(origin.messageId, index, confirmed);
      if (!isCurrent(origin) || request !== attachmentRequest) return;
      if (r.status === "needs_confirmation") confirmOpen = { origin, index, reason: r.reason ?? "" };
      else onStatus("Opened with the default application");
    } catch (e) { if (isCurrent(origin) && request === attachmentRequest) onStatus(errorText(e)); }
  }
  function confirmAttachment() {
    const pending = confirmOpen;
    confirmOpen = null;
    if (pending) void openAtt(pending.index, true, pending.origin);
  }
  function done(key: number, origin: Selection, r: ComposerResult) {
    if (!isCurrent(origin) || reply?.key !== key) return;
    reply = null; onComposerDone(r);
  }
</script>

{#if message}
  <article class="viewer" class:composing={reply !== null}>
    <div class="message">
      <div class="toolbar" role="toolbar" aria-label="Message actions">
        {#if isDraft}
          <button class="tbtn primary" onclick={() => startCompose("draft")}><Icon name="drafts" size={16} /> Edit draft</button>
        {:else}
          <button class="tbtn" onclick={() => startCompose("reply")} aria-pressed={reply?.mode === "reply"}><Icon name="reply" size={16} /> Reply</button>
          <button class="tbtn" onclick={() => startCompose("reply_all")} aria-pressed={reply?.mode === "reply_all"}><Icon name="replyall" size={16} /> Reply all</button>
          <button class="tbtn" onclick={() => startCompose("forward")} aria-pressed={reply?.mode === "forward"}><Icon name="forward" size={16} /> Forward</button>
        {/if}
        <span class="pim">
          <button class="tbtn" onclick={openTodo} aria-pressed={todo !== null} aria-haspopup="dialog" title="Create a task from this email"><Icon name="checkcircle" size={16} /> Add to To-do</button>
          {#if todo}
            <div class="popover" role="dialog" aria-label="Add to To-do"><form onsubmit={(e) => { e.preventDefault(); void addTodo(); }}>
              <h3>Add to To-do</h3>
              <label>Task title <input bind:value={todo.title} maxlength="300" /></label>
              <label>Due date
                <select bind:value={todo.due}>
                  <option value="0">Today</option><option value="1">Tomorrow</option><option value="7">Next week</option><option value="">No date</option>
                </select></label>
              <label>List
                <select bind:value={todo.list}>{#each lists as l (l.id)}<option value={l.id}>{l.name}</option>{/each}<option value={null}>No list</option></select></label>
              <label>Priority
                <select bind:value={todo.priority}><option value={0}>None</option><option value={1}>High</option><option value={5}>Medium</option><option value={9}>Low</option></select></label>
              <button class="tbtn primary wide" disabled={todoBusy || !todo.title.trim()}><Icon name="plus" size={15} /> Add task</button>
            </form></div>
          {/if}
        </span>
        <button class="tbtn" onclick={saveToNotes} disabled={notesBusy} title="Save this email as a note, linked to it"><Icon name="note" size={16} /> Save to Notes</button>
        <span class="spacer"></span>
        <button class="tbtn" onclick={toggleSource} aria-pressed={source !== null || sourceLoading} title="View the raw message source">
          <Icon name="code" size={16} /> {sourceLoading ? "Cancel source" : source !== null ? "Message" : "View source"}</button>
      </div>
      <h1>{message.subject || "(no subject)"}</h1>
      <div class="head">
        <div class="avatar">{initials(message.from_name, message.from_addr)}</div>
        <div class="who">
          <div class="name">{message.from_name || message.from_addr}</div>
          <div class="muted addr">{message.from_addr}</div>
          <div class="muted addr">to {message.to}{message.cc ? `, cc ${message.cc}` : ""}</div>
        </div>
        <div class="muted">{shortDate(message.date)}</div>
      </div>
      {#if source !== null}
        <!-- Raw RFC 822 source as escaped text (Svelte text node, never HTML). -->
        <pre class="source" aria-label="Message source">{source}</pre>
      {:else}
        {#key `${account.id}:${message.id}`}
          {@const inviteOrigin = selection()}
          <InviteCard messageId={message.id}
            onStatus={(text, warn) => { if (inviteOrigin && isCurrent(inviteOrigin)) onStatus(text, warn); }}
            onResponded={onCalendarChanged} />
        {/key}
        {#if message.remote_images_blocked > 0 || message.remote_images_loaded > 0}
          <div class="notice" role="status">
            <Icon name="shield" size={16} />
            {#if message.remote_images_loaded > 0}
              <span>{message.remote_images_loaded} remote image{message.remote_images_loaded === 1 ? "" : "s"} loaded{message.remote_images_blocked ? `, ${message.remote_images_blocked} blocked (not a valid image or not allowed)` : ""}.</span>
            {:else}
              <span>{message.remote_images_blocked} remote image{message.remote_images_blocked === 1 ? "" : "s"} blocked to protect your privacy.</span>
              {#if message.remote_images_available > 0}<button class="link" onclick={() => onReload(true)}>Load remote images</button>{/if}
            {/if}
            {#if message.from_addr && message.remote_images_available > 0}
              <button class="link" onclick={trustSender} disabled={trustBusy}>{message.sender_trusted ? "Stop loading for this sender" : "Always load from this sender"}</button>
            {/if}
          </div>
        {/if}
        {#if message.document}
          <!-- Empty sandbox: no scripts, no same-origin, no forms, no popups, no top navigation.
               Link clicks are intercepted by the app shell (navigation policy) and confirmed. -->
          <iframe class="body" title="Message content" sandbox="" referrerpolicy="no-referrer" srcdoc={message.document}></iframe>
        {:else}
          <pre class="body text">{message.text}</pre>
        {/if}
      {/if}
      {#if message.attachments.length}
        <div class="attachments">
          {#each message.attachments as a (a.index)}
            <div class="att" class:risky={a.risky}>
              <Icon name="clip" />
              <div class="meta"><div class="aname" title={a.name}>{a.name}</div><div class="muted small">{a.mime} · {size(a.size)}{a.risky ? " · program/script" : ""}</div></div>
              <button class="mini" onclick={() => save(a.index)} title="Save…" aria-label={`Save ${a.name}`}>Save</button>
              <button class="mini" onclick={() => openAtt(a.index)} title="Open with default app" aria-label={`Open ${a.name}`}>Open</button>
            </div>
          {/each}
        </div>
      {/if}
      {#if hoverUrl}<div class="hover" role="status" aria-live="off">{hoverUrl}</div>{/if}
    </div>
    {#if reply}
      {#key reply.key}
        {@const composing = reply}
        <div class="inline-composer">
          <Composer account={composing.account} draft={composing.draft} title={titles[composing.mode]} inline
            replaceDraft={composing.mode === "draft" ? composing.origin.messageId : null}
            onDone={(r) => done(composing.key, composing.origin, r)} />
        </div>
      {/key}
    {/if}
  </article>
  {#if confirmOpen}
    <div class="backdrop" role="presentation">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-label="Open program?">
        <h3>⚠ Open a program or script?</h3>
        <p>{confirmOpen.reason}</p>
        <div class="actions">
          <button class="btn" onclick={() => (confirmOpen = null)}>Cancel</button>
          <button class="btn danger" onclick={confirmAttachment}>Open anyway</button>
        </div>
      </div>
    </div>
  {/if}
{:else}
  <div class="empty muted">Select a message to read it</div>
{/if}

<style>
  .viewer { display: flex; flex-direction: column; height: auto; flex: 1; min-height: 0; overflow: hidden; position: relative; }
  .message { display: flex; flex-direction: column; gap: 10px; padding: 12px 28px 16px; flex: 1; min-height: 0; overflow: auto; position: relative; }
  .composing .message { flex: 1 1 45%; }
  .inline-composer { flex: 1 1 55%; min-height: 300px; padding: 0 16px 14px; display: flex; flex-direction: column; }
  .toolbar { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
  .pim { position: relative; }
  .popover { position: absolute; top: calc(100% + 6px); left: 0; z-index: 40; width: 300px; display: flex; flex-direction: column; gap: 10px;
    background: var(--panel); border: 1px solid var(--border); border-radius: 12px; padding: 14px; box-shadow: 0 12px 32px #0006; }
  .popover form { display: flex; flex-direction: column; gap: 10px; }
  .popover h3 { margin: 0; font-size: 15px; }
  .popover label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--muted); }
  .popover input, .popover select { font-size: 13px; color: var(--text); }
  .wide { justify-content: center; padding: 9px; }
  .tbtn { display: inline-flex; gap: 6px; align-items: center; border: 1px solid var(--border); background: var(--panel-2); border-radius: 8px; padding: 6px 12px; font-size: 13px; }
  .tbtn:hover { background: var(--hover); }
  .tbtn[aria-pressed="true"] { background: var(--selected); border-color: var(--accent); }
  .tbtn.primary { background: var(--accent); color: #fff; border-color: var(--accent); }
  .spacer { flex: 1; }
  h1 { font-size: 22px; margin: 4px 0 0; font-weight: 600; }
  .head { display: flex; gap: 12px; align-items: flex-start; }
  .who { flex: 1; min-width: 0; }
  .name { font-weight: 600; }
  .addr { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .avatar { width: 44px; height: 44px; border-radius: 50%; background: var(--accent); color: #fff; display: grid; place-items: center; font-weight: 600; flex: none; }
  .notice { display: flex; flex-wrap: wrap; gap: 8px 12px; align-items: center; background: var(--accent-soft); border-radius: 8px; padding: 8px 12px; font-size: 13px; }
  .link { border: 0; background: transparent; color: var(--unread); padding: 0; font-weight: 600; font-size: 13px; }
  .link:hover { text-decoration: underline; }
  .body { flex: 1 0 auto; width: 100%; border: 0; min-height: 360px; border-radius: 8px; background: transparent; }
  .composing .body { min-height: 260px; }
  .text { white-space: pre-wrap; word-break: break-word; font: inherit; line-height: 1.55; margin: 0; min-height: 0; }
  .source { flex: 1 0 auto; margin: 0; padding: 12px; background: var(--panel-2); border: 1px solid var(--border); border-radius: 8px;
    font: 12px/1.45 ui-monospace, "DejaVu Sans Mono", monospace; white-space: pre-wrap; word-break: break-all; }
  .attachments { display: flex; flex-wrap: wrap; gap: 10px; }
  .att { display: flex; gap: 10px; align-items: center; border: 1px solid var(--border); border-radius: 10px; padding: 8px 10px; color: var(--unread); max-width: 340px; }
  .att.risky { border-color: #e0525e; color: #e0525e; }
  .meta { color: var(--text); min-width: 0; flex: 1; }
  .aname { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .mini { border: 1px solid var(--border); background: var(--panel-2); border-radius: 6px; padding: 3px 8px; font-size: 12px; color: var(--text); }
  .mini:hover { background: var(--hover); }
  .small { font-size: 12px; }
  .hover { position: sticky; bottom: 0; align-self: flex-start; max-width: 100%; background: var(--panel-2); border: 1px solid var(--border);
    border-radius: 6px; padding: 3px 8px; font-size: 12px; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .empty { height: auto; flex: 1; min-height: 0; display: grid; place-items: center; }
  .backdrop { position: fixed; inset: 0; background: #0008; display: grid; place-items: center; z-index: 40; }
  .dialog { width: min(480px, 92vw); background: var(--panel); border: 1px solid var(--border); border-radius: 14px; padding: 20px 22px; }
  .dialog h3 { margin: 0 0 8px; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
  :global(.btn.danger) { background: #c9374a; border-color: #c9374a; color: #fff; }
</style>
