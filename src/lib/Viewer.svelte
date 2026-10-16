<script lang="ts">
  import { onDestroy } from "svelte";
  import { Icon } from "@neutron/ui";
  import Composer from "./Composer.svelte";
  import InviteCard from "./InviteCard.svelte";
  import AddressCard from "./AddressCard.svelte";
  import { initials, shortDate, size } from "./format";
  import { api, dayAt, errorText, pimApi, type Account, type ComposeDraft, type ComposeMode, type ComposerResult, type ConversationItem, type MailAddress, type MessageView, type TaskList } from "./api";

  let { message, account, isDraft, hoverUrl, onReload, onStatus, onComposerDone, onCalendarChanged = () => {}, onPimChanged = () => {}, onOpenNote = () => {}, onComposeTo = () => {}, onOpenMessage = () => {} }: {
    message: MessageView | null; account: Account; isDraft: boolean; hoverUrl: string | null;
    onReload: (loadRemote: boolean) => void; onStatus: (s: string, warn?: boolean) => void; onComposerDone: (r: ComposerResult) => void;
    onCalendarChanged?: () => void; onPimChanged?: () => void; onOpenNote?: (id: number) => void; onComposeTo?: (a: MailAddress) => void;
    onOpenMessage?: (id: number) => void;
  } = $props();

  // The whole conversation (all folders, incl. your replies in Sent).
  let conversation = $state<ConversationItem[]>([]);
  let conversationOpen = $state(true);
  let conversationRequest = 0;
  $effect(() => {
    const id = message?.id;
    const request = ++conversationRequest;
    if (id === undefined) { conversation = []; return; }
    api.conversation(id)
      .then((c) => { if (request === conversationRequest) conversation = c; })
      .catch(() => { if (request === conversationRequest) conversation = []; });
  });

  // Sender and recipients as clickable chips; each opens an address card.
  const COLLAPSED_RECIPIENTS = 4;
  let card = $state<{ address: MailAddress; left: number } | null>(null);
  let recipientsOpen = $state(false);
  let headEl: HTMLElement | undefined = $state();
  const sender = $derived<MailAddress | null>(message?.from_addr ? { name: message.from_name, email: message.from_addr } : null);
  const recipients = $derived(message ? [
    ...(message.to_list ?? []).map((a) => ({ kind: "to", a })),
    ...(message.cc_list ?? []).map((a) => ({ kind: "cc", a })),
  ] : []);
  const shownRecipients = $derived(recipientsOpen ? recipients : recipients.slice(0, COLLAPSED_RECIPIENTS));
  function openCard(e: MouseEvent, address: MailAddress) {
    if (card?.address.email === address.email) { card = null; return; }
    const head = headEl?.getBoundingClientRect();
    const at = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const left = head ? Math.max(0, Math.min(at.left - head.left, head.width - 300)) : 0;
    card = { address, left };
  }
  // Inline composer height, adjustable with the splitter above it.
  let viewerEl: HTMLElement | undefined = $state();
  let composerHeight = $state<number | null>(null);
  let split: { y: number; h: number } | null = null;
  function startSplit(e: PointerEvent) {
    const el = (e.currentTarget as HTMLElement).nextElementSibling as HTMLElement | null;
    split = { y: e.clientY, h: el?.getBoundingClientRect().height ?? 320 };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function moveSplit(e: PointerEvent) {
    if (!split) return;
    const total = viewerEl?.getBoundingClientRect().height ?? window.innerHeight;
    composerHeight = Math.round(Math.max(220, Math.min(total - 140, split.h + split.y - e.clientY)));
  }
  function endSplit() { split = null; }
  function splitKey(e: KeyboardEvent) {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    const total = viewerEl?.getBoundingClientRect().height ?? window.innerHeight;
    const now = composerHeight ?? total * 0.55;
    composerHeight = Math.round(Math.max(220, Math.min(total - 140, now + (e.key === "ArrowUp" ? 40 : -40))));
  }
  function trustChanged(email: string, trusted: boolean) {
    // Re-render so the sender's images load (or are blocked) right away.
    if (message && email.toLowerCase() === message.from_addr.toLowerCase()) onReload(trusted);
  }

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
      todoBusy = false; notesBusy = false; trustBusy = false; card = null; recipientsOpen = false;
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
  async function print() {
    const origin = selection();
    if (!origin) return;
    try { await api.printMessage(origin.messageId); }
    catch (e) { if (isCurrent(origin)) onStatus(errorText(e), true); }
  }
  function onWindowKey(e: KeyboardEvent) {
    // Ctrl+P prints the open message, not the whole app window.
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p" && message && !reply) { e.preventDefault(); void print(); }
  }
  function done(key: number, origin: Selection, r: ComposerResult) {
    if (!isCurrent(origin) || reply?.key !== key) return;
    reply = null; onComposerDone(r);
  }
</script>

<svelte:window onkeydown={onWindowKey} />
{#if message}
  <article class="viewer" class:composing={reply !== null} bind:this={viewerEl}>
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
        <button class="tbtn" onclick={print} title="Print this message (Ctrl+P)"><Icon name="external" size={16} /> Print</button>
        <button class="tbtn" onclick={toggleSource} aria-pressed={source !== null || sourceLoading} title="View the raw message source">
          <Icon name="code" size={16} /> {sourceLoading ? "Cancel source" : source !== null ? "Message" : "View source"}</button>
      </div>
      <h1>{message.subject || "(no subject)"}</h1>
      {#if conversation.length > 1}
        <div class="conv">
          <button class="conv-head" onclick={() => (conversationOpen = !conversationOpen)} aria-expanded={conversationOpen}>
            <Icon name="chevron" size={13} /> Conversation · {conversation.length} messages</button>
          {#if conversationOpen}
            <ol class="conv-list">
              {#each conversation as c (c.id)}
                <li>
                  <button class="conv-item" class:current={c.id === message.id} class:unread={!c.seen}
                    aria-current={c.id === message.id ? "true" : undefined} onclick={() => { if (c.id !== message?.id) onOpenMessage(c.id); }}>
                    <span class="cfrom">{c.from_name || c.from_addr}</span>
                    {#if c.folder_id !== message.folder_id}<span class="cfolder">{c.folder_name}</span>{/if}
                    <span class="cprev muted">{c.preview}</span>
                    <span class="cdate muted">{shortDate(c.date)}</span>
                  </button>
                </li>
              {/each}
            </ol>
          {/if}
        </div>
      {/if}
      <div class="head" bind:this={headEl}>
        {#if sender}
          {@const from = sender}
          <button class="avatar" onclick={(e) => openCard(e, from)} title={`${from.name ? `${from.name} ` : ""}<${from.email}>`}
            aria-label={`Contact card for ${from.email}`}>{initials(message.from_name, message.from_addr)}</button>
        {:else}
          <div class="avatar">{initials(message.from_name, message.from_addr)}</div>
        {/if}
        <div class="who">
          <div class="from-line">
            {#if sender}
              {@const from = sender}
              <button class="chip name" onclick={(e) => openCard(e, from)} aria-expanded={card?.address.email === from.email}>
                {message.from_name || message.from_addr}</button>
              {#if message.from_name}<span class="muted addr">&lt;{message.from_addr}&gt;</span>{/if}
            {:else}
              <span class="name">(unknown sender)</span>
            {/if}
            <span class="muted date">{shortDate(message.date)}</span>
          </div>
          {#if recipients.length}
            <div class="recips">
              {#each shownRecipients as r, i (`${r.kind}:${i}:${r.a.email}`)}
                {#if i === 0 || shownRecipients[i - 1].kind !== r.kind}<span class="label muted">{r.kind}</span>{/if}
                <button class="chip" onclick={(e) => openCard(e, r.a)} title={`${r.a.name ? `${r.a.name} ` : ""}<${r.a.email}>`}
                  aria-expanded={card?.address.email === r.a.email}>{r.a.name || r.a.email}</button>
              {/each}
              {#if recipients.length > COLLAPSED_RECIPIENTS}
                <button class="more" onclick={() => (recipientsOpen = !recipientsOpen)} aria-expanded={recipientsOpen}>
                  {recipientsOpen ? "Show less" : `+${recipients.length - COLLAPSED_RECIPIENTS} more`}</button>
              {/if}
            </div>
          {:else if message.to || message.cc}
            <div class="muted addr">to {message.to}{message.cc ? `, cc ${message.cc}` : ""}</div>
          {/if}
        </div>
        {#if card}
          {#key card.address.email}
            <AddressCard address={card.address} accountId={account.id} left={card.left}
              onClose={() => (card = null)} onCompose={onComposeTo} onTrustChanged={trustChanged} {onStatus} />
          {/key}
        {/if}
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
        <!-- WAI-ARIA window splitter: a focusable separator, resized with drag or the arrow keys. -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
        <div class="splitter" role="separator" aria-orientation="horizontal" aria-label="Resize composer" tabindex="0"
          aria-valuemin={220} aria-valuenow={composerHeight ?? undefined}
          title="Drag to resize the composer" onpointerdown={startSplit} onpointermove={moveSplit} onpointerup={endSplit}
          onpointercancel={endSplit} onkeydown={splitKey} ondblclick={() => (composerHeight = null)}></div>
        <div class="inline-composer" style:flex={composerHeight === null ? null : `0 0 ${composerHeight}px`}>
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
  .inline-composer { flex: 1 1 55%; min-height: 220px; padding: 0 16px 14px; display: flex; flex-direction: column; }
  .splitter { flex: none; height: 10px; margin: 0 16px; cursor: ns-resize; position: relative; touch-action: none; outline: none; }
  .splitter::after { content: ""; position: absolute; left: 50%; top: 3px; width: 44px; height: 4px; margin-left: -22px; border-radius: 2px; background: var(--border); }
  .splitter:hover::after, .splitter:focus-visible::after { background: var(--accent); }
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
  .head { display: flex; gap: 12px; align-items: flex-start; position: relative; }
  .conv { border: 1px solid var(--border); border-radius: 10px; background: var(--panel-2); font-size: 13px; }
  .conv-head { display: flex; gap: 6px; align-items: center; width: 100%; border: 0; background: transparent; padding: 6px 10px; color: var(--muted); font-size: 12px; font-weight: 600; }
  .conv-head[aria-expanded="false"] :global(svg) { transform: rotate(-90deg); }
  .conv-list { list-style: none; margin: 0; padding: 0 4px 4px; max-height: 190px; overflow: auto; }
  .conv-item { display: flex; gap: 8px; align-items: baseline; width: 100%; min-width: 0;
    border: 0; background: transparent; text-align: left; padding: 4px 8px; border-radius: 6px; color: var(--text); }
  .cfrom { flex: 0 1 10em; min-width: 3em; }
  .cprev { flex: 1 1 0; min-width: 0; }
  .cfolder, .cdate { flex: none; }
  .conv-item:hover { background: var(--hover); }
  .conv-item.current { background: var(--selected); }
  .conv-item.unread .cfrom { font-weight: 700; }
  .cfrom, .cprev { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cfolder { font-size: 11px; padding: 0 6px; border-radius: 999px; border: 1px solid var(--border); color: var(--muted); }
  .cdate { font-size: 12px; white-space: nowrap; }
  .who { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  .from-line { display: flex; flex-wrap: wrap; align-items: baseline; gap: 6px; min-width: 0; }
  .name { font-weight: 600; }
  .addr { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .date { flex: none; margin-left: auto; padding-left: 8px; font-size: 13px; }
  .avatar { width: 44px; height: 44px; border-radius: 50%; background: var(--accent); color: #fff; display: grid; place-items: center; font-weight: 600; flex: none;
    border: 0; padding: 0; font-size: inherit; }
  button.avatar { cursor: pointer; }
  button.avatar:hover { filter: brightness(1.1); box-shadow: 0 0 0 3px var(--accent-soft); }
  .recips { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 6px; font-size: 13px; }
  .label { text-transform: lowercase; margin-left: 2px; }
  .chip { border: 1px solid transparent; background: var(--panel-2); color: var(--text); border-radius: 999px; padding: 1px 8px; font-size: 13px;
    max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; }
  .chip.name { font-weight: 600; font-size: 14px; padding: 1px 6px; margin-left: -6px; background: transparent; }
  .chip:hover, .chip[aria-expanded="true"] { background: var(--hover); border-color: var(--border); }
  .more { border: 0; background: transparent; color: var(--unread); font-weight: 600; font-size: 13px; padding: 1px 4px; }
  .more:hover { text-decoration: underline; }
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
